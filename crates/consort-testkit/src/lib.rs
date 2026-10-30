//! Test helpers for Consort.
//!
//! [`FakeHarness`] is a harness that plays a script of events. Each turn
//! plays the next entry of [`FakeScript::turns`] (the last entry repeats).
//! A script can open an approval and wait for the answer, so tests can
//! round-trip `session.respond` without a real harness.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use consort_harness::{
    AccountConfig, AccountStatus, Answer, Capabilities, EventReceiver, Harness, HarnessError,
    HarnessKind, HarnessSession, SessionSpec, SteerFuture,
};
use consort_proto::ids::{ItemId, NativeRef, RequestId, TurnId};
use consort_proto::model::{
    ApprovalDetails, ApprovalMode, AssistantMessagePayload, AuthState, CommandPayload, ItemPayload,
    ItemStatus, ModelInfo, RequestOption, RequestOptionKind, SessionItem, SessionState,
};
use consort_proto::time::Timestamp;
use consort_proto::worker_link::{
    RequestBody, RequestOpened, RequestOutcome, SessionEvent, TurnEndReason, TurnInput,
};
use tokio::sync::{mpsc, oneshot, watch};

/// One step of a fake turn.
#[derive(Debug, Clone)]
pub enum FakeStep {
    /// Send this event as it is.
    Event(SessionEvent),
    /// A completed assistant message.
    Text(String),
    /// A completed command with its output.
    Command {
        /// The command line.
        command: String,
        /// The output.
        output: String,
    },
    /// Open a command approval and wait for the answer. The turn goes on
    /// after any answer; a deny adds a "Denied." message.
    Approval {
        /// The request ID.
        request_id: String,
        /// The command.
        command: String,
    },
    /// Open an approval for an MCP tool and wait for the answer, like
    /// `Approval`.
    ToolApproval {
        /// The request ID.
        request_id: String,
        /// The MCP server name.
        server: String,
        /// The tool name.
        tool: String,
    },
    /// Wait.
    Sleep(Duration),
    /// End the turn as failed.
    Fail(String),
}

/// The script of a fake session.
#[derive(Debug, Clone, Default)]
pub struct FakeScript {
    /// One list of steps per turn. The last list repeats.
    pub turns: Vec<Vec<FakeStep>>,
}

impl FakeScript {
    /// A script whose every turn replies with one message.
    pub fn reply(text: &str) -> Self {
        Self {
            turns: vec![vec![FakeStep::Text(text.to_owned())]],
        }
    }
}

/// What a fake harness saw, for assertions.
#[derive(Debug, Default)]
pub struct FakeRecord {
    /// The spec of every `open`.
    pub opened: Vec<SessionSpec>,
    /// Every turn input.
    pub turns: Vec<TurnInput>,
    /// Every steer input.
    pub steers: Vec<TurnInput>,
    /// Every answer.
    pub answers: Vec<(RequestId, Answer)>,
    /// How many interrupts arrived.
    pub interrupts: usize,
    /// How many sessions closed.
    pub closed: usize,
    /// The account of every probe, with its runtime settings.
    pub probed: Vec<AccountConfig>,
    /// The account of every `open`, with its runtime settings.
    pub opened_accounts: Vec<AccountConfig>,
    /// Every approval mode change, in order.
    pub mode_changes: Vec<ApprovalMode>,
    /// The approval mode that each turn ran in, in order.
    pub turn_modes: Vec<ApprovalMode>,
}

/// How [`FakeHarness::open`] fails, after [`FakeHarness::set_open_failure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeOpenFailure {
    /// The harness says that the account is not signed in, with this
    /// reason.
    SignedOut(String),
    /// The harness process exits before the session opens, as when a stop
    /// signal reaches it.
    Exited,
}

/// A harness that plays a script.
#[derive(Debug, Clone)]
pub struct FakeHarness {
    kind: HarnessKind,
    script: FakeScript,
    record: Arc<Mutex<FakeRecord>>,
    capabilities: Capabilities,
    /// Shared by the clones, so a test can change it after it gives the
    /// harness to a worker.
    open_failure: Arc<Mutex<Option<FakeOpenFailure>>>,
    /// When set, `open` with `resume` fails.
    pub fail_resume: bool,
    /// When set, every `turn` fails.
    pub fail_turn: bool,
    /// How long the answer to a steer takes.
    pub steer_delay: Duration,
    /// The capabilities of a session, when they differ from the adapter's,
    /// as when an adapter has to start a session in a lower mode.
    pub session_capabilities: Option<Capabilities>,
    /// When set, the fake asks for the managed tool of its accounts, like a
    /// real adapter. Off by default, so a worker test installs nothing.
    pub uses_managed_tools: bool,
    /// When set, the session cannot change its approval mode, like a
    /// harness that takes its mode only at start.
    pub fixed_mode: bool,
}

