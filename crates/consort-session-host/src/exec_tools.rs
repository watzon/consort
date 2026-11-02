//! The tools of a task session.
//!
//! The harness runs here on the session host, and the work runs on an
//! execution worker. Each call goes through the same steps: the hub decides
//! with `tool.authorize`, the user answers when the hub asks, and `tool.exec`
//! runs the call. The same operation ID goes out again when the link fails,
//! and the journal of the worker keeps the work from running twice. A call
//! whose result nobody knows never starts with a new ID.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::execution_link::{ExecOpResult, ExecTool, ToolClass};
use consort_proto::ids::{ItemId, RequestId, SessionId, TurnId};
use consort_proto::model::{
    ApprovalDetails, ApprovalMode, CommandCheckOutcome, CommandPayload, FileChange, FileChangeKind,
    FileChangePayload, ItemPayload, ItemStatus, RequestOption, RequestOptionKind, SessionItem,
};
use consort_proto::session_host_link::{
    RequestAnswer, RequestBody, RequestOpened, RequestOutcome, SessionEvent, ToolAuthorize,
    ToolDecision, ToolExec,
};
use consort_proto::time::Timestamp;
use consort_proto::tools::ToolCallResult;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// The name of the MCP server that carries the hub tools and these tools.
pub(crate) const HUB_SERVER: &str = "consort";

/// The option of an approval that allows the call.
const ALLOW: &str = "allow";
/// The option of an approval that denies the call.
const DENY: &str = "deny";

/// How long a call waits for a link that is down, before it says that nobody
/// knows the result. A restart of the hub or of the worker takes less.
const RETRY_LIMIT: Duration = if cfg!(test) {
    Duration::from_millis(200)
} else {
    Duration::from_secs(10 * 60)
};
/// The time between two tries.
const RETRY_EVERY: Duration = if cfg!(test) {
    Duration::from_millis(10)
} else {
    Duration::from_secs(2)
};

/// What the tools need from the rest of the session host.
#[async_trait]
pub trait ExecBackend: Send + Sync {
    /// Asks the approval engine of the hub (`tool.authorize`).
    async fn authorize(&self, request: ToolAuthorize) -> Result<ToolDecision, ErrorBody>;
    /// Runs a call on the worker (`tool.exec`).
    async fn exec(&self, call: ToolExec) -> Result<ExecOpResult, ErrorBody>;
    /// Records an event of the session.
    fn record(&self, session_id: SessionId, event: SessionEvent);
}

/// The timeline item of a running command.
struct ProcessItem {
    id: ItemId,
    turn_id: TurnId,
    payload: CommandPayload,
    created_at: Timestamp,
    /// The output that the item holds, in bytes of the worker's file.
    offset: u64,
}

#[derive(Default)]
struct State {
    turn: Option<TurnId>,
    /// The commands from before a restart of the session host, by process
    /// ID. The first read of one gives it a new timeline item.
    adopted: HashMap<String, String>,
    pending: HashMap<RequestId, oneshot::Sender<RequestAnswer>>,
    processes: HashMap<String, ProcessItem>,
    cancel: CancellationToken,
}

/// The tools of one session.
pub struct ExecTools {
    session_id: SessionId,
    backend: Arc<dyn ExecBackend>,
    mode: Mutex<ApprovalMode>,
    state: Mutex<State>,
}

/// Closes a request when its wait ends in any way.
struct PendingGuard<'a> {
    tools: &'a ExecTools,
    request_id: RequestId,
    outcome: RequestOutcome,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.tools.lock().pending.remove(&self.request_id);
        self.tools.backend.record(
            self.tools.session_id,
            SessionEvent::RequestClosed {
                request_id: self.request_id.clone(),
                outcome: self.outcome,
            },
        );
    }
}

/// Whether the harness item of this tool call stays out of the timeline. The
/// session host writes a better one itself: a command or a file change.
pub(crate) fn writes_own_item(server: Option<&str>, tool: &str) -> bool {
    server == Some(HUB_SERVER)
        && ExecTool::from_name(tool).is_some_and(|t| {
            matches!(
                t.class(),
                ToolClass::Shell | ToolClass::Write | ToolClass::Remove
            ) || matches!(t, ExecTool::ProcessRead | ExecTool::ProcessWait)
        })
}

fn denied(reason: impl Into<String>) -> ToolCallResult {
    ToolCallResult::error(&ErrorBody::new(ErrorCode::PolicyDenied, reason))
}

fn text_arg<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments.get(key).and_then(Value::as_str)
}

