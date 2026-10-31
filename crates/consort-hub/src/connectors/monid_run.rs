//! The Monid hub tools: `search_tools`, `get_monid_tool` and
//! `run_monid_tool`, with the approval steps, the call rows and the client
//! requests (ADR 0016).
//!
//! The hub starts a run once. It never tries again by itself, because Monid
//! documents no idempotency and a second try can charge twice.

use std::time::{Duration, Instant};

use consort_proto::client_api::{
    ChannelResult, ChannelSetMonid, MONID_CALLS_PAGE_MAX, MonidCallGet, MonidCallList,
    MonidCallListResult, MonidCallResult, MonidCallStop, MonidCatalogSearch,
    MonidCatalogSearchResult, MonidCatalogTool, MonidGetStatus, MonidRemoveKey, MonidSetKey,
    MonidStatusResult, MonidUpdateSettings,
};
use consort_proto::error::ErrorCode;
use consort_proto::ids::{ApprovalId, ChannelId, MonidCallId, RoutineId, SessionId, TaskId};
use consort_proto::model::{
    Approval, ApprovalDetails, ApprovalMode, ApprovalRule, ApprovalStatus, Author, BlobRef,
    ChannelFile, MONID_CARD_ALLOW_TASK, MONID_CARD_ALWAYS, MONID_CARD_NEVER, MessageKind,
    MonidAllowedTool, MonidCall, MonidCallNotice, MonidCallState, MonidCard, MonidChannelAccess,
    MonidLimitNote, MonidLimits, MonidSettings, MonidStatus, MonidToolRef, MonidUnattended,
    TaskCreator,
};
use consort_proto::status::Actor;
use consort_proto::tools::{
    GetMonidToolArgs, GetMonidToolResult, MONID_RESULT_MAX_BYTES, RunMonidToolArgs,
    RunMonidToolResult, SEARCH_TOOLS_MAX, SearchToolAccess, SearchToolRow, SearchToolsArgs,
    SearchToolsResult,
};
use consort_proto::turn_input::{WakeEvent, render_events};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::composio::ApiKey;
use super::monid::{self, Client, Run, RunStatus};
use super::{KEY_MAX_CHARS, month_bounds};
use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub, ToolCtx};
use crate::limits::cut_text;
use crate::ops::flow::deliver_in_tx;
use crate::store::conversation::NewMessage;
use crate::store::{Tx, connectors as con_store, conversation, coordinator as coord_store};
use crate::store::{monid as store, routines, work};

/// The most tools from the connectors in one `search_tools` result, so that
/// Monid's rows stay in it.
const SEARCH_CONNECTOR_ROWS_MAX: usize = 8;
/// The longest line about a tool in `search_tools`.
const SEARCH_LINE_MAX_CHARS: usize = 140;
/// The longest name of a tool on a card or a row.
const NAME_MAX_CHARS: usize = 60;
/// The most text about a late result in a session's turn.
const LATE_RESULT_MAX_BYTES: usize = 32 * 1024;
/// A poll that fails this many times in a row ends the wait for a run.
const FOLLOW_FAILURES_MAX: u32 = 100;
/// How long the hub follows one run.
const FOLLOW_MAX: Duration = Duration::from_secs(24 * 60 * 60);
/// What a refused tool says, on the row and to the session.
const REFUSED_TEXT: &str = "This channel does not allow this tool. To allow it, open the Channel panel, then Connectors, then Monid.";
/// What a run that Monid stopped says.
const STOPPED_BY_MONID_TEXT: &str = "The Monid balance or a budget on Monid's site stopped this run. Add money or change the budget on Monid's site, then ask again.";

/// Where a call came from.
#[derive(Debug, Clone, Copy)]
struct CallScope {
    channel_id: ChannelId,
    session_id: SessionId,
    task_id: Option<TaskId>,
}

impl CallScope {
    fn of(ctx: &ToolCtx) -> Self {
        Self {
            channel_id: ctx.channel.id,
            session_id: ctx.session.id,
            task_id: ctx.task.as_ref().map(|t| t.id),
        }
    }

    /// The key of "Allow for this task": the task, or the coordinator
    /// session.
    fn grant_key(&self) -> String {
        match self.task_id {
            Some(task) => task.to_string(),
            None => self.session_id.to_string(),
        }
    }
}

/// A call that may run.
#[derive(Debug, Clone)]
struct PendingCall {
    scope: CallScope,
    provider: String,
    endpoint: String,
    name: String,
    rate: String,
    input: Value,
    approval_id: Option<ApprovalId>,
}

/// A turn that no user started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unattended {
    Routine(Option<RoutineId>),
    Proactive,
}

/// What the hub knows when it decides about a call.
#[derive(Debug, Clone)]
struct GateFacts {
    access: MonidChannelAccess,
    settings: MonidSettings,
    /// The setting that applies to this unattended turn.
    unattended: Option<MonidUnattended>,
    full_access: bool,
    granted: bool,
    over_limit: Option<String>,
}

/// What the hub does with a call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Gate {
    Refuse(String),
    Ask {
        rule: Option<ApprovalRule>,
        over_limit: Option<String>,
    },
    Run,
}

/// The steps of ADR 0016. The first step that matches decides.
fn decide(facts: &GateFacts, provider: &str, endpoint: &str) -> Gate {
    if facts.access.refuses(provider, endpoint) {
        return Gate::Refuse(REFUSED_TEXT.to_owned());
    }
    if facts.unattended == Some(MonidUnattended::Off) {
        return Gate::Refuse(
            "Work that no user started cannot use Monid here. To change it, open Settings, then Connectors, then Monid."
                .to_owned(),
        );
    }
    if let Some(note) = &facts.over_limit {
        return Gate::Ask {
            rule: None,
            over_limit: Some(note.clone()),
        };
    }
    if facts.full_access || facts.granted || facts.access.runs_without_asking(provider, endpoint) {
        return Gate::Run;
    }
    let rule = match facts.unattended {
        Some(MonidUnattended::RunWithoutAsking) => return Gate::Run,
        Some(_) => ApprovalRule::AskEveryTime,
        None => facts.access.approval.unwrap_or(facts.settings.approval),
    };
    match rule {
        ApprovalRule::RunWithoutAsking => Gate::Run,
        rule => Gate::Ask {
            rule: Some(rule),
            over_limit: None,
        },
    }
}

/// The spend that the limits compare with.
#[derive(Debug, Clone, Copy, Default)]
struct Spent {
    task: Option<i64>,
    channel_month: i64,
    hub_month: i64,
}