impl FakeHarness {
    /// A fake harness that reports itself as `kind`.
    pub fn new(kind: HarnessKind, script: FakeScript) -> Self {
        Self {
            kind,
            script,
            record: Arc::new(Mutex::new(FakeRecord::default())),
            open_failure: Arc::new(Mutex::new(None)),
            capabilities: Capabilities {
                steer: true,
                resume: true,
                usage: false,
                mcp_http: true,
                approval_modes: ApprovalMode::ALL.to_vec(),
                ..Capabilities::default()
            },
            fail_resume: false,
            fail_turn: false,
            steer_delay: Duration::ZERO,
            session_capabilities: None,
            uses_managed_tools: false,
            fixed_mode: false,
        }
    }

    /// The same fake with other capabilities, for example without Full
    /// access.
    pub fn with_capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// What the harness saw so far.
    pub fn record(&self) -> Arc<Mutex<FakeRecord>> {
        self.record.clone()
    }

    /// Makes every `open` of this harness and of its clones fail, or work
    /// again with `None`.
    pub fn set_open_failure(&self, failure: Option<FakeOpenFailure>) {
        if let Ok(mut current) = self.open_failure.lock() {
            *current = failure;
        }
    }

    /// The model that the fake account offers.
    pub fn model() -> ModelInfo {
        ModelInfo {
            id: "fake-model".to_owned(),
            name: "Fake model".to_owned(),
            options: Vec::new(),
        }
    }
}

fn now_item(spec: &SessionSpec, turn: &TurnId, payload: ItemPayload) -> SessionEvent {
    let now = Timestamp::now();
    SessionEvent::Item(SessionItem {
        id: ItemId::new(),
        session_id: spec.session_id,
        turn_id: turn.clone(),
        payload,
        status: ItemStatus::Completed,
        created_at: now,
        updated_at: now,
    })
}

#[async_trait]
impl Harness for FakeHarness {
    fn kind(&self) -> HarnessKind {
        self.kind
    }

    fn capabilities(&self, _account: &AccountConfig) -> Capabilities {
        self.capabilities.clone()
    }

    fn managed_tool(&self, account: &AccountConfig) -> Option<consort_harness::ToolId> {
        if self.uses_managed_tools {
            account.managed_tool()
        } else {
            None
        }
    }

    async fn probe(&self, account: &AccountConfig) -> AccountStatus {
        if let Ok(mut record) = self.record.lock() {
            record.probed.push(account.clone());
        }
        AccountStatus {
            harness_version: Some("0.0.0-fake".to_owned()),
            auth_state: AuthState::SignedIn,
            auth_detail: Some("Fake account".to_owned()),
            models: vec![Self::model()],
            capabilities: self.capabilities.clone(),
            ..AccountStatus::default()
        }
    }

    async fn open(
        &self,
        account: &AccountConfig,
        spec: SessionSpec,
    ) -> Result<(Box<dyn HarnessSession>, EventReceiver), HarnessError> {
        if spec.resume.is_some() && self.fail_resume {
            return Err(HarnessError::ResumeFailed(
                "the fake harness forgot".to_owned(),
            ));
        }
        let failure = self.open_failure.lock().ok().and_then(|f| f.clone());
        match failure {
            Some(FakeOpenFailure::SignedOut(reason)) => {
                return Err(HarnessError::SignedOut(reason));
            }
            Some(FakeOpenFailure::Exited) => return Err(HarnessError::Closed),
            None => {}
        }
        if let Ok(mut record) = self.record.lock() {
            record.opened.push(spec.clone());
            record.opened_accounts.push(account.clone());
        }
        let (events, rx) = mpsc::channel(256);
        let native = spec
            .resume
            .clone()
            .unwrap_or_else(|| NativeRef::new(format!("fake-{}", spec.session_id)));
        let _ = events
            .send(SessionEvent::NativeRef {
                native_ref: native.clone(),
            })
            .await;
        let _ = events
            .send(SessionEvent::State {
                state: SessionState::Idle,
            })
            .await;
        let (cancel, _) = watch::channel(0_u64);
        let mode = spec.approval_mode;
        Ok((
            Box::new(FakeSession {
                spec,
                script: self.script.clone(),
                capabilities: self
                    .session_capabilities
                    .clone()
                    .unwrap_or_else(|| self.capabilities.clone()),
                native,
                events,
                record: self.record.clone(),
                waiters: Arc::new(Mutex::new(HashMap::new())),
                turn_index: 0,
                cancel,
                fail_turn: self.fail_turn,
                steer_delay: self.steer_delay,
                mode,
                fixed_mode: self.fixed_mode,
            }),
            rx,
        ))
    }
}