/// The paths that a call changes.
fn changed_paths(arguments: &Value) -> Vec<String> {
    ["path", "from", "to"]
        .into_iter()
        .filter_map(|key| text_arg(arguments, key).map(str::to_owned))
        .collect()
}

/// The first line of a command, for one line of text.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

impl ExecTools {
    /// New tools for a session.
    pub fn new(session_id: SessionId, backend: Arc<dyn ExecBackend>) -> Self {
        Self {
            session_id,
            backend,
            mode: Mutex::new(ApprovalMode::Ask),
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Sets the approval mode that the next call runs in.
    pub fn set_mode(&self, mode: ApprovalMode) {
        match self.mode.lock() {
            Ok(mut current) => *current = mode,
            Err(poisoned) => *poisoned.into_inner() = mode,
        }
    }

    fn mode(&self) -> ApprovalMode {
        match self.mode.lock() {
            Ok(mode) => *mode,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Reads the turn from an event of the harness.
    pub fn observe(&self, event: &SessionEvent) {
        if let SessionEvent::TurnStarted { turn_id } = event {
            self.lock().turn = Some(turn_id.clone());
        }
    }

    /// Gives an answer to a request of these tools. False when the request
    /// belongs to the harness.
    pub fn answer(&self, request_id: &RequestId, answer: RequestAnswer) -> bool {
        match self.lock().pending.remove(request_id) {
            Some(waiting) => {
                let _ = waiting.send(answer);
                true
            }
            None => false,
        }
    }

    /// Stops each wait of a running call, after an interrupt or at the end of
    /// the session. The worker keeps running what it started.
    pub fn cancel_waits(&self) {
        let mut state = self.lock();
        state.cancel.cancel();
        state.cancel = CancellationToken::new();
    }

    /// Runs one tool call.
    pub async fn call(
        &self,
        tool: ExecTool,
        mut arguments: Value,
        tool_use_id: Option<String>,
    ) -> ToolCallResult {
        let operation_id = arguments
            .as_object_mut()
            .and_then(|map| map.remove("operation_id"))
            .and_then(|id| id.as_str().map(str::to_owned))
            .or(tool_use_id)
            .unwrap_or_else(|| format!("op-{}", uuid::Uuid::now_v7()));
        let cancel = self.lock().cancel.clone();

        let decision = self
            .retrying(&cancel, || {
                self.backend.authorize(ToolAuthorize {
                    session_id: self.session_id,
                    tool: tool.name().to_owned(),
                    arguments: arguments.clone(),
                    mode: self.mode(),
                })
            })
            .await;
        let check = match decision {
            // With no answer from the hub, nothing runs.
            Err(err) => return ToolCallResult::error(&err),
            Ok(ToolDecision::Allow { check }) => check,
            Ok(ToolDecision::Deny { reason, check }) => {
                self.blocked_item(tool, &arguments, check);
                return denied(reason);
            }
            Ok(ToolDecision::Ask {
                note,
                blocked,
                check,
            }) => match self.ask(tool, &arguments, note, blocked, &cancel).await {
                Ok(()) => check,
                Err(result) => return result,
            },
        };

        let result = self
            .retrying(&cancel, || {
                self.backend.exec(ToolExec {
                    session_id: self.session_id,
                    tool: tool.name().to_owned(),
                    arguments: arguments.clone(),
                    operation_id: operation_id.clone(),
                })
            })
            .await;
        match result {
            Err(err) => {
                self.failed_item(tool, &arguments, check, &err.message);
                ToolCallResult::error(&err)
            }
            Ok(result) => {
                self.result_item(tool, &arguments, check, &result);
                to_tool_result(result)
            }
        }
    }

    /// Asks the worker what this session left open before a restart of the
    /// session host. Returns the note for the next turn. Nothing runs here.
    pub async fn check_open_work(&self) -> String {
        let call = self.backend.exec(ToolExec {
            session_id: self.session_id,
            tool: ExecTool::ProcessList.name().to_owned(),
            arguments: json!({}),
            operation_id: format!("op-{}", uuid::Uuid::now_v7()),
        });
        match tokio::time::timeout(RETRY_LIMIT, call).await {
            Ok(Ok(result)) if !result.is_error => {
                let mut guard = self.lock();
                for process in list_of(&result.result, "processes") {
                    if text_arg(process, "state") == Some("running")
                        && let (Some(id), Some(command)) =
                            (text_arg(process, "process_id"), text_arg(process, "command"))
                    {
                        guard.adopted.insert(id.to_owned(), command.to_owned());
                    }
                }
                open_work_note(&result.result)
            }
            _ => "The session host restarted, and it could not reach the worker to check your commands. Do not start a command again until you know its result.".to_owned(),
        }
    }

    /// Runs a call of the backend again while the link is down.
    async fn retrying<T, F, Fut>(&self, cancel: &CancellationToken, call: F) -> Result<T, ErrorBody>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, ErrorBody>>,
    {
        let deadline = tokio::time::Instant::now() + RETRY_LIMIT;
        loop {
            match call().await {
                Err(err) if err.code == ErrorCode::WorkerOffline => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(unknown_result());
                    }
                    tokio::select! {
                        () = tokio::time::sleep(RETRY_EVERY) => {}
                        () = cancel.cancelled() => return Err(unknown_result()),
                    }
                }
                other => return other,
            }
        }
    }

    /// Asks the user. `Err` holds the result for the agent.
    async fn ask(
        &self,
        tool: ExecTool,
        arguments: &Value,
        note: Option<String>,
        blocked: Vec<String>,
        cancel: &CancellationToken,
    ) -> Result<(), ToolCallResult> {
        let (summary, body) = approval_of(tool, arguments, note, blocked);
        let request_id = RequestId::from(format!("exec-{}", uuid::Uuid::now_v7()));
        let opened = RequestOpened {
            request_id: request_id.clone(),
            summary,
            body,
            options: vec![
                RequestOption {
                    id: ALLOW.to_owned(),
                    label: "Allow".to_owned(),
                    kind: RequestOptionKind::AllowOnce,
                    description: None,
                },
                RequestOption {
                    id: DENY.to_owned(),
                    label: "Deny".to_owned(),
                    kind: RequestOptionKind::DenyOnce,
                    description: None,
                },
            ],
        };
        // The user cannot see all of this in a card, so nobody can allow it.
        if crate::cap::request_must_be_cut(&opened) {
            return Err(denied(crate::sessions::too_long_reason(&opened.body)));
        }
        let (answer_tx, answer_rx) = oneshot::channel();
        self.lock().pending.insert(request_id.clone(), answer_tx);
        self.backend
            .record(self.session_id, SessionEvent::RequestOpened(opened));
        let mut guard = PendingGuard {
            tools: self,
            request_id,
            outcome: RequestOutcome::Canceled,
        };
        let answer = tokio::select! {
            answer = answer_rx => answer.ok(),
            () = cancel.cancelled() => None,
        };
        let Some(answer) = answer else {
            return Err(denied(
                "The user stopped this turn, so the call did not run.",
            ));
        };
        if answer.option_id.as_deref() == Some(ALLOW) {
            guard.outcome = RequestOutcome::Allowed;
            return Ok(());
        }
        guard.outcome = RequestOutcome::Denied;
        Err(denied(answer.text.unwrap_or_else(|| {
            "The user denied this call, so it did not run.".to_owned()
        })))
    }

    fn record_item(
        &self,
        id: ItemId,
        turn_id: TurnId,
        payload: ItemPayload,
        status: ItemStatus,
        created_at: Timestamp,
    ) {
        self.backend.record(
            self.session_id,
            SessionEvent::Item(SessionItem {
                id,
                session_id: self.session_id,
                turn_id,
                payload,
                status,
                created_at,
                updated_at: Timestamp::now(),
            }),
        );
    }

    /// A timeline item for a command that did not run.
    fn blocked_item(&self, tool: ExecTool, arguments: &Value, check: Option<CommandCheckOutcome>) {
        if tool.class() == ToolClass::Shell {
            self.failed_item(tool, arguments, check, "");
        }
    }

    fn failed_item(
        &self,
        tool: ExecTool,
        arguments: &Value,
        check: Option<CommandCheckOutcome>,
        message: &str,
    ) {
        let Some(turn_id) = self.lock().turn.clone() else {
            return;
        };
        let now = Timestamp::now();
        let payload = match tool.class() {
            ToolClass::Shell => {
                let mut payload = command_payload(arguments, check);
                payload.output = message.to_owned();
                ItemPayload::Command(payload)
            }
            ToolClass::Write | ToolClass::Remove if !changed_paths(arguments).is_empty() => {
                ItemPayload::FileChange(file_change_of(tool, arguments))
            }
            _ => return,
        };
        self.record_item(ItemId::new(), turn_id, payload, ItemStatus::Failed, now);
    }

    /// The timeline item of a finished call.
    fn result_item(
        &self,
        tool: ExecTool,
        arguments: &Value,
        check: Option<CommandCheckOutcome>,
        result: &ExecOpResult,
    ) {
        match tool {
            ExecTool::ProcessStart => self.process_started(arguments, check, result),
            ExecTool::ProcessRead | ExecTool::ProcessWait => {
                self.process_updated(arguments, result);
            }
            ExecTool::FsApplyPatch | ExecTool::FsWrite | ExecTool::FsMove | ExecTool::FsRemove => {
                let Some(turn_id) = self.lock().turn.clone() else {
                    return;
                };
                let status = if result.is_error {
                    ItemStatus::Failed
                } else {
                    ItemStatus::Completed
                };
                self.record_item(
                    ItemId::new(),
                    turn_id,
                    ItemPayload::FileChange(file_change_of(tool, arguments)),
                    status,
                    Timestamp::now(),
                );
            }
            _ => {}
        }
    }

    fn process_started(
        &self,
        arguments: &Value,
        check: Option<CommandCheckOutcome>,
        result: &ExecOpResult,
    ) {
        let Some(turn_id) = self.lock().turn.clone() else {
            return;
        };
        let mut payload = command_payload(arguments, check);
        let now = Timestamp::now();
        let id = ItemId::new();
        let process_id = text_arg(&result.result, "process_id").map(str::to_owned);
        let Some(process_id) = process_id.filter(|_| !result.is_error) else {
            payload.output = text_arg(&result.result, "message")
                .unwrap_or_default()
                .to_owned();
            self.record_item(
                id,
                turn_id,
                ItemPayload::Command(payload),
                ItemStatus::Failed,
                now,
            );
            return;
        };
        self.record_item(
            id,
            turn_id.clone(),
            ItemPayload::Command(payload.clone()),
            ItemStatus::InProgress,
            now,
        );
        self.lock().processes.insert(
            process_id,
            ProcessItem {
                id,
                turn_id,
                payload,
                created_at: now,
                offset: 0,
            },
        );
    }

    fn process_updated(&self, arguments: &Value, result: &ExecOpResult) {
        let Some(process_id) = text_arg(arguments, "process_id") else {
            return;
        };
        let value = &result.result;
        let state = text_arg(value, "state").unwrap_or("running");
        let running = state == "running";
        let update = {
            let mut guard = self.lock();
            if let Some(command) = guard.adopted.remove(process_id)
                && let Some(turn_id) = guard.turn.clone()
            {
                let item = ProcessItem {
                    id: ItemId::new(),
                    turn_id,
                    payload: command_payload(&json!({"command": command}), None),
                    created_at: Timestamp::now(),
                    offset: arguments.get("offset").and_then(Value::as_u64).unwrap_or(0),
                };
                guard.processes.insert(process_id.to_owned(), item);
            }
            let Some(process) = guard.processes.get_mut(process_id) else {
                return;
            };
            // The agent can read the same output twice. Only new bytes count.
            let from = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0);
            let next = value
                .get("next_offset")
                .and_then(Value::as_u64)
                .unwrap_or(from);
            if from <= process.offset && next > process.offset {
                let output = text_arg(value, "output").unwrap_or_default();
                process
                    .payload
                    .append_output(tail_after(output, process.offset - from));
                process.offset = next;
            }
            if let Some(code) = value.get("exit_code").and_then(Value::as_i64) {
                process.payload.exit_code = i32::try_from(code).ok();
            }
            if let Some(ms) = value.get("duration_ms").and_then(Value::as_u64) {
                process.payload.duration_ms = Some(ms);
            }
            let snapshot = (
                process.id,
                process.turn_id.clone(),
                process.payload.clone(),
                process.created_at,
            );
            if !running {
                guard.processes.remove(process_id);
            }
            snapshot
        };
        let (id, turn_id, payload, created_at) = update;
        let status = match (running, payload.exit_code) {
            (true, _) => ItemStatus::InProgress,
            (false, Some(0)) => ItemStatus::Completed,
            (false, _) => ItemStatus::Failed,
        };
        self.record_item(
            id,
            turn_id,
            ItemPayload::Command(payload),
            status,
            created_at,
        );
    }
}

fn list_of<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// What the agent reads before it goes on after a restart of the session
/// host: the result of `process_list`.
fn open_work_note(list: &Value) -> String {
    let mut note = String::from(
        "The session host restarted, and your last turn stopped. Consort checked the worker.",
    );
    let processes = list_of(list, "processes");
    if !processes.is_empty() {
        note.push_str(" The commands of this session:");
    }
    for process in processes {
        let state = text_arg(process, "state").unwrap_or("unknown");
        let id = text_arg(process, "process_id").unwrap_or_default();
        let command = first_line(text_arg(process, "command").unwrap_or_default());
        note.push_str(&format!("\n- {id}: {state}"));
        if let Some(code) = process.get("exit_code").and_then(Value::as_i64) {
            note.push_str(&format!(", exit code {code}"));
        }
        note.push_str(&format!(": {command}"));
    }
    let unknown = list_of(list, "unknown_operations");
    if !unknown.is_empty() {
        note.push_str("\nThese operations have no known result:");
    }
    for operation in unknown {
        let tool = text_arg(operation, "tool").unwrap_or_default();
        let id = text_arg(operation, "operation_id").unwrap_or_default();
        note.push_str(&format!("\n- {tool} ({id})"));
    }
    note.push_str(
        "\nUse process_wait to get the result of a command. Do not start a command or repeat an operation again until you know that it did not finish. If you cannot find out, say so in your report.",
    );
    note
}

/// The part of `output` after the first `skip` bytes. Output comes in UTF-8
/// text with lossy decoding, so the cut moves to a character boundary.
fn tail_after(output: &str, skip: u64) -> &str {
    let mut at = usize::try_from(skip)
        .unwrap_or(usize::MAX)
        .min(output.len());
    while !output.is_char_boundary(at) {
        at += 1;
    }
    &output[at..]
}

/// The error of a call that may have run.
fn unknown_result() -> ErrorBody {
    ErrorBody::new(
        ErrorCode::WorkerOffline,
        "The worker did not answer, so nobody knows whether this call ran. Check the workspace first. To send the call again, use the same operation_id and the same arguments.",
    )
}

/// What the agent reads. A replay says so, because the work did not run again.
fn to_tool_result(result: ExecOpResult) -> ToolCallResult {
    let mut value = result.result;
    if result.replayed_from_journal
        && let Some(map) = value.as_object_mut()
    {
        map.insert("replayed_from_journal".to_owned(), json!(true));
    }
    let mut out = ToolCallResult::json(&value);
    out.is_error = result.is_error;
    out
}

fn command_payload(arguments: &Value, check: Option<CommandCheckOutcome>) -> CommandPayload {
    CommandPayload {
        command: text_arg(arguments, "command")
            .or_else(|| text_arg(arguments, "text"))
            .unwrap_or_default()
            .to_owned(),
        cwd: text_arg(arguments, "cwd").map(str::to_owned),
        output: String::new(),
        output_truncated: false,
        exit_code: None,
        duration_ms: None,
        check,
    }
}

fn file_change_of(tool: ExecTool, arguments: &Value) -> FileChangePayload {
    let path = |key: &str| text_arg(arguments, key).unwrap_or_default().to_owned();
    let (lines_added, lines_removed) = match tool {
        ExecTool::FsApplyPatch => {
            arguments
                .get("edits")
                .and_then(Value::as_array)
                .map_or((0, 0), |edits| {
                    edits.iter().fold((0_u32, 0_u32), |(added, removed), edit| {
                        let lines = |key: &str| {
                            u32::try_from(text_arg(edit, key).unwrap_or_default().lines().count())
                                .unwrap_or(u32::MAX)
                        };
                        (
                            added.saturating_add(lines("new")),
                            removed.saturating_add(lines("old")),
                        )
                    })
                })
        }
        ExecTool::FsWrite => (
            u32::try_from(
                text_arg(arguments, "content")
                    .unwrap_or_default()
                    .lines()
                    .count(),
            )
            .unwrap_or(u32::MAX),
            0,
        ),
        _ => (0, 0),
    };
    let change = match tool {
        ExecTool::FsMove => FileChange {
            path: path("to"),
            change: FileChangeKind::Rename,
            from_path: Some(path("from")),
            lines_added,
            lines_removed,
            diff: None,
        },
        _ => FileChange {
            path: path("path"),
            change: match tool {
                ExecTool::FsRemove => FileChangeKind::Delete,
                ExecTool::FsWrite if arguments.get("expected_sha256").is_none() => {
                    FileChangeKind::Add
                }
                _ => FileChangeKind::Modify,
            },
            from_path: None,
            lines_added,
            lines_removed,
            diff: None,
        },
    };
    FileChangePayload {
        changes: vec![change],
    }
}

/// The one line and the details of an approval.
fn approval_of(
    tool: ExecTool,
    arguments: &Value,
    note: Option<String>,
    blocked: Vec<String>,
) -> (String, RequestBody) {
    match tool.class() {
        ToolClass::Shell => {
            let command = text_arg(arguments, "command")
                .or_else(|| text_arg(arguments, "text"))
                .unwrap_or_default()
                .to_owned();
            (
                format!("Run {}", first_line(&command)),
                RequestBody::Approval(ApprovalDetails::Command {
                    command,
                    cwd: text_arg(arguments, "cwd").map(str::to_owned),
                    check_note: note,
                    blocked,
                }),
            )
        }
        ToolClass::Write | ToolClass::Remove if !changed_paths(arguments).is_empty() => {
            let paths = changed_paths(arguments);
            (
                format!("Change {}", paths.join(", ")),
                RequestBody::Approval(ApprovalDetails::FileChange { paths }),
            )
        }
        _ => (
            format!("Use the tool {}", tool.name()),
            RequestBody::Approval(ApprovalDetails::Tool {
                server: Some(HUB_SERVER.to_owned()),
                tool: tool.name().to_owned(),
                arguments: arguments.clone(),
            }),
        ),
    }
}

/// The verdict for the blocked item of a test.
#[cfg(test)]
fn verdict_of(item: &SessionItem) -> Option<consort_proto::model::CommandCheckVerdict> {
    match &item.payload {
        ItemPayload::Command(command) => command.check.as_ref().map(|c| c.verdict),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use consort_proto::model::CommandCheckVerdict;

    use super::*;

    #[derive(Default)]
    struct Fake {
        decision: Mutex<Option<ToolDecision>>,
        offline_times: AtomicUsize,
        exec_calls: Mutex<Vec<ToolExec>>,
        results: Mutex<Vec<ExecOpResult>>,
        events: Mutex<Vec<SessionEvent>>,
    }

    #[async_trait]
    impl ExecBackend for Fake {
        async fn authorize(&self, _request: ToolAuthorize) -> Result<ToolDecision, ErrorBody> {
            Ok(self
                .decision
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(ToolDecision::Allow { check: None }))
        }

        async fn exec(&self, call: ToolExec) -> Result<ExecOpResult, ErrorBody> {
            self.exec_calls.lock().unwrap().push(call);
            if self.offline_times.load(Ordering::SeqCst) > 0 {
                self.offline_times.fetch_sub(1, Ordering::SeqCst);
                return Err(ErrorBody::new(ErrorCode::WorkerOffline, "offline"));
            }
            let mut results = self.results.lock().unwrap();
            Ok(if results.is_empty() {
                ExecOpResult {
                    result: json!({"ok": true}),
                    replayed_from_journal: false,
                    is_error: false,
                }
            } else {
                results.remove(0)
            })
        }

        fn record(&self, _session_id: SessionId, event: SessionEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn tools(fake: &Arc<Fake>) -> Arc<ExecTools> {
        let tools = Arc::new(ExecTools::new(SessionId::new(), fake.clone()));
        tools.observe(&SessionEvent::TurnStarted {
            turn_id: TurnId::new("turn-1"),
        });
        tools
    }

    fn items(fake: &Fake) -> Vec<SessionItem> {
        fake.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Item(item) => Some(item.clone()),
                _ => None,
            })
            .collect()
    }

    fn check(verdict: CommandCheckVerdict) -> CommandCheckOutcome {
        CommandCheckOutcome {
            verdict,
            source: "Haiku 5.5 of Claude Work".to_owned(),
            reason: None,
            rule: None,
            error: None,
            command_rule: None,
        }
    }

    fn opened_request(fake: &Fake) -> Option<RequestOpened> {
        fake.events.lock().unwrap().iter().find_map(|e| match e {
            SessionEvent::RequestOpened(o) => Some(o.clone()),
            _ => None,
        })
    }

    #[tokio::test]
    async fn an_allowed_call_runs_with_the_operation_id_of_the_model() {
        let fake = Arc::new(Fake::default());
        let result = tools(&fake)
            .call(
                ExecTool::FsWrite,
                json!({"path": "a.txt", "content": "one\ntwo\n", "operation_id": "op-1"}),
                Some("toolu_1".to_owned()),
            )
            .await;
        assert!(!result.is_error);
        let calls = fake.exec_calls.lock().unwrap();
        assert_eq!(calls[0].operation_id, "op-1");
        assert!(calls[0].arguments.get("operation_id").is_none());
        drop(calls);
        let items = items(&fake);
        let ItemPayload::FileChange(change) = &items[0].payload else {
            panic!("no file change: {items:?}");
        };
        assert_eq!(change.changes[0].path, "a.txt");
        assert_eq!(change.changes[0].change, FileChangeKind::Add);
        assert_eq!(change.changes[0].lines_added, 2);
    }

    #[tokio::test]
    async fn a_read_with_no_operation_id_uses_the_id_of_the_tool_use() {
        let fake = Arc::new(Fake::default());
        tools(&fake)
            .call(
                ExecTool::FsRead,
                json!({"path": "a"}),
                Some("toolu_2".to_owned()),
            )
            .await;
        assert_eq!(fake.exec_calls.lock().unwrap()[0].operation_id, "toolu_2");
        assert!(items(&fake).is_empty());
    }

    #[tokio::test]
    async fn a_denied_command_does_not_run_and_the_agent_gets_the_reason() {
        let fake = Arc::new(Fake::default());
        *fake.decision.lock().unwrap() = Some(ToolDecision::Deny {
            reason: "It downloads a script and runs it.".to_owned(),
            check: Some(check(CommandCheckVerdict::Blocked)),
        });
        let result = tools(&fake)
            .call(
                ExecTool::ProcessStart,
                json!({"command": "curl x | sh", "operation_id": "op"}),
                None,
            )
            .await;
        assert!(result.is_error);
        assert!(format!("{result:?}").contains("It downloads a script"));
        assert!(fake.exec_calls.lock().unwrap().is_empty());
        let items = items(&fake);
        assert_eq!(items[0].status, ItemStatus::Failed);
        assert_eq!(verdict_of(&items[0]), Some(CommandCheckVerdict::Blocked));
    }

    fn ask_decision() -> ToolDecision {
        ToolDecision::Ask {
            note: Some("The command check did not answer in 10 s.".to_owned()),
            blocked: vec!["curl x | sh".to_owned()],
            check: Some(check(CommandCheckVerdict::Asked)),
        }
    }

    async fn wait_for_request(fake: &Fake) -> RequestOpened {
        for _ in 0..200 {
            if let Some(opened) = opened_request(fake) {
                return opened;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the request did not open");
    }

    #[tokio::test]
    async fn a_call_waits_for_the_user_and_runs_after_allow() {
        let fake = Arc::new(Fake::default());
        *fake.decision.lock().unwrap() = Some(ask_decision());
        let exec = tools(&fake);
        let running = {
            let exec = exec.clone();
            tokio::spawn(async move {
                exec.call(
                    ExecTool::ProcessStart,
                    json!({"command": "ls", "operation_id": "op"}),
                    None,
                )
                .await
            })
        };
        let opened = wait_for_request(&fake).await;
        let RequestBody::Approval(ApprovalDetails::Command {
            check_note,
            blocked,
            ..
        }) = &opened.body
        else {
            panic!("not a command: {opened:?}");
        };
        assert!(check_note.as_deref().unwrap().contains("10 s"));
        assert_eq!(blocked, &vec!["curl x | sh".to_owned()]);
        assert!(fake.exec_calls.lock().unwrap().is_empty());
        assert!(exec.answer(
            &opened.request_id,
            RequestAnswer {
                option_id: Some(ALLOW.to_owned()),
                text: None
            }
        ));
        running.await.unwrap();
        assert_eq!(fake.exec_calls.lock().unwrap().len(), 1);
        assert!(fake.events.lock().unwrap().iter().any(|e| matches!(
            e,
            SessionEvent::RequestClosed {
                outcome: RequestOutcome::Allowed,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn a_denied_answer_does_not_run_the_call() {
        let fake = Arc::new(Fake::default());
        *fake.decision.lock().unwrap() = Some(ask_decision());
        let exec = tools(&fake);
        let running = {
            let exec = exec.clone();
            tokio::spawn(async move {
                exec.call(
                    ExecTool::FsRemove,
                    json!({"path": "a", "operation_id": "op"}),
                    None,
                )
                .await
            })
        };
        let opened = wait_for_request(&fake).await;
        assert!(matches!(
            opened.body,
            RequestBody::Approval(ApprovalDetails::FileChange { .. })
        ));
        exec.answer(
            &opened.request_id,
            RequestAnswer {
                option_id: Some(DENY.to_owned()),
                text: None,
            },
        );
        assert!(running.await.unwrap().is_error);
        assert!(fake.exec_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_interrupt_ends_the_wait_and_closes_the_request() {
        let fake = Arc::new(Fake::default());
        *fake.decision.lock().unwrap() = Some(ask_decision());
        let exec = tools(&fake);
        let running = {
            let exec = exec.clone();
            tokio::spawn(async move {
                exec.call(
                    ExecTool::ProcessStart,
                    json!({"command": "ls", "operation_id": "op"}),
                    None,
                )
                .await
            })
        };
        wait_for_request(&fake).await;
        exec.cancel_waits();
        assert!(running.await.unwrap().is_error);
        assert!(fake.exec_calls.lock().unwrap().is_empty());
        assert!(fake.events.lock().unwrap().iter().any(|e| matches!(
            e,
            SessionEvent::RequestClosed {
                outcome: RequestOutcome::Canceled,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn a_lost_link_sends_the_same_operation_id_again() {
        let fake = Arc::new(Fake::default());
        fake.offline_times.store(2, Ordering::SeqCst);
        let result = tools(&fake)
            .call(
                ExecTool::FsWrite,
                json!({"path": "a", "content": "x", "operation_id": "op-7"}),
                None,
            )
            .await;
        assert!(!result.is_error);
        let calls = fake.exec_calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|c| c.operation_id == "op-7"));
    }

    #[tokio::test]
    async fn a_link_that_stays_down_gives_an_unknown_result() {
        let fake = Arc::new(Fake::default());
        fake.offline_times.store(1_000_000, Ordering::SeqCst);
        let result = tools(&fake)
            .call(
                ExecTool::ProcessStart,
                json!({"command": "make", "operation_id": "op-8"}),
                None,
            )
            .await;
        assert!(result.is_error);
        assert!(format!("{result:?}").contains("nobody knows"));
    }

    #[tokio::test]
    async fn a_command_item_follows_its_process_and_counts_each_byte_once() {
        let fake = Arc::new(Fake::default());
        let snapshot = |output: &str, next: u64, state: &str, code: Option<i32>| ExecOpResult {
            result: json!({"process_id": "p1", "state": state, "output": output, "next_offset": next, "exit_code": code}),
            replayed_from_journal: false,
            is_error: false,
        };
        *fake.results.lock().unwrap() = vec![
            ExecOpResult {
                result: json!({"process_id": "p1", "state": "running"}),
                replayed_from_journal: false,
                is_error: false,
            },
            snapshot("one\n", 4, "running", None),
            // The agent reads from the start again.
            snapshot("one\ntwo\n", 8, "exited", Some(0)),
        ];
        let exec = tools(&fake);
        exec.call(
            ExecTool::ProcessStart,
            json!({"command": "make", "operation_id": "op"}),
            None,
        )
        .await;
        exec.call(ExecTool::ProcessWait, json!({"process_id": "p1"}), None)
            .await;
        exec.call(
            ExecTool::ProcessWait,
            json!({"process_id": "p1", "offset": 0}),
            None,
        )
        .await;
        let items = items(&fake);
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|i| i.id == items[0].id));
        let ItemPayload::Command(last) = &items[2].payload else {
            panic!("not a command");
        };
        assert_eq!(last.output, "one\ntwo\n");
        assert_eq!(last.exit_code, Some(0));
        assert_eq!(items[2].status, ItemStatus::Completed);
    }

    #[tokio::test]
    async fn a_replay_says_so_to_the_agent() {
        let fake = Arc::new(Fake::default());
        *fake.results.lock().unwrap() = vec![ExecOpResult {
            result: json!({"process_id": "p1"}),
            replayed_from_journal: true,
            is_error: false,
        }];
        let result = tools(&fake)
            .call(
                ExecTool::ProcessStart,
                json!({"command": "make", "operation_id": "op"}),
                None,
            )
            .await;
        assert_eq!(
            result.structured_content.unwrap()["replayed_from_journal"],
            true
        );
    }

    #[test]
    fn the_note_after_a_restart_names_the_commands_and_the_unknown_operations() {
        let note = open_work_note(&json!({
            "processes": [
                {"process_id": "p1", "command": "cargo test\nmore", "state": "running", "exit_code": null},
                {"process_id": "p2", "command": "ls", "state": "exited", "exit_code": 2},
            ],
            "unknown_operations": [{"operation_id": "op9", "tool": "fs_write"}],
        }));
        assert!(note.contains("- p1: running: cargo test\n"), "{note}");
        assert!(note.contains("- p2: exited, exit code 2: ls"), "{note}");
        assert!(note.contains("- fs_write (op9)"), "{note}");
        assert!(note.contains("Do not start a command"), "{note}");
    }

    #[test]
    fn the_harness_item_of_a_change_or_a_process_stays_out() {
        assert!(writes_own_item(Some("consort"), "process_start"));
        assert!(writes_own_item(Some("consort"), "process_wait"));
        assert!(writes_own_item(Some("consort"), "fs_write"));
        assert!(!writes_own_item(Some("consort"), "fs_read"));
        assert!(!writes_own_item(Some("consort"), "post_message"));
        assert!(!writes_own_item(Some("other"), "process_start"));
    }
}