/// Why a call is over a limit that the user set. An estimate that is not
/// known counts as over a limit per call, because the hub cannot show that
/// the call stays under it. The other limits compare what was spent.
fn over_limit_note(limits: &MonidLimits, estimate: Option<i64>, spent: Spent) -> Option<String> {
    let money = monid::money_text;
    if let Some(limit) = limits.per_call_micros {
        match estimate {
            None => {
                return Some(format!(
                    "The hub cannot tell what this call will cost, and your limit for each call is {}.",
                    money(limit)
                ));
            }
            Some(cost) if cost > limit => {
                return Some(format!(
                    "This call costs about {}, and your limit for each call is {}.",
                    money(cost),
                    money(limit)
                ));
            }
            _ => {}
        }
    }
    let check = |limit: Option<i64>, spent: i64, whose: &str, suffix: &str| {
        let limit = limit?;
        let total = spent.saturating_add(estimate.unwrap_or(0));
        if total <= limit && spent < limit {
            return None;
        }
        let spent_text = format!(
            "{whose} has spent {} of its {} limit{suffix}.",
            money(spent),
            money(limit)
        );
        Some(match estimate {
            Some(cost) if cost > 0 => format!(
                "{spent_text} This call costs {} and goes over it.",
                money(cost)
            ),
            _ => spent_text,
        })
    };
    spent
        .task
        .and_then(|task| check(limits.per_task_micros, task, "This task", ""))
        .or_else(|| {
            check(
                limits.per_channel_month_micros,
                spent.channel_month,
                "This channel",
                " this month",
            )
        })
        .or_else(|| {
            check(
                limits.per_hub_month_micros,
                spent.hub_month,
                "This hub",
                " this month",
            )
        })
}

/// The input of a run, in the shape of `POST /v1/run`. Empty parts stay out.
fn run_input(body: Value, query_params: Value, path_params: Value) -> Value {
    let mut input = serde_json::Map::new();
    for (key, part) in [
        ("body", body),
        ("queryParams", query_params),
        ("pathParams", path_params),
    ] {
        let empty = part.is_null() || part.as_object().is_some_and(|map| map.is_empty());
        if !empty {
            input.insert(key.to_owned(), part);
        }
    }
    Value::Object(input)
}

/// The name of a tool on a card or a row.
pub(super) fn tool_name(tool: &monid::Tool) -> String {
    // `discover` gives no summary, only a description.
    let text = match tool.summary.trim() {
        "" => tool.description.trim(),
        summary => summary,
    };
    if !text.is_empty() {
        let first = text
            .split(". ")
            .next()
            .unwrap_or(text)
            .trim_end_matches('.');
        return crate::util::truncate_chars(first, NAME_MAX_CHARS);
    }
    if tool.provider_name.trim().is_empty() {
        format!("{} {}", tool.provider, tool.endpoint)
    } else {
        format!("{} {}", tool.provider_name.trim(), tool.endpoint)
    }
}

/// One line about a tool.
fn tool_line(tool: &monid::Tool) -> String {
    let text = if tool.summary.trim().is_empty() {
        tool.description.trim()
    } else {
        tool.summary.trim()
    };
    crate::util::truncate_chars(text, SEARCH_LINE_MAX_CHARS)
}

/// The output for the session and the client. A large output is the start of
/// its JSON text, as a string.
fn cut_output(output: &Value) -> (Value, Option<String>) {
    let text = output.to_string();
    if text.len() <= MONID_RESULT_MAX_BYTES {
        return (output.clone(), None);
    }
    let cut = crate::limits::json_prefix(&text, MONID_RESULT_MAX_BYTES - 1024);
    (Value::String(cut.to_owned()), Some(text))
}

/// What a final run means for its call.
fn state_of_run(run: &Run) -> MonidCallState {
    match run.status() {
        RunStatus::Completed if run.provider_failed() => MonidCallState::ProviderError,
        RunStatus::Completed => MonidCallState::Completed,
        RunStatus::Blocked => MonidCallState::StoppedByMonid,
        RunStatus::Stopped => MonidCallState::Stopped,
        RunStatus::TimedOut => MonidCallState::TimedOut,
        RunStatus::Failed | RunStatus::Going => MonidCallState::Failed,
    }
}

/// What went wrong in a final run, with what to do next.
fn error_of_run(run: &Run, state: MonidCallState) -> Option<String> {
    let detail = match &run.error {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
        Value::Object(map) => map
            .get("message")
            .and_then(Value::as_str)
            .map(|text| text.trim().to_owned()),
        _ => None,
    }
    .map(|text| crate::util::truncate_chars(&text, 1_000));
    match state {
        MonidCallState::Completed | MonidCallState::Running => None,
        MonidCallState::StoppedByMonid => Some(STOPPED_BY_MONID_TEXT.to_owned()),
        MonidCallState::Stopped => Some("The run stopped on request.".to_owned()),
        MonidCallState::TimedOut => Some(
            "The run went over its time limit on Monid. Try again with less work in one call."
                .to_owned(),
        ),
        MonidCallState::ProviderError => Some(detail.unwrap_or_else(|| {
            format!(
                "The provider answered with status {}. Try again later, or use another tool.",
                run.provider_response.http_status.unwrap_or(0)
            )
        })),
        _ => Some(detail.unwrap_or_else(|| "The run failed, and Monid gave no reason.".to_owned())),
    }
}

/// The result of a call that ended, for the session.
fn result_of_call(call: &MonidCall) -> RunMonidToolResult {
    RunMonidToolResult::Ran {
        call_id: call.id,
        successful: call.state == MonidCallState::Completed,
        cost: monid::money_text(call.cost_micros),
        data: call.output.clone(),
        error: call.error.clone(),
        truncated: call.output_blob.is_some(),
        files: call
            .files
            .iter()
            .map(|file| file_path(call, &file.name))
            .collect(),
    }
}

/// The path of a saved file in the channel's files. The run's first letters
/// keep two runs with the same file name apart.
fn file_path(call: &MonidCall, name: &str) -> String {
    let prefix: String = call
        .run_id
        .as_deref()
        .unwrap_or("run")
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("monid/{prefix}-{name}")
}