type Waiters = Arc<Mutex<HashMap<RequestId, oneshot::Sender<Answer>>>>;

struct FakeSession {
    spec: SessionSpec,
    script: FakeScript,
    capabilities: Capabilities,
    native: NativeRef,
    events: mpsc::Sender<SessionEvent>,
    record: Arc<Mutex<FakeRecord>>,
    waiters: Waiters,
    turn_index: usize,
    cancel: watch::Sender<u64>,
    fail_turn: bool,
    steer_delay: Duration,
    /// The approval mode that the next turn runs in.
    mode: ApprovalMode,
    fixed_mode: bool,
}

async fn play(
    spec: SessionSpec,
    steps: Vec<FakeStep>,
    turn: TurnId,
    events: mpsc::Sender<SessionEvent>,
    waiters: Waiters,
    mut cancel: watch::Receiver<u64>,
) {
    let send = |e: SessionEvent| {
        let events = events.clone();
        async move {
            let _ = events.send(e).await;
        }
    };
    send(SessionEvent::TurnStarted {
        turn_id: turn.clone(),
    })
    .await;
    send(SessionEvent::State {
        state: SessionState::Active,
    })
    .await;
    let mut reason = TurnEndReason::Completed;
    let mut error = None;
    for step in steps {
        if cancel.has_changed().unwrap_or(false) {
            reason = TurnEndReason::Interrupted;
            break;
        }
        match step {
            FakeStep::Event(event) => send(event).await,
            FakeStep::Text(text) => {
                send(now_item(
                    &spec,
                    &turn,
                    ItemPayload::AssistantMessage(AssistantMessagePayload { text }),
                ))
                .await;
            }
            FakeStep::Command { command, output } => {
                let payload = CommandPayload {
                    command,
                    output,
                    exit_code: Some(0),
                    ..CommandPayload::default()
                };
                send(now_item(&spec, &turn, ItemPayload::Command(payload))).await;
            }
            step @ (FakeStep::Approval { .. } | FakeStep::ToolApproval { .. }) => {
                let (request_id, summary, body) = match step {
                    FakeStep::ToolApproval {
                        request_id,
                        server,
                        tool,
                    } => (
                        request_id,
                        format!("mcp__{server}__{tool}"),
                        ApprovalDetails::Tool {
                            server: Some(server),
                            tool,
                            arguments: serde_json::Value::Null,
                        },
                    ),
                    FakeStep::Approval {
                        request_id,
                        command,
                    } => (
                        request_id,
                        format!("Run `{command}`"),
                        ApprovalDetails::Command {
                            command,
                            // Like the real adapters, the fake names the
                            // folder of every command approval.
                            cwd: Some(spec.workspace.to_string_lossy().into_owned()),
                        },
                    ),
                    _ => unreachable!("the outer pattern allows only approvals"),
                };
                let request_id = RequestId::new(request_id);
                let (tx, rx) = oneshot::channel();
                if let Ok(mut w) = waiters.lock() {
                    w.insert(request_id.clone(), tx);
                }
                send(SessionEvent::RequestOpened(RequestOpened {
                    request_id: request_id.clone(),
                    summary,
                    body: RequestBody::Approval(body),
                    options: vec![
                        RequestOption {
                            id: "allow".into(),
                            label: "Allow".into(),
                            kind: RequestOptionKind::AllowOnce,
                            description: None,
                        },
                        RequestOption {
                            id: "deny".into(),
                            label: "Deny".into(),
                            kind: RequestOptionKind::DenyOnce,
                            description: None,
                        },
                    ],
                }))
                .await;
                tokio::select! {
                    answer = rx => {
                        let allowed = answer.ok().and_then(|a| a.option_id).as_deref() == Some("allow");
                        let outcome = if allowed { RequestOutcome::Allowed } else { RequestOutcome::Denied };
                        send(SessionEvent::RequestClosed { request_id, outcome }).await;
                        if !allowed {
                            send(now_item(&spec, &turn, ItemPayload::AssistantMessage(AssistantMessagePayload { text: "Denied.".into() }))).await;
                        }
                    }
                    _ = cancel.changed() => {
                        send(SessionEvent::RequestClosed { request_id, outcome: RequestOutcome::Canceled }).await;
                        reason = TurnEndReason::Interrupted;
                        break;
                    }
                }
            }
            FakeStep::Sleep(duration) => {
                tokio::select! {
                    () = tokio::time::sleep(duration) => {}
                    _ = cancel.changed() => { reason = TurnEndReason::Interrupted; break; }
                }
            }
            FakeStep::Fail(message) => {
                reason = TurnEndReason::Failed;
                error = Some(message);
                break;
            }
        }
    }
    send(SessionEvent::TurnEnded {
        turn_id: turn,
        reason,
        error,
    })
    .await;
    send(SessionEvent::State {
        state: SessionState::Idle,
    })
    .await;
}