/// The text for a session about a call that ended after its tool call
/// returned.
fn late_text(call: &MonidCall) -> String {
    let text = call.output.to_string();
    let shown = crate::limits::json_prefix(&text, LATE_RESULT_MAX_BYTES);
    let cut = if call.output_blob.is_some() || shown.len() < text.len() {
        " The result is cut."
    } else {
        ""
    };
    let cost = monid::money_text(call.cost_micros);
    let files = if call.files.is_empty() {
        String::new()
    } else {
        let paths: Vec<String> = call
            .files
            .iter()
            .map(|f| file_path(call, &f.name))
            .collect();
        format!(" Files: {}.", paths.join(", "))
    };
    if call.state == MonidCallState::Completed {
        format!(
            "The Monid call {} ended (call {}). It cost {cost}.{files}{cut} Result: {shown}",
            call.name, call.id
        )
    } else {
        format!(
            "The Monid call {} did not give a result (call {}). It cost {cost}: {}",
            call.name,
            call.id,
            call.error.as_deref().unwrap_or("no reason given")
        )
    }
}

/// Saves a call and draws its row again.
fn write_call(tx: &mut Tx<'_>, call: &MonidCall) -> Result<()> {
    tx.save_monid_call(call)?;
    if let Some(id) = call.message_id
        && let Some(mut message) = conversation::message(tx.db(), id)?
    {
        message.body = serde_json::to_string(&MonidCallNotice::of(call))?;
        message.attachments = call.files.clone();
        tx.save_message(&message)?;
    }
    Ok(())
}

/// Saves a new call with its row: in the task thread, or in the timeline.
fn insert_call(tx: &mut Tx<'_>, call: &mut MonidCall) -> Result<()> {
    let body = serde_json::to_string(&MonidCallNotice::of(call))?;
    let task = match call.task_id {
        Some(id) => work::task(tx.db(), id)?,
        None => None,
    };
    let message = match task {
        Some(task) => {
            let session = work::session(tx.db(), call.session_id)?;
            tx.post_in_task_thread(
                &task,
                session.as_ref(),
                Author::System,
                MessageKind::Notice,
                &body,
            )?
        }
        None => {
            let mut new =
                NewMessage::new(call.channel_id, Author::System, MessageKind::Notice, body);
            new.author_session_id = Some(call.session_id);
            tx.insert_message(new)?
        }
    };
    call.message_id = Some(message.id);
    tx.save_monid_call(call)
}

/// Adds a tool to a channel's list, or to its blocks.
fn add_card_choice(access: &mut MonidChannelAccess, tool: MonidToolRef, always: bool) {
    let same = |a: &MonidToolRef| a.provider == tool.provider && a.endpoint == tool.endpoint;
    if always {
        access.blocked.retain(|b| !same(b));
        match access.allowed.iter_mut().find(|a| same(&a.tool)) {
            Some(entry) => entry.run_without_asking = true,
            None => access.allowed.push(MonidAllowedTool {
                tool,
                run_without_asking: true,
            }),
        }
    } else {
        access.allowed.retain(|a| !same(&a.tool));
        if !access.blocked.iter().any(same) {
            access.blocked.push(tool);
        }
    }
}

impl Hub {
    /// The channel's Monid access now, or an error when it has no Monid.
    async fn monid_access(
        &self,
        channel_id: ChannelId,
        task_id: Option<TaskId>,
    ) -> Result<MonidChannelAccess> {
        let access = self
            .store
            .read(move |conn| {
                let channel = conversation::channel_or_404(conn, channel_id)?;
                store::effective_access(conn, &channel, task_id)
            })
            .await?;
        access.ok_or_else(|| {
            HubError::new(
                ErrorCode::MonidNotConfigured,
                "This channel does not have this Monid tool. Call request_access to ask the user for it.",
            )
        })
    }

    pub(crate) async fn tool_search_tools(
        &self,
        ctx: &ToolCtx,
        args: SearchToolsArgs,
    ) -> Result<Value> {
        let query = args.query.trim().to_owned();
        if query.is_empty() {
            return Err(HubError::invalid(
                "The query is empty. Say in a few words what the tool must do.",
            ));
        }
        let mut tools = self.search_connector_tools(ctx, &query).await?;
        let mut note = None;
        let task_id = ctx.task.as_ref().map(|t| t.id);
        let channel_id = ctx.channel.id;
        let monid = self
            .store
            .read(move |conn| {
                let channel = conversation::channel_or_404(conn, channel_id)?;
                Ok(store::key_present(conn)?.then_some((
                    store::effective_access(conn, &channel, task_id)?,
                    channel.monid,
                )))
            })
            .await?;
        if let Some((access, own)) = monid {
            let found = match self.connectors.monid_client() {
                Ok(client) => client.search(&query, SEARCH_TOOLS_MAX as u32).await,
                Err(err) => Err(err),
            };
            match found {
                Ok(found) => tools.extend(
                    found
                        .iter()
                        // A tool that the user blocked stays out: it cannot
                        // be requested.
                        .filter(|tool| {
                            !own.blocked
                                .iter()
                                .any(|b| b.covers(&tool.provider, &tool.endpoint))
                        })
                        .map(|tool| SearchToolRow {
                            source: "monid".to_owned(),
                            tool: tool.endpoint.clone(),
                            provider: tool.provider.clone(),
                            description: tool_line(tool),
                            price: Some(monid::quote(&tool.price, &json!({})).rate),
                            access: match &access {
                                Some(access) if !access.refuses(&tool.provider, &tool.endpoint) => {
                                    SearchToolAccess::Available
                                }
                                _ => SearchToolAccess::Request,
                            },
                        }),
                ),
                Err(err) => note = Some(err.to_error_body().message),
            }
        }
        tools.truncate(SEARCH_TOOLS_MAX);
        Ok(serde_json::to_value(SearchToolsResult { tools, note })?)
    }

    /// The tools of the channel's connections that match all words.
    async fn search_connector_tools(
        &self,
        ctx: &ToolCtx,
        query: &str,
    ) -> Result<Vec<SearchToolRow>> {
        let channel_id = ctx.channel.id;
        let task_id = ctx.task.as_ref().map(|t| t.id);
        let (usable, others) = self
            .store
            .read(move |conn| {
                let channel = conversation::channel_or_404(conn, channel_id)?;
                Ok((
                    con_store::effective_connections_for(conn, &channel, task_id)?,
                    con_store::requestable_connections(conn, &channel, task_id)?,
                ))
            })
            .await?;
        let lower = query.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();
        let mut rows = Vec::new();
        let connections = usable
            .iter()
            .map(|c| (c, SearchToolAccess::Available))
            .chain(others.iter().map(|c| (c, SearchToolAccess::Request)));
        for (connection, access) in connections.filter(|(c, _)| c.status.is_active()) {
            // A catalog that fails leaves this connection out. Monid can
            // still answer.
            let Ok(tools) = self.service_tools(&connection.service).await else {
                continue;
            };
            rows.extend(
                tools
                    .iter()
                    .filter(|tool| {
                        let haystack = format!("{} {}", tool.slug, tool.description).to_lowercase();
                        words.iter().all(|word| haystack.contains(word))
                    })
                    .map(|tool| SearchToolRow {
                        source: connection.handle.clone(),
                        tool: tool.slug.clone(),
                        provider: String::new(),
                        description: crate::util::truncate_chars(
                            tool.description.trim(),
                            SEARCH_LINE_MAX_CHARS,
                        ),
                        price: None,
                        access,
                    }),
            );
            if rows.len() >= SEARCH_CONNECTOR_ROWS_MAX {
                break;
            }
        }
        rows.truncate(SEARCH_CONNECTOR_ROWS_MAX);
        Ok(rows)
    }

    pub(crate) async fn tool_get_monid_tool(
        &self,
        ctx: &ToolCtx,
        args: GetMonidToolArgs,
    ) -> Result<Value> {
        let task_id = ctx.task.as_ref().map(|t| t.id);
        let access = self.monid_access(ctx.channel.id, task_id).await?;
        let (provider, endpoint) = (args.provider.trim(), args.endpoint.trim());
        if access.refuses(provider, endpoint) {
            return Err(HubError::new(ErrorCode::PolicyDenied, REFUSED_TEXT));
        }
        let tool = self
            .connectors
            .monid_client()?
            .inspect(provider, endpoint)
            .await?;
        Ok(serde_json::to_value(GetMonidToolResult {
            provider: provider.to_owned(),
            endpoint: endpoint.to_owned(),
            price: monid::quote(&tool.price, &json!({})).rate,
            description: if tool.description.trim().is_empty() {
                tool.summary
            } else {
                tool.description
            },
            input: tool.input,
        })?)
    }

    /// The unattended turn that this call is in, if any.
    async fn unattended_turn(&self, ctx: &ToolCtx) -> Result<Option<Unattended>> {
        if let Some(task) = &ctx.task {
            if task.created_by != TaskCreator::Routine {
                return Ok(None);
            }
            let card = task.card_message_id;
            let routine = self
                .store
                .read(move |conn| {
                    Ok(conversation::message(conn, card)?.and_then(|m| m.author_routine_id))
                })
                .await?;
            return Ok(Some(Unattended::Routine(routine)));
        }
        if self.turn_kind(ctx).await?.is_proactive() {
            return Ok(Some(Unattended::Proactive));
        }
        let (session_id, channel_id) = (ctx.session.id, ctx.channel.id);
        self.store
            .read(move |conn| {
                let row = coord_store::coordinator(conn, channel_id)?;
                if row.session_id != Some(session_id) || !row.turn_open {
                    return Ok(None);
                }
                if row.turn_events.iter().any(WakeEvent::user_caused) {
                    return Ok(None);
                }
                Ok(row.turn_events.iter().find_map(|event| match event {
                    WakeEvent::Routine { id, .. } => Some(Unattended::Routine(Some(*id))),
                    _ => None,
                }))
            })
            .await
    }

    /// Reads what the hub needs for the steps of ADR 0016.
    async fn gate_facts(
        &self,
        ctx: &ToolCtx,
        provider: &str,
        endpoint: &str,
        estimate: Option<i64>,
    ) -> Result<GateFacts> {
        let unattended = self.unattended_turn(ctx).await?;
        let scope = CallScope::of(ctx);
        let (provider, endpoint) = (provider.to_owned(), endpoint.to_owned());
        let now = self.monid_now();
        let channel_mode = ctx.channel.default_approval_mode;
        let task_mode = ctx.task.as_ref().map(|t| t.approval_mode);
        self.store
            .read(move |conn| {
                let channel = conversation::channel_or_404(conn, scope.channel_id)?;
                let settings = store::settings(conn)?;
                let (month, _) = month_bounds(now);
                let spent = Spent {
                    task: scope
                        .task_id
                        .map(|id| store::spent_in_task(conn, id))
                        .transpose()?,
                    channel_month: store::spent_in_channel_since(conn, scope.channel_id, month)?,
                    hub_month: store::spent_since(conn, month)?,
                };
                let setting = match unattended {
                    None => None,
                    Some(Unattended::Proactive) => Some(settings.proactive),
                    Some(Unattended::Routine(id)) => {
                        let own = match id {
                            Some(id) => routines::routine(conn, id)?.and_then(|r| r.monid),
                            None => None,
                        };
                        Some(own.unwrap_or(settings.routines))
                    }
                };
                // A proactive turn never runs in Full access (DECISIONS 811).
                let full_access = unattended != Some(Unattended::Proactive)
                    && task_mode.unwrap_or(channel_mode) == ApprovalMode::FullAccess;
                Ok(GateFacts {
                    granted: store::has_grant(conn, &scope.grant_key(), &provider, &endpoint)?,
                    over_limit: over_limit_note(&settings.limits, estimate, spent),
                    access: store::effective_access(conn, &channel, scope.task_id)?
                        .unwrap_or(channel.monid),
                    settings,
                    unattended: setting,
                    full_access,
                })
            })
            .await
    }