#[async_trait]
impl HarnessSession for FakeSession {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn native_ref(&self) -> Option<NativeRef> {
        Some(self.native.clone())
    }

    async fn turn(&mut self, input: TurnInput) -> Result<TurnId, HarnessError> {
        if let Ok(mut record) = self.record.lock() {
            record.turns.push(input);
            record.turn_modes.push(self.mode);
        }
        if self.fail_turn {
            return Err(HarnessError::Protocol(
                "the fake harness refused the turn".to_owned(),
            ));
        }
        let steps = self
            .script
            .turns
            .get(self.turn_index)
            .or_else(|| self.script.turns.last())
            .cloned()
            .unwrap_or_default();
        self.turn_index += 1;
        let turn = TurnId::new(format!("fake-turn-{}", self.turn_index));
        tokio::spawn(play(
            self.spec.clone(),
            steps,
            turn.clone(),
            self.events.clone(),
            self.waiters.clone(),
            self.cancel.subscribe(),
        ));
        Ok(turn)
    }

    fn steer(&mut self, input: TurnInput) -> SteerFuture {
        if let Ok(mut record) = self.record.lock() {
            record.steers.push(input);
        }
        let delay = self.steer_delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Ok(())
        })
    }

    async fn set_approval_mode(&mut self, mode: ApprovalMode) -> Result<(), HarnessError> {
        if self.fixed_mode {
            return Err(HarnessError::Unsupported("change the approval mode"));
        }
        if let Ok(mut record) = self.record.lock() {
            record.mode_changes.push(mode);
        }
        self.mode = mode;
        Ok(())
    }

    async fn interrupt(&mut self) -> Result<(), HarnessError> {
        if let Ok(mut record) = self.record.lock() {
            record.interrupts += 1;
        }
        self.cancel.send_modify(|n| *n += 1);
        Ok(())
    }

    async fn respond(&mut self, request: RequestId, answer: Answer) -> Result<(), HarnessError> {
        let waiter = self
            .waiters
            .lock()
            .ok()
            .and_then(|mut w| w.remove(&request));
        let Some(waiter) = waiter else {
            return Err(HarnessError::UnknownRequest(request.to_string()));
        };
        if let Ok(mut record) = self.record.lock() {
            record.answers.push((request, answer.clone()));
        }
        let _ = waiter.send(answer);
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<(), HarnessError> {
        if let Ok(mut record) = self.record.lock() {
            record.closed += 1;
        }
        let _ = self
            .events
            .send(SessionEvent::State {
                state: SessionState::Ended,
            })
            .await;
        Ok(())
    }
}

/// The code that the fake `claude` accepts in `auth login`.
pub const FAKE_CLAUDE_GOOD_CODE: &str = "good-code-123";

/// The sign-in URL that the fake `claude` prints.
pub const FAKE_CLAUDE_URL: &str =
    "https://claude.example/oauth/authorize?code=true&client_id=fake&state=abc";