    fn monid_now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_millis(self.now().as_millis())
            .unwrap_or_else(chrono::Utc::now)
    }

    /// Writes the record and the row of a call that does not run.
    async fn write_not_run(
        &self,
        pending: &PendingCall,
        state: MonidCallState,
        message: &str,
    ) -> Result<MonidCallId> {
        let mut call = self.new_call(pending, state);
        call.error = Some(message.to_owned());
        let id = call.id;
        self.store
            .write(move |tx| insert_call(tx, &mut call))
            .await?;
        Ok(id)
    }

    fn new_call(&self, pending: &PendingCall, state: MonidCallState) -> MonidCall {
        MonidCall {
            id: MonidCallId::new(),
            channel_id: pending.scope.channel_id,
            session_id: pending.scope.session_id,
            task_id: pending.scope.task_id,
            message_id: None,
            approval_id: pending.approval_id,
            provider: pending.provider.clone(),
            endpoint: pending.endpoint.clone(),
            name: pending.name.clone(),
            state,
            rate: pending.rate.clone(),
            cost_micros: 0,
            run_id: None,
            stoppable: false,
            input: pending.input.clone(),
            output: Value::Null,
            output_blob: None,
            error: None,
            files: Vec::new(),
            duration_ms: 0,
            at: self.now(),
        }
    }

    pub(crate) async fn tool_run_monid_tool(
        &self,
        ctx: &ToolCtx,
        args: RunMonidToolArgs,
    ) -> Result<Value> {
        let started = Instant::now();
        let provider = args.provider.trim().to_owned();
        let endpoint = args.endpoint.trim().to_owned();
        if provider.is_empty() || endpoint.is_empty() {
            return Err(HubError::invalid(
                "Name the provider and the endpoint from search_tools.",
            ));
        }
        for part in [&args.body, &args.query_params, &args.path_params] {
            if !(part.is_null() || part.is_object()) {
                return Err(HubError::invalid(
                    "body, query_params and path_params must be objects, as get_monid_tool describes them.",
                ));
            }
        }
        if self.in_research_turn(ctx).await? {
            return Err(crate::proactivity::turn::denied(
                crate::proactivity::turn::RESEARCH_REFUSAL,
            ));
        }
        self.monid_access(ctx.channel.id, ctx.task.as_ref().map(|t| t.id))
            .await?;
        let input = run_input(args.body, args.query_params, args.path_params);
        let mut pending = PendingCall {
            scope: CallScope::of(ctx),
            name: format!("{provider} {endpoint}"),
            provider,
            endpoint,
            rate: String::new(),
            input,
            approval_id: None,
        };

        // A refusal needs no price, so it asks Monid nothing.
        let facts = self
            .gate_facts(ctx, &pending.provider, &pending.endpoint, Some(0))
            .await?;
        if let Gate::Refuse(message) = decide(&facts, &pending.provider, &pending.endpoint) {
            let call_id = self
                .write_not_run(&pending, MonidCallState::Blocked, &message)
                .await?;
            let user_blocked = facts
                .access
                .blocked
                .iter()
                .any(|b| b.covers(&pending.provider, &pending.endpoint));
            let message = if user_blocked || facts.unattended == Some(MonidUnattended::Off) {
                message
            } else {
                format!("{message} Call request_access to ask the user for it.")
            };
            return Ok(serde_json::to_value(RunMonidToolResult::Denied {
                call_id,
                message,
            })?);
        }

        let tool = self
            .connectors
            .monid_client()?
            .inspect(&pending.provider, &pending.endpoint)
            .await?;
        let quote = monid::quote(&tool.price, &pending.input);
        pending.name = tool_name(&tool);
        pending.rate.clone_from(&quote.rate);
        let facts = self
            .gate_facts(
                ctx,
                &pending.provider,
                &pending.endpoint,
                quote.estimate_micros,
            )
            .await?;
        let budget = self.connectors.timings().approval_wait;
        let (rule, over_limit) = match decide(&facts, &pending.provider, &pending.endpoint) {
            Gate::Refuse(message) => {
                let call_id = self
                    .write_not_run(&pending, MonidCallState::Blocked, &message)
                    .await?;
                return Ok(serde_json::to_value(RunMonidToolResult::Denied {
                    call_id,
                    message,
                })?);
            }
            Gate::Run => {
                let wait = budget.saturating_sub(started.elapsed());
                let result = self.execute_monid_call(&pending, Some(wait)).await?;
                return Ok(serde_json::to_value(result)?);
            }
            Gate::Ask { rule, over_limit } => (rule, over_limit),
        };

        // Ask the user with a card, and wait for the answer.
        let approval_id = ApprovalId::new();
        pending.approval_id = Some(approval_id);
        let mut answer = self.connectors.wait_for(approval_id);
        let card = MonidCard {
            provider: pending.provider.clone(),
            endpoint: pending.endpoint.clone(),
            name: pending.name.clone(),
            rate: pending.rate.clone(),
            estimate_micros: quote.estimate_micros,
            input: pending.input.clone(),
            over_limit: over_limit.map(|text| MonidLimitNote { text }),
            ask_rule: rule,
        };
        let summary = format!("Run a paid tool: {}", pending.name);
        if let Err(err) = self
            .post_hub_card(
                ctx,
                approval_id,
                summary,
                ApprovalDetails::Monid(Box::new(card)),
            )
            .await
        {
            self.connectors.take_waiter(approval_id);
            return Err(err);
        }
        let status =
            match tokio::time::timeout(budget.saturating_sub(started.elapsed()), &mut answer).await
            {
                Ok(Ok(status)) => Some(status),
                Ok(Err(_)) => None,
                Err(_) => {
                    if self.connectors.take_waiter(approval_id).is_some() {
                        // Nobody answered in time. A later answer runs the call.
                        None
                    } else {
                        // The answer came at the same moment and took the waiter.
                        answer.await.ok()
                    }
                }
            };
        let Some(status) = status else {
            return Ok(serde_json::to_value(RunMonidToolResult::WaitingForUser {
                approval_id,
                message: "The user has not answered the approval card yet. Go on with other work. When the user answers, the hub runs the call and sends the result.".to_owned(),
            })?);
        };
        let approval = self
            .store
            .read(move |conn| work::approval(conn, approval_id))
            .await?
            .ok_or_else(|| HubError::internal("a Monid card is gone"))?;
        let wait = budget.saturating_sub(started.elapsed());
        let result = self
            .answered_card(&pending, &approval, status, rule, Some(wait))
            .await?;
        Ok(serde_json::to_value(result)?)
    }

    /// Carries out the answer to a card: the choice in its more menu, then
    /// the call.
    async fn answered_card(
        &self,
        pending: &PendingCall,
        approval: &Approval,
        status: ApprovalStatus,
        rule: Option<ApprovalRule>,
        wait: Option<Duration>,
    ) -> Result<RunMonidToolResult> {
        let choice = approval.answer_option_id.clone().unwrap_or_default();
        let tool = MonidToolRef {
            provider: pending.provider.clone(),
            endpoint: Some(pending.endpoint.clone()),
            name: pending.name.clone(),
        };
        if choice == MONID_CARD_ALWAYS || choice == MONID_CARD_NEVER {
            let channel_id = pending.scope.channel_id;
            let always = choice == MONID_CARD_ALWAYS;
            self.store
                .write(move |tx| {
                    let mut channel = conversation::channel_or_404(tx.db(), channel_id)?;
                    add_card_choice(&mut channel.monid, tool, always);
                    channel.updated_at = tx.now();
                    tx.save_channel(&channel)
                })
                .await?;
        }
        if status != ApprovalStatus::Allowed {
            let (state, message) = if status != ApprovalStatus::Denied {
                (
                    MonidCallState::Denied,
                    "The approval card closed without an answer, so the call did not run.",
                )
            } else if choice == MONID_CARD_NEVER {
                (
                    MonidCallState::Blocked,
                    "The user blocked this tool in this channel. Do not use it here. Find another way, or tell the user.",
                )
            } else {
                (
                    MonidCallState::Denied,
                    "The user did not allow this call. Do not try it again unless the user asks.",
                )
            };
            let call_id = self.write_not_run(pending, state, message).await?;
            return Ok(RunMonidToolResult::Denied {
                call_id,
                message: message.to_owned(),
            });
        }
        if choice == MONID_CARD_ALLOW_TASK || rule == Some(ApprovalRule::AskOncePerTask) {
            let key = pending.scope.grant_key();
            let (provider, endpoint) = (pending.provider.clone(), pending.endpoint.clone());
            self.store
                .write(move |tx| tx.save_monid_grant(&key, &provider, &endpoint))
                .await?;
        }
        self.execute_monid_call(pending, wait).await
    }

    /// Starts the run once and follows it. With `wait`, the call returns
    /// `still_running` after that time and the session gets the result in a
    /// later turn. With `None`, the call waits for the end.
    async fn execute_monid_call(
        &self,
        pending: &PendingCall,
        wait: Option<Duration>,
    ) -> Result<RunMonidToolResult> {
        let client = self.connectors.monid_client()?;
        let mut call = self.new_call(pending, MonidCallState::Running);
        let saved = {
            let mut draft = call.clone();
            self.store
                .write(move |tx| {
                    insert_call(tx, &mut draft)?;
                    Ok(draft)
                })
                .await?
        };
        call = saved;
        let started = Instant::now();
        let run = match client
            .start_run(&call.provider, &call.endpoint, &call.input)
            .await
        {
            Ok(run) => run,
            Err(err) => {
                let mut message = err.to_error_body().message;
                call.state = if monid::is_low_balance(&err) {
                    MonidCallState::StoppedByMonid
                } else {
                    MonidCallState::Failed
                };
                // A lost answer does not show that nothing ran.
                if call.state == MonidCallState::Failed && err.code() == ErrorCode::MonidError {
                    message.push_str(
                        " The run may have started. Check app.monid.ai before you ask again.",
                    );
                }
                call.error = Some(message);
                call.duration_ms = elapsed_ms(started);
                let saved = call.clone();
                self.store.write(move |tx| write_call(tx, &saved)).await?;
                return Ok(result_of_call(&call));
            }
        };
        call.run_id = Some(run.run_id.clone());
        call.stoppable = run.stoppable;
        let saved = call.clone();
        self.store.write(move |tx| write_call(tx, &saved)).await?;

        let (reply, mut result) = oneshot::channel();
        let hub = self.clone();
        let followed = call.clone();
        tokio::spawn(async move {
            hub.follow_run(client, followed, started, Some(reply)).await;
        });
        let still_running = || RunMonidToolResult::StillRunning {
            call_id: call.id,
            message: format!(
                "{} takes long. Go on with other work. The hub follows the run and sends the result when it ends.",
                call.name
            ),
        };
        let Some(wait) = wait else {
            return Ok(result.await.unwrap_or_else(|_| still_running()));
        };
        Ok(match tokio::time::timeout(wait, &mut result).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => still_running(),
            Err(_) => {
                // From here a late end goes to the session as a wake event.
                result.close();
                result.try_recv().unwrap_or_else(|_| still_running())
            }
        })
    }

    /// Polls a run until it ends, then saves the call. `reply` gets the
    /// result. When nothing waits for it any more, the result goes to the
    /// session in a later turn.
    async fn follow_run(
        &self,
        client: Client,
        mut call: MonidCall,
        started: Instant,
        reply: Option<oneshot::Sender<RunMonidToolResult>>,
    ) {
        let Some(run_id) = call.run_id.clone() else {
            return;
        };
        let interval = self.connectors.timings().poll_interval;
        let mut failures = 0;
        let run = loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                () = tokio::time::sleep(interval) => {}
            }
            if started.elapsed() > FOLLOW_MAX {
                break None;
            }
            match client.run(&run_id).await {
                Ok(run) if run.status().is_final() => break Some(run),
                Ok(run) => {
                    failures = 0;
                    if run.stoppable != call.stoppable {
                        call.stoppable = run.stoppable;
                        let saved = call.clone();
                        if let Err(err) = self.store.write(move |tx| write_call(tx, &saved)).await {
                            tracing::warn!(error = %err, call_id = %call.id, "could not save a Monid call");
                        }
                    }
                }
                Err(err) => {
                    failures += 1;
                    if failures >= FOLLOW_FAILURES_MAX || monid::is_unauthorized(&err) {
                        tracing::warn!(error = %err, call_id = %call.id, "stopped following a Monid run");
                        break None;
                    }
                }
            }
        };
        call.duration_ms = elapsed_ms(started);
        call.stoppable = false;
        match run {
            Some(run) => self.apply_run(&mut call, &run).await,
            None => {
                call.state = MonidCallState::Failed;
                call.error = Some(format!(
                    "The hub lost track of run {run_id}. It may have ended and cost money. Check it at app.monid.ai."
                ));
            }
        }
        let saved = call.clone();
        let finished = self
            .store
            .write(move |tx| {
                write_call(tx, &saved)?;
                if let Some(id) = saved.task_id
                    && let Some(mut task) = work::task(tx.db(), id)?
                {
                    task.monid_calls = task.monid_calls.saturating_add(1);
                    task.monid_cost_micros =
                        task.monid_cost_micros.saturating_add(saved.cost_micros);
                    tx.save_task(&task)?;
                }
                Ok(())
            })
            .await;
        if let Err(err) = finished {
            tracing::warn!(error = %err, call_id = %call.id, "could not save a Monid call");
        }
        let result = result_of_call(&call);
        let sent = match reply {
            Some(reply) => reply.send(result).is_ok(),
            None => false,
        };
        if !sent && let Err(err) = self.send_late_result(&call).await {
            tracing::warn!(error = %err, call_id = %call.id, "could not report a Monid result");
        }
    }

    /// Fills in a call from its final run: the cost, the output and the
    /// files.
    async fn apply_run(&self, call: &mut MonidCall, run: &Run) {
        call.state = state_of_run(run);
        call.cost_micros = run.cost_micros();
        call.error = error_of_run(run, call.state);
        let (output, whole) = cut_output(&run.output);
        call.output = output;
        if let Some(whole) = whole {
            let size = whole.len() as u64;
            match self.put_blob_bytes(whole.into_bytes()).await {
                Ok(sha256) => {
                    call.output_blob = Some(BlobRef {
                        sha256,
                        name: format!("monid-output-{}.json", call.id),
                        mime: "application/json".to_owned(),
                        size,
                    });
                }
                Err(err) => {
                    tracing::warn!(error = %err, call_id = %call.id, "could not save a Monid output");
                }
            }
        }
        if call.state != MonidCallState::Completed {
            return;
        }
        let allow_private = self.connectors.monid_allow_private();
        for file in monid::output_files(&run.output) {
            match monid::download_file(&file.url, allow_private).await {
                Ok(download) => {
                    if let Err(err) = self.save_run_file(call, &file.name, download).await {
                        tracing::warn!(error = %err, call_id = %call.id, "could not save a Monid file");
                        call.error = Some(format!(
                            "The hub could not save {} in the channel's files.",
                            file.name
                        ));
                    }
                }
                Err(err) => {
                    // The run stays a success: the address is in the output.
                    call.error = Some(err.to_error_body().message);
                }
            }
        }
    }

    async fn save_run_file(
        &self,
        call: &mut MonidCall,
        name: &str,
        download: monid::Downloaded,
    ) -> Result<()> {
        let size = download.bytes.len() as u64;
        let sha256 = self.put_blob_bytes(download.bytes).await?;
        let file = BlobRef {
            sha256,
            name: name.to_owned(),
            mime: download.mime,
            size,
        };
        let row = ChannelFile {
            channel_id: call.channel_id,
            path: file_path(call, name),
            blob_sha256: file.sha256.clone(),
            size,
            mime: file.mime.clone(),
            task_id: None,
            title: None,
            updated_by: call.session_id.to_string(),
            updated_at: self.now(),
            deleted_at: None,
        };
        self.store
            .write(move |tx| tx.save_channel_file(&row))
            .await?;
        call.files.push(file);
        Ok(())
    }

    /// Gives a call that ended late to its session: a wake event for a
    /// coordinator, a message for a task.
    async fn send_late_result(&self, call: &MonidCall) -> Result<()> {
        let event = WakeEvent::MonidResult {
            id: call.id,
            text: cut_text(&late_text(call), LATE_RESULT_MAX_BYTES * 2),
        };
        let (channel_id, task_id) = (call.channel_id, call.task_id);
        let hub = self.clone();
        let (delivered, kick) = self
            .store
            .write(move |tx| match task_id {
                Some(task_id) => {
                    let text = render_events(std::slice::from_ref(&event));
                    match deliver_in_tx(&hub, tx, task_id, &text, &[], Actor::System) {
                        Ok(delivered) => {
                            let changed = delivered.status_changed;
                            Ok((Some(delivered), changed))
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "could not give a Monid result to its task");
                            Ok((None, false))
                        }
                    }
                }
                None => {
                    tx.queue_wake(channel_id, &event)?;
                    Ok((None, true))
                }
            })
            .await?;
        if let Some(delivered) = delivered {
            self.after_delivery(&delivered);
        }
        if kick {
            self.kick_coordinator(channel_id);
        }
        Ok(())
    }

    /// The user answered a card after the call stopped waiting. Allow runs
    /// the call now. Returns the text for the session.
    pub(crate) async fn late_monid_answer(&self, approval: &Approval) -> Result<String> {
        let ApprovalDetails::Monid(card) = &approval.details else {
            return Err(HubError::internal("not a Monid card"));
        };
        let pending = PendingCall {
            scope: CallScope {
                channel_id: approval.channel_id,
                session_id: approval.session_id,
                task_id: approval.task_id,
            },
            provider: card.provider.clone(),
            endpoint: card.endpoint.clone(),
            name: card.name.clone(),
            rate: card.rate.clone(),
            input: card.input.clone(),
            approval_id: Some(approval.id),
        };
        // The access may have changed while the card waited.
        if approval.status == ApprovalStatus::Allowed {
            let refused = match self
                .monid_access(approval.channel_id, approval.task_id)
                .await
            {
                Ok(access) => access.refuses(&card.provider, &card.endpoint),
                Err(_) => true,
            };
            if refused {
                self.write_not_run(&pending, MonidCallState::Blocked, REFUSED_TEXT)
                    .await?;
                return Ok(format!("{} did not run: {REFUSED_TEXT}", card.name));
            }
        }
        let result = self
            .answered_card(&pending, approval, approval.status, card.ask_rule, None)
            .await;
        Ok(match result {
            Ok(RunMonidToolResult::Ran {
                call_id,
                successful,
                cost,
                data,
                error,
                truncated,
                files,
            }) => {
                let text = data.to_string();
                let shown = crate::limits::json_prefix(&text, LATE_RESULT_MAX_BYTES);
                let cut = if truncated || shown.len() < text.len() {
                    " The result is cut."
                } else {
                    ""
                };
                let files = if files.is_empty() {
                    String::new()
                } else {
                    format!(" Files: {}.", files.join(", "))
                };
                if successful {
                    format!(
                        "The user allowed {}, and it ran (call {call_id}). It cost {cost}.{files}{cut} Result: {shown}",
                        card.name
                    )
                } else {
                    format!(
                        "The user allowed {}, but it gave no result (call {call_id}). It cost {cost}: {}",
                        card.name,
                        error.unwrap_or_else(|| "no reason given".to_owned())
                    )
                }
            }
            Ok(RunMonidToolResult::Denied { message, .. }) => format!("{}: {message}", card.name),
            Ok(RunMonidToolResult::StillRunning { message, .. }) => message,
            Ok(RunMonidToolResult::WaitingForUser { .. }) => String::new(),
            Err(err) => format!(
                "The user allowed {}, but it did not run: {}",
                card.name,
                err.to_error_body().message
            ),
        })
    }

    /// At hub start: the marker of the key follows the key file, and the
    /// hub follows the runs that were going when it stopped. It does not
    /// start any of them again.
    pub(crate) async fn resume_monid(&self) -> Result<()> {
        let present = self.connectors.monid_key().is_some();
        let running = self
            .store
            .write(move |tx| {
                tx.set_monid_key_present(present)?;
                store::running_calls(tx.db())
            })
            .await?;
        for mut call in running {
            match (call.run_id.is_some(), self.connectors.monid_client()) {
                (true, Ok(client)) => {
                    let hub = self.clone();
                    tokio::spawn(async move {
                        hub.follow_run(client, call, Instant::now(), None).await;
                    });
                }
                _ => {
                    call.state = MonidCallState::Failed;
                    call.stoppable = false;
                    call.error = Some(
                        "The hub stopped while this call started. The run may have started. Check app.monid.ai before you ask again."
                            .to_owned(),
                    );
                    let saved = call.clone();
                    self.store.write(move |tx| write_call(tx, &saved)).await?;
                    self.send_late_result(&call).await?;
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn monid_set_key(
        &self,
        _caller: &Caller,
        request: MonidSetKey,
    ) -> Result<MonidStatusResult> {
        let key = request.api_key.trim().to_owned();
        if key.is_empty() {
            return Err(HubError::invalid(
                "The key is empty. Paste the key from app.monid.ai.",
            ));
        }
        if key.chars().count() > KEY_MAX_CHARS
            || key.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(HubError::invalid(
                "This does not look like a Monid key. Copy the key again from app.monid.ai.",
            ));
        }
        let key = ApiKey::new(key);
        self.connectors
            .monid_client_for(key.clone())?
            .balance()
            .await?;
        self.connectors.save_monid_key(key).await?;
        let settings = request.settings;
        self.store
            .write(move |tx| {
                tx.set_monid_key_present(true)?;
                if let Some(settings) = settings {
                    check_limits(&settings.limits)?;
                    tx.save_monid_settings(&settings)?;
                }
                Ok(())
            })
            .await?;
        tracing::info!("the Monid key changed");
        self.monid_get_status(_caller, MonidGetStatus {}).await
    }

    pub(crate) async fn monid_remove_key(
        &self,
        caller: &Caller,
        _request: MonidRemoveKey,
    ) -> Result<MonidStatusResult> {
        self.connectors.remove_monid_key().await?;
        self.store
            .write(|tx| tx.set_monid_key_present(false))
            .await?;
        tracing::info!("the Monid key was removed");
        self.monid_get_status(caller, MonidGetStatus {}).await
    }

    pub(crate) async fn monid_get_status(
        &self,
        _caller: &Caller,
        _request: MonidGetStatus,
    ) -> Result<MonidStatusResult> {
        let key = self.connectors.monid_key();
        let (balance_micros, error) = match self.connectors.monid_client() {
            Ok(client) => match client.balance().await {
                Ok(balance) => (Some(balance.balance.micros()), None),
                Err(err) => (None, Some(err.to_error_body().message)),
            },
            Err(_) => (None, None),
        };
        let (month, _) = month_bounds(self.monid_now());
        let (settings, spent_month_micros, spend) = self
            .store
            .read(move |conn| {
                Ok((
                    store::settings(conn)?,
                    store::spent_since(conn, month)?,
                    store::spend_by_channel_since(conn, month)?,
                ))
            })
            .await?;
        Ok(MonidStatusResult {
            status: MonidStatus {
                key_present: key.is_some(),
                key_hint: key.map(|k| k.hint()),
                balance_micros,
                error,
                settings,
                spent_month_micros,
                spend,
            },
        })
    }

    pub(crate) async fn monid_update_settings(
        &self,
        caller: &Caller,
        request: MonidUpdateSettings,
    ) -> Result<MonidStatusResult> {
        check_limits(&request.settings.limits)?;
        self.store
            .write(move |tx| tx.save_monid_settings(&request.settings))
            .await?;
        self.monid_get_status(caller, MonidGetStatus {}).await
    }

    pub(crate) async fn monid_catalog_search(
        &self,
        _caller: &Caller,
        request: MonidCatalogSearch,
    ) -> Result<MonidCatalogSearchResult> {
        let query = request.query.trim();
        if query.is_empty() {
            return Ok(MonidCatalogSearchResult { tools: Vec::new() });
        }
        let found = self
            .connectors
            .monid_client()?
            .search(query, monid::SEARCH_LIMIT_MAX)
            .await?;
        Ok(MonidCatalogSearchResult {
            tools: found
                .iter()
                .map(|tool| MonidCatalogTool {
                    provider: tool.provider.clone(),
                    provider_name: tool.provider_name.clone(),
                    endpoint: tool.endpoint.clone(),
                    name: tool_name(tool),
                    rate: monid::quote(&tool.price, &json!({})).rate,
                })
                .collect(),
        })
    }

    pub(crate) async fn monid_call_get(
        &self,
        _caller: &Caller,
        request: MonidCallGet,
    ) -> Result<MonidCallResult> {
        let id = request.id;
        let call = self.store.read(move |conn| store::call(conn, id)).await?;
        call.map(|call| MonidCallResult { call })
            .ok_or_else(|| HubError::not_found("This Monid call does not exist."))
    }

    pub(crate) async fn monid_call_list(
        &self,
        _caller: &Caller,
        request: MonidCallList,
    ) -> Result<MonidCallListResult> {
        let limit = request
            .limit
            .unwrap_or(MONID_CALLS_PAGE_MAX)
            .clamp(1, MONID_CALLS_PAGE_MAX);
        let mut calls = self
            .store
            .read(move |conn| store::calls_in_channel(conn, request.channel_id, limit))
            .await?;
        for call in &mut calls {
            call.input = Value::Null;
            call.output = Value::Null;
        }
        Ok(MonidCallListResult { calls })
    }

    pub(crate) async fn monid_call_stop(
        &self,
        _caller: &Caller,
        request: MonidCallStop,
    ) -> Result<MonidCallResult> {
        let id = request.id;
        let call = self
            .store
            .read(move |conn| store::call(conn, id))
            .await?
            .ok_or_else(|| HubError::not_found("This Monid call does not exist."))?;
        let run_id = call
            .run_id
            .as_deref()
            .filter(|_| call.state == MonidCallState::Running)
            .ok_or_else(|| HubError::conflict("This call is not running now."))?;
        // The run ends when Monid says so. The hub keeps polling until then.
        self.connectors.monid_client()?.stop_run(run_id).await?;
        Ok(MonidCallResult { call })
    }

    pub(crate) async fn channel_set_monid(
        &self,
        _caller: &Caller,
        request: ChannelSetMonid,
    ) -> Result<ChannelResult> {
        let channel = self
            .store
            .write(move |tx| {
                let mut channel = conversation::channel_or_404(tx.db(), request.channel_id)?;
                channel.monid = request.access;
                channel.updated_at = tx.now();
                tx.save_channel(&channel)?;
                Ok(channel)
            })
            .await?;
        Ok(ChannelResult { channel })
    }
}

fn check_limits(limits: &MonidLimits) -> Result<()> {
    let all = [
        limits.per_call_micros,
        limits.per_task_micros,
        limits.per_channel_month_micros,
        limits.per_hub_month_micros,
    ];
    if all.iter().flatten().any(|limit| *limit < 0) {
        return Err(HubError::invalid(
            "A limit cannot be below zero. Clear it to have no limit.",
        ));
    }
    Ok(())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "monid_tests.rs"]
pub(super) mod tests;