/// Writes a fake `claude` program to `dir` and returns its path. It is a
/// POSIX shell script that acts like Claude Code 2.1.312 for the commands
/// that the worker runs:
///
/// - `auth login --claudeai|--console`: writes its arguments and the values
///   of `BROWSER` and `DISABLE_AUTOUPDATER` to `$CLAUDE_CONFIG_DIR/fake-login`,
///   prints the URL wrapped in terminal codes, and reads a code. With
///   [`FAKE_CLAUDE_GOOD_CODE`] it marks the folder signed in and exits 0.
///   Else it prints "Login failed: ..." and exits 1.
/// - `auth status --json`: prints the JSON of Claude Code, signed in when
///   the folder has the mark. It exits 1 when nobody is signed in.
/// - `auth logout`: removes the mark.
///
/// The mark is the file `fake-signed-in`. It holds no credential.
#[cfg(unix)]
pub fn write_fake_claude(dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        r#"#!/bin/sh
home="${{CLAUDE_CONFIG_DIR:-.}}"
mkdir -p "$home"
case "$1 $2" in
  "auth login")
    printf '%s|BROWSER=%s|DISABLE_AUTOUPDATER=%s\n' "$3" "$BROWSER" "$DISABLE_AUTOUPDATER" > "$home/fake-login"
    printf 'Opening browser to sign in...\r\n'
    printf 'If the browser did not open, visit: \033]8;;{url}\007\033[94m{url}\033[39m\033]8;;\007\r\n'
    printf 'Paste code here if prompted > '
    read code
    if [ "$code" = "{good}" ]; then
      touch "$home/fake-signed-in"
      printf 'Login successful.\r\n'
      exit 0
    fi
    printf "\033[31mLogin failed: That code was not accepted.\033[0m\r\n"
    exit 1
    ;;
  "auth status")
    if [ -f "$home/fake-signed-in" ]; then
      printf '{{"loggedIn": true, "authMethod": "claude.ai", "email": "fake@example.com", "subscriptionType": "max"}}\n'
      exit 0
    fi
    printf '{{"loggedIn": false, "authMethod": "none"}}\n'
    exit 1
    ;;
  "auth logout")
    rm -f "$home/fake-signed-in"
    printf 'Successfully logged out.\n'
    exit 0
    ;;
esac
echo "fake claude: unknown command $*" >&2
exit 2
"#,
        url = FAKE_CLAUDE_URL,
        good = FAKE_CLAUDE_GOOD_CODE,
    );
    let path = dir.join("claude");
    std::fs::write(&path, script).expect("write the fake claude");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make the fake claude executable");
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::ids::SessionId;
    use std::collections::BTreeMap;

    fn spec() -> SessionSpec {
        SessionSpec {
            session_id: SessionId::new(),
            workspace: std::env::temp_dir(),
            runtime_dir: std::env::temp_dir(),
            model: "fake-model".into(),
            options: Default::default(),
            approval_mode: ApprovalMode::Ask,
            allow_shell: true,
            system_prompt_append: String::new(),
            mcp_servers: vec![],
            env: BTreeMap::new(),
            extras_enabled: true,
            skills: None,
            resume: None,
            process_watcher: None,
        }
    }

    async fn until_end(rx: &mut EventReceiver) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        while let Some(e) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
        {
            let end = matches!(e, SessionEvent::TurnEnded { .. });
            out.push(e);
            if end {
                break;
            }
        }
        out
    }

    #[tokio::test]
    async fn plays_a_turn_with_an_approval() {
        let script = FakeScript {
            turns: vec![vec![
                FakeStep::Approval {
                    request_id: "r1".into(),
                    command: "ls".into(),
                },
                FakeStep::Text("Done.".into()),
            ]],
        };
        let harness = FakeHarness::new(HarnessKind::ClaudeCode, script);
        let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
        let (mut session, mut rx) = harness.open(&account, spec()).await.unwrap();
        session
            .turn(TurnInput {
                text: "go".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        loop {
            if let Some(SessionEvent::RequestOpened(_)) = rx.recv().await {
                break;
            }
        }
        session
            .respond(
                RequestId::new("r1"),
                Answer {
                    option_id: Some("allow".into()),
                    text: None,
                },
            )
            .await
            .unwrap();
        let rest = until_end(&mut rx).await;
        assert!(rest.iter().any(|e| matches!(
            e,
            SessionEvent::RequestClosed {
                outcome: RequestOutcome::Allowed,
                ..
            }
        )));
        assert!(matches!(
            rest.last(),
            Some(SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
                ..
            })
        ));
        assert_eq!(harness.record().lock().unwrap().answers.len(), 1);
    }

    #[tokio::test]
    async fn interrupt_ends_the_turn() {
        let script = FakeScript {
            turns: vec![vec![FakeStep::Sleep(Duration::from_secs(30))]],
        };
        let harness = FakeHarness::new(HarnessKind::Codex, script);
        let account = AccountConfig::default_for(HarnessKind::Codex);
        let (mut session, mut rx) = harness.open(&account, spec()).await.unwrap();
        session.turn(TurnInput::default()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        session.interrupt().await.unwrap();
        let events = until_end(&mut rx).await;
        assert!(matches!(
            events.last(),
            Some(SessionEvent::TurnEnded {
                reason: TurnEndReason::Interrupted,
                ..
            })
        ));
    }
}
