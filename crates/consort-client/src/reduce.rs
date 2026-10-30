//! The reducers: they turn the cache into view state. The rules that the
//! mock shares (sidebar glyphs, Tasks panel groups) live in [`crate::derive`].

use std::collections::BTreeMap;

use consort_proto::{
    ids::{MessageId, TaskId},
    model::{
        self, ApprovalDetails, ApprovalStatus, AuthState as ProtoAuth, Author, ChannelKind as PK,
        DecisionStatus, MessageKind, RequestOptionKind, ToolId, ToolState as ToolStateProto,
        WorkerState,
    },
    status::TaskStatus as PS,
};

use crate::{
    cache::{Cache, ItemSummary},
    derive::{refresh, waiting_detail},
    format,
    view::*,
};

pub fn task_status(status: PS) -> TaskStatus {
    match status {
        PS::Scheduled => TaskStatus::Scheduled,
        PS::UpNext => TaskStatus::UpNext,
        PS::Working => TaskStatus::Working,
        PS::NeedsInput => TaskStatus::NeedsInput,
        PS::Review => TaskStatus::Review,
        PS::Failed => TaskStatus::Failed,
        PS::Resolved => TaskStatus::Resolved,
    }
}

fn isolation(i: model::Isolation) -> Isolation {
    match i {
        model::Isolation::InPlace => Isolation::InPlace,
        model::Isolation::Worktree => Isolation::Worktree,
        model::Isolation::Clone => Isolation::Clone,
    }
}

fn os(os: model::Os) -> Os {
    match os {
        model::Os::Macos => Os::MacOs,
        model::Os::Windows => Os::Windows,
        model::Os::Linux => Os::Linux,
    }
}

fn approval_mode(mode: model::ApprovalMode) -> ApprovalMode {
    match mode {
        model::ApprovalMode::Ask => ApprovalMode::Ask,
        model::ApprovalMode::AutoEdits => ApprovalMode::AutoEdits,
        model::ApprovalMode::Auto => ApprovalMode::Auto,
        model::ApprovalMode::FullAccess => ApprovalMode::FullAccess,
    }
}

/// The name of a model, from the inventory of any worker with an account of
/// this label. The ID when no worker reports it.
fn model_name(cache: &Cache, account_label: &str, model_id: &str) -> String {
    cache
        .workers
        .values()
        .filter_map(|w| w.inventory.as_ref())
        .flat_map(|i| i.accounts.iter())
        .filter(|a| a.label == account_label)
        .flat_map(|a| a.models.iter())
        .find(|m| m.id == model_id)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| model_id.to_owned())
}

/// A model selection as the view shows it, with the model ID and the
/// options kept for the hub.
pub fn selection_view(cache: &Cache, s: &model::ModelSelection) -> SelectionView {
    SelectionView {
        account_label: s.account_label.clone(),
        model_id: s.model.clone(),
        model_name: model_name(cache, &s.account_label, &s.model),
        options: s
            .options
            .iter()
            .map(|(id, value)| {
                (
                    id.clone(),
                    match value {
                        model::OptionValue::Bool(b) => SelectionOption::Bool(*b),
                        model::OptionValue::Text(t) => SelectionOption::Text(t.clone()),
                    },
                )
            })
            .collect(),
    }
}

/// A sign-in method as the view names it.
pub fn sign_in_choice(method: &model::SignInMethod) -> SignInChoice {
    match method {
        model::SignInMethod::ClaudePlan => SignInChoice::ClaudePlan,
        model::SignInMethod::ClaudeConsole => SignInChoice::ClaudeConsole,
        model::SignInMethod::ChatgptDevice => SignInChoice::ChatgptDevice,
        model::SignInMethod::ChatgptBrowser => SignInChoice::ChatgptBrowser,
        model::SignInMethod::ApiKey { variable } => SignInChoice::ApiKey {
            variable: variable.clone(),
        },
        model::SignInMethod::AcpAgent { method_id, name } => SignInChoice::AcpAgent {
            method_id: method_id.clone(),
            name: name.clone(),
        },
        model::SignInMethod::Terminal { command } => SignInChoice::Terminal {
            command: command.clone(),
        },
    }
}

/// The step of a sign-in as the view shows it.
pub fn sign_in_step(state: &model::SignInState) -> SignInStep {
    match state {
        model::SignInState::Starting => SignInStep::Starting,
        model::SignInState::OpenUrl { url } => SignInStep::OpenUrl { url: url.clone() },
        model::SignInState::DeviceCode { url, user_code } => SignInStep::DeviceCode {
            url: url.clone(),
            user_code: user_code.clone(),
        },
        model::SignInState::NeedsCode { url } => SignInStep::NeedsCode { url: url.clone() },
        model::SignInState::Verifying => SignInStep::Verifying,
        model::SignInState::Succeeded { detail } => SignInStep::Succeeded {
            detail: detail.clone(),
        },
        model::SignInState::Failed { message } => SignInStep::Failed {
            message: message.clone(),
        },
        model::SignInState::Cancelled => SignInStep::Cancelled,
        model::SignInState::Expired => SignInStep::Expired,
    }
}

/// The managed tool that runs a harness.
fn tool_of_harness(harness: &str) -> Option<String> {
    match harness {
        "claude" => Some(ToolId::CLAUDE_CODE.to_owned()),
        "codex" => Some(ToolId::CODEX.to_owned()),
        "opencode" => Some(ToolId::OPENCODE.to_owned()),
        "gemini" => Some(ToolId::GEMINI_CLI.to_owned()),
        other => other
            .strip_prefix("acp:")
            .filter(|id| !id.is_empty())
            .map(|id| ToolId::acp(id).to_string()),
    }
}

/// The logo of a provider, with the icon of a registry agent when the
/// client has it.
fn with_icon(logo: ProviderLogo, cache: &Cache) -> ProviderLogo {
    match logo {
        ProviderLogo::Registry { id, .. } => ProviderLogo::Registry {
            svg: cache.acp_icons.get(&id).cloned(),
            id,
        },
        other => other,
    }
}

/// The name of an ACP registry agent, when the client has the registry.
fn registry_name(cache: &Cache, id: &str) -> Option<String> {
    cache
        .acp_registry
        .iter()
        .find(|a| a.id == id)
        .map(|a| a.name.clone())
}

/// A plan as a sentence: "Claude Max" becomes "Claude Max plan."
fn plan_sentence(plan: &str) -> String {
    let plan = plan.trim();
    if plan.to_lowercase().ends_with("plan") || plan.ends_with('.') {
        sentence(plan)
    } else {
        format!("{plan} plan.")
    }
}

/// One account row: its status, the line under its name and its action.
fn account_view(
    cache: &Cache,
    worker: &model::Worker,
    tools: &[model::ToolStatus],
    a: &model::HarnessAccount,
) -> HarnessAccountView {
    let (mut provider, logo) = provider_of(&a.harness);
    if let ProviderLogo::Registry { id, .. } = &logo
        && let Some(name) = registry_name(cache, id)
    {
        provider = name;
    }
    let installing = tool_of_harness(&a.harness).is_some_and(|tool| {
        tools
            .iter()
            .any(|t| t.tool.as_str() == tool && t.state == ToolStateProto::Installing)
    });
    // The row is in Settings > Workers already, so it does not tell the user
    // to go there.
    let auth_detail = a
        .auth_detail
        .as_deref()
        .map(|d| strip_settings_step(d.trim()))
        .filter(|d| !d.is_empty());
    let has_key = !a.secret_names.is_empty();
    let (status, text) = if installing {
        (AccountStatus::Installing, format!("Installing {provider}."))
    } else {
        match a.auth_state {
            ProtoAuth::SignedIn if has_key && a.plan.is_none() && a.email.is_none() => (
                AccountStatus::Ready,
                format!("API key saved on {}.", worker.name),
            ),
            ProtoAuth::SignedIn => (
                AccountStatus::SignedIn,
                match (&a.plan, &a.email, auth_detail) {
                    (Some(plan), _, _) => plan_sentence(plan),
                    (None, Some(email), _) => format!("Signed in as {email}."),
                    (None, None, Some(detail)) => sentence(detail),
                    (None, None, None) => {
                        account_detail(&provider, a.harness_version.as_deref(), None).0
                    }
                },
            ),
            ProtoAuth::SignedOut => (
                AccountStatus::NotSignedIn,
                match auth_detail {
                    Some(detail) => sentence(detail),
                    None if matches!(logo, ProviderLogo::Registry { .. }) => {
                        "ACP agent from the registry. Not signed in yet.".to_owned()
                    }
                    None => "Not signed in yet.".to_owned(),
                },
            ),
            ProtoAuth::Error => (
                if auth_detail.is_some_and(|d| d.to_lowercase().contains("expired")) {
                    AccountStatus::Expired
                } else {
                    AccountStatus::Error
                },
                match auth_detail {
                    Some(detail) => sentence(detail),
                    None => "The worker could not check the login. Sign in again.".to_owned(),
                },
            ),
            ProtoAuth::Unknown => (
                AccountStatus::Unknown,
                account_detail(&provider, a.harness_version.as_deref(), auth_detail).0,
            ),
        }
    };
    let (detail, detail_code) = account_detail_code(&text);
    let methods: Vec<SignInChoice> = a.sign_in_methods.iter().map(sign_in_choice).collect();
    let action = match status.action() {
        // Signing in needs a method from the worker. An account written by
        // hand in worker.toml may have none.
        AccountAction::SignIn | AccountAction::SignInAgain if methods.is_empty() => {
            AccountAction::None
        }
        action => action,
    };
    HarnessAccountView {
        id: a.account_id.to_string(),
        label: a.label.clone(),
        harness: a.harness.clone(),
        provider,
        logo: with_icon(logo, cache),
        detail,
        detail_code,
        auth: match a.auth_state {
            ProtoAuth::SignedIn => AuthState::SignedIn,
            ProtoAuth::SignedOut => AuthState::SignedOut,
            ProtoAuth::Error => AuthState::Error,
            ProtoAuth::Unknown => AuthState::Unknown,
        },
        status,
        action,
        sign_in_methods: methods,
        secret_names: a.secret_names.clone(),
        email: a.email.clone(),
        extras_enabled: a.extras_enabled,
        extras: a.extras.as_ref().map(|e| {
            let group = |g: &model::ExtrasGroup| crate::connectors::ExtrasGroupView {
                count: g.count,
                names: g.names.clone(),
            };
            crate::connectors::ProviderExtrasView {
                mcp_servers: group(&e.mcp_servers),
                plugins: group(&e.plugins),
                skills: group(&e.skills),
                cannot_turn_off: e.cannot_turn_off.clone(),
            }
        }),
    }
}

/// The rank of a tool in the Installed tools list: the providers first,
/// then the registry agents, then the tools that only support them.
fn tool_rank(tool: &str) -> (u8, String) {
    let rank = match tool {
        ToolId::CLAUDE_CODE => 0,
        ToolId::CODEX => 1,
        ToolId::OPENCODE => 2,
        ToolId::GEMINI_CLI => 3,
        ToolId::NODE => 5,
        ToolId::CLAUDE_AGENT_ACP => 6,
        _ => 4,
    };
    (rank, tool.to_owned())
}

/// One row of the Installed tools list.
fn tool_view(cache: &Cache, worker_name: &str, t: &model::ToolStatus) -> ToolView {
    let id = t.tool.as_str();
    let (name, logo) = match id {
        ToolId::NODE => ("Node.js".to_owned(), ProviderLogo::Generic),
        ToolId::CLAUDE_AGENT_ACP => (
            "Claude Code ACP adapter".to_owned(),
            ProviderLogo::ClaudeCode,
        ),
        other => {
            let (name, logo) = provider_of(other);
            match t.tool.acp_registry_id() {
                Some(registry) => (
                    registry_name(cache, registry).unwrap_or(name),
                    ProviderLogo::Registry {
                        id: registry.to_owned(),
                        svg: None,
                    },
                ),
                None => (name, logo),
            }
        }
    };
    let state = match t.state {
        ToolStateProto::Missing => ToolState::Missing,
        ToolStateProto::Installing => ToolState::Installing,
        ToolStateProto::Ready => ToolState::Ready,
        ToolStateProto::UpdateAvailable => ToolState::UpdateAvailable,
        ToolStateProto::Failed => ToolState::Failed,
    };
    let given = t
        .detail
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(sentence);
    let detail = match state {
        ToolState::Missing => Some(given.unwrap_or_else(|| {
            format!("Not installed. {worker_name} installs it when an account needs it.")
        })),
        ToolState::Failed => Some(given.unwrap_or_else(|| {
            "The last install failed. Try again, or check the worker log.".to_owned()
        })),
        _ => given,
    };
    ToolView {
        id: id.to_owned(),
        name,
        logo: with_icon(logo, cache),
        version: t.version.clone(),
        latest: t.latest.clone(),
        state,
        detail,
    }
}

/// The choices of an approval, in the order the card shows them.
pub fn approval_choice(kind: RequestOptionKind) -> Option<ApprovalChoice> {
    match kind {
        RequestOptionKind::AllowOnce => Some(ApprovalChoice::AllowOnce),
        RequestOptionKind::AllowAlways => Some(ApprovalChoice::AllowForTask),
        RequestOptionKind::DenyOnce | RequestOptionKind::DenyAlways => Some(ApprovalChoice::Deny),
        RequestOptionKind::Choice => None,
    }
}

/// A tool name without the `mcp__<server>__` prefix that harnesses add, and
/// the server named in that prefix.
fn split_tool_name(name: &str) -> (Option<&str>, &str) {
    match name
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
    {
        Some((server, tool)) => (Some(server), tool),
        None => (None, name),
    }
}

/// The line of an approval card whose approval row is gone (answered or
/// canceled before this client loaded it): made from the message body.
fn closed_summary(body: &str, in_task: bool) -> String {
    let body = body.trim();
    if body.starts_with("mcp__") {
        let (server, tool) = split_tool_name(body);
        let who = if in_task { "The task" } else { "Consort" };
        return match server {
            Some(server) => format!("{who} wanted to use {tool} from {server}"),
            None => format!("{who} wanted to use {tool}"),
        };
    }
    if body.is_empty() {
        "A request for your approval".to_owned()
    } else {
        body.to_owned()
    }
}

fn closed_approval(m: &model::Message) -> ApprovalView {
    ApprovalView {
        id: format!("closed-{}", m.id),
        task_id: m.author_task_id.map(|t| t.to_string()),
        summary: closed_summary(&m.body, m.author_task_id.is_some()),
        folder: None,
        detail: None,
        choices: Vec::new(),
        state: RequestState::Closed,
        answer: None,
        allow_always_label: None,
        card: None,
        button_labels: Vec::new(),
    }
}

fn closed_decision(m: &model::Message) -> DecisionView {
    DecisionView {
        id: format!("closed-{}", m.id),
        task_id: m.author_task_id.map(|t| t.to_string()),
        question: m.body.clone(),
        context: None,
        options: Vec::new(),
        recommended: None,
        allow_free_text: false,
        state: RequestState::Closed,
        chosen: None,
        answer_text: None,
    }
}

/// A word that names a file by its full path.
fn is_path(word: &str) -> bool {
    word.len() > 1
        && (word.starts_with('/')
            || word.starts_with('~')
            || word.get(1..3) == Some(":\\")
            || word.contains('\\'))
}

/// The last part of a path.
fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

/// A file change line with each full path cut to its file name, such as
/// "Write hello.txt". The detail block keeps the full paths.
fn file_change_summary(summary: &str, paths: &[String]) -> String {
    let words: Vec<&str> = summary.split_whitespace().collect();
    let mut out: Vec<String> = words
        .iter()
        .map(|w| {
            if is_path(w) {
                file_name(w).to_owned()
            } else {
                (*w).to_owned()
            }
        })
        .collect();
    if out.is_empty() {
        out = match paths {
            [one] => vec!["Change".to_owned(), file_name(one).to_owned()],
            many => vec![format!("Change {} files", many.len())],
        };
    }
    out.join(" ")
}

/// The line on an approval card. A tool approval reads "Consort wants to
/// use post_message from hub", never a raw `mcp__hub__post_message`.
fn approval_summary(a: &model::Approval) -> String {
    if let ApprovalDetails::FileChange { paths } = &a.details {
        return file_change_summary(&a.summary, paths);
    }
    let ApprovalDetails::Tool { tool, server, .. } = &a.details else {
        return a.summary.clone();
    };
    let raw = a.summary.trim();
    let readable = !raw.is_empty() && !raw.starts_with("mcp__") && raw != tool;
    if readable {
        return a.summary.clone();
    }
    let (prefix_server, name) = split_tool_name(tool);
    let who = if a.task_id.is_some() {
        "The task"
    } else {
        "Consort"
    };
    match server.as_deref().or(prefix_server) {
        Some(server) => format!("{who} wants to use {name} from {server}"),
        None => format!("{who} wants to use {name}"),
    }
}

/// The folder and the detail block of an approval. The detail block holds
/// the real action: the command, the files, the tool with its server and
/// arguments, or the network target. The summary line is text that the agent
/// wrote, so the user must be able to check it against the detail.
fn approval_detail(details: &ApprovalDetails) -> (Option<String>, Option<String>) {
    match details {
        ApprovalDetails::Command { command, cwd } => (cwd.clone(), Some(command.clone())),
        ApprovalDetails::FileChange { paths, .. } => (None, Some(paths.join("\n"))),
        ApprovalDetails::Tool {
            server,
            tool,
            arguments,
        } => {
            let (prefix_server, name) = split_tool_name(tool);
            let mut lines = vec![format!("Tool: {name}")];
            if let Some(server) = server.as_deref().or(prefix_server) {
                lines.push(format!("Server: {server}"));
            }
            let arguments = match arguments {
                serde_json::Value::Null => None,
                serde_json::Value::Object(map) if map.is_empty() => None,
                value => serde_json::to_string_pretty(value).ok(),
            };
            let mut text = lines.join("\n");
            if let Some(arguments) = arguments {
                text.push_str("\n\n");
                text.push_str(&arguments);
            }
            (None, Some(text))
        }
        ApprovalDetails::Network { target } => (None, Some(format!("Network: {target}"))),
        ApprovalDetails::Other { text } => (
            None,
            text.as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned),
        ),
        // Hub cards. The desktop draws each one from `ApprovalView::card`
        // (`consort-desktop/src/ui/hub_cards.rs`). This plain text of the
        // facts stays for a client that shows only the detail block.
        ApprovalDetails::Connector(card) => {
            let mut lines = vec![
                format!(
                    "Connector: {} ({})",
                    card.connection_name, card.service_name
                ),
                format!("Tool: {}", card.tool),
            ];
            if !card.fields.is_empty() {
                lines.push(String::new());
                lines.extend(
                    card.fields
                        .iter()
                        .map(|f| format!("{}: {}", f.label, f.value)),
                );
            }
            (None, Some(lines.join("\n")))
        }
        ApprovalDetails::Management(card) => {
            (None, Some(format!("{}\n\n{}", card.change, card.effect)))
        }
        ApprovalDetails::ConnectService(card) => (
            None,
            Some(format!(
                "Service: {}\nName: {}",
                card.service_name, card.name
            )),
        ),
        ApprovalDetails::SignIn(card) => (None, Some(format!("Account: {}", card.account_label))),
        ApprovalDetails::SkillInstall(card) => {
            let mut lines = vec![format!("Skill: {}", card.name)];
            lines.push(format!(
                "License: {}",
                card.license.as_deref().unwrap_or("not given")
            ));
            if !card.scripts.is_empty() {
                lines.push(format!("Scripts: {}", card.scripts.join(", ")));
            }
            if let Some(reason) = &card.blocked_reason {
                lines.push(format!("Blocked: {reason}"));
            }
            (None, Some(lines.join("\n")))
        }
    }
}

fn approval_view(cache: &Cache, a: &model::Approval) -> ApprovalView {
    let (folder, detail) = approval_detail(&a.details);
    let (card, button_labels) =
        match crate::extensions_view::hub_card(&a.details, &a.options, cache) {
            Some((card, labels)) => (Some(Box::new(card)), labels),
            None => (None, Vec::new()),
        };
    let mut choices: Vec<ApprovalChoice> = Vec::new();
    for option in &a.options {
        if let Some(choice) = approval_choice(option.kind)
            && !choices.contains(&choice)
        {
            choices.push(choice);
        }
    }
    if choices.is_empty() {
        choices = vec![ApprovalChoice::AllowOnce, ApprovalChoice::Deny];
    }
    choices.sort_by_key(|c| match c {
        ApprovalChoice::AllowOnce => 0,
        ApprovalChoice::AllowForTask => 1,
        ApprovalChoice::Deny => 2,
    });
    let answer = a
        .answer_option_id
        .as_deref()
        .and_then(|id| a.option(id))
        .and_then(|o| approval_choice(o.kind));
    // The harness's words for the wide choice. This is the option that
    // `option_for` sends, so the button says what it does.
    let allow_always_label = a
        .options
        .iter()
        .find(|o| o.kind == RequestOptionKind::AllowAlways)
        .map(|o| o.label.clone());
    ApprovalView {
        id: a.id.to_string(),
        task_id: a.task_id.map(|t| t.to_string()),
        summary: approval_summary(a),
        folder,
        detail,
        choices,
        state: match a.status {
            ApprovalStatus::Pending => RequestState::Pending,
            ApprovalStatus::Allowed | ApprovalStatus::Denied => RequestState::Answered,
            ApprovalStatus::Expired | ApprovalStatus::Canceled => RequestState::Expired,
        },
        answer,
        allow_always_label,
        card,
        button_labels,
    }
}

fn decision_view(d: &model::Decision) -> DecisionView {
    DecisionView {
        id: d.id.to_string(),
        task_id: d.task_id.map(|t| t.to_string()),
        question: d.question.clone(),
        context: d.context.clone(),
        options: d
            .options
            .iter()
            .map(|o| DecisionOption {
                label: o.label.clone(),
                consequence: o.consequence.clone(),
            })
            .collect(),
        recommended: d.recommended_index.map(u32::from),
        allow_free_text: d.allow_free_text,
        state: match d.status {
            DecisionStatus::Pending => RequestState::Pending,
            DecisionStatus::Answered => RequestState::Answered,
            DecisionStatus::Expired | DecisionStatus::Canceled => RequestState::Expired,
        },
        chosen: d
            .answer
            .as_ref()
            .and_then(|a| a.option_index)
            .map(u32::from),
        answer_text: d.answer.as_ref().and_then(|a| a.text.clone()),
    }
}

/// The kind and the text of a notice message. The hub writes the body as
/// the JSON `{"kind": "...", "text": "..."}` (DECISIONS row 66). A body that
/// does not have a text is shown as it is, with an empty kind, so no
/// notice is lost.
pub fn notice_parts(body: &str) -> (String, String) {
    #[derive(serde::Deserialize)]
    struct Parsed {
        #[serde(default)]
        kind: String,
        text: String,
    }
    match serde_json::from_str::<Parsed>(body) {
        Ok(parsed) if !parsed.text.trim().is_empty() => (parsed.kind, parsed.text),
        _ => (String::new(), body.to_owned()),
    }
}

/// The note of a `memory_saved` notice body (protocol.md, "The memory
/// saved line"). `None` when the body does not name a note.
pub fn memory_saved(body: &str) -> Option<crate::view::MemorySavedView> {
    #[derive(serde::Deserialize)]
    struct Parsed {
        scope: String,
        path: String,
        #[serde(default)]
        version: u64,
        #[serde(default)]
        created: bool,
        #[serde(default)]
        suggested_by_name: Option<String>,
    }
    let parsed: Parsed = serde_json::from_str(body).ok()?;
    if parsed.path.is_empty() {
        return None;
    }
    Some(crate::view::MemorySavedView {
        scope: parsed.scope,
        path: parsed.path,
        version: parsed.version,
        created: parsed.created,
        suggested_by: parsed.suggested_by_name.unwrap_or_default(),
    })
}

fn speaker(m: &model::Message) -> Speaker {
    match m.author {
        Author::User => Speaker::You,
        Author::Assistant if m.author_task_id.is_some() => Speaker::Task,
        Author::Assistant => Speaker::Consort,
        Author::System => Speaker::System,
    }
}

/// Builds the whole view state from the cache. `now` is Unix milliseconds.
/// The name of a channel as the user sees it. The main channel shows as
/// Tutti whatever the hub calls it: the hub and the wire still say
/// Tutti until the rename pass.
fn shown_name(c: &model::Channel) -> String {
    if c.kind == PK::Tutti {
        crate::view::TUTTI.to_owned()
    } else {
        c.name.clone()
    }
}

/// What the assistant of each channel does now. See [`crate::activity`].
fn channel_activities(cache: &Cache, state: &ViewState) -> Vec<crate::activity::ChannelActivity> {
    let sessions: Vec<crate::activity::SessionActivity> =
        cache.session_activity.values().cloned().collect();
    let task_sessions: Vec<(String, Option<String>)> = state
        .tasks
        .iter()
        .map(|t| {
            (
                t.id.clone(),
                cache
                    .tasks
                    .get(&t.id)
                    .and_then(|task| task.current_session_id)
                    .map(|s| s.to_string()),
            )
        })
        .collect();
    let tasks: Vec<crate::activity::TaskWork<'_>> = state
        .tasks
        .iter()
        .zip(task_sessions.iter())
        .map(|(t, (_, session))| crate::activity::TaskWork {
            channel_id: &t.channel_id,
            channel_name: &t.channel_name,
            status: t.status,
            session_id: session.as_deref(),
        })
        .collect();
    let name = |id: &str| state.channel(id).map(|c| c.name.clone());
    state
        .channels
        .iter()
        .map(|c| crate::activity::ChannelActivity {
            channel_id: c.id.clone(),
            activity: crate::activity::channel_activity(&crate::activity::Inputs {
                channel_id: &c.id,
                tutti: c.kind == ChannelKind::Tutti,
                tasks: &tasks,
                sessions: &sessions,
                timeline: state.timeline(&c.id),
                channel_name: &name,
            }),
        })
        .collect()
}

/// The artifact task of each blob in the channel files, by hash.
fn artifact_tasks(cache: &Cache) -> BTreeMap<String, crate::files::ArtifactTask> {
    cache
        .channel_files
        .values()
        .filter_map(|f| {
            let task_id = f.task_id?;
            let task = cache.tasks.get(&task_id.to_string());
            Some((
                f.blob_sha256.to_string(),
                crate::files::ArtifactTask {
                    task_id: task_id.to_string(),
                    task_name: task
                        .map(|t| t.title.clone())
                        .or_else(|| f.title.clone())
                        .unwrap_or_else(|| "A task".to_owned()),
                    status: task.map(|t| task_status(t.status)).unwrap_or_default(),
                },
            ))
        })
        .collect()
}

/// The views of the blobs that a message names.
fn attachment_views(
    blobs: &[model::BlobRef],
    artifacts: &BTreeMap<String, crate::files::ArtifactTask>,
    cache: &Cache,
) -> Vec<crate::files::AttachmentView> {
    blobs
        .iter()
        .map(|b| {
            let sha = b.sha256.to_string();
            crate::files::attachment_view(
                b,
                artifacts.get(&sha).cloned(),
                cache.local_blobs.get(&sha).cloned(),
            )
        })
        .collect()
}

/// The files of the loaded channels, for the Files tab.
/// The routines, by channel and then by name.
fn routine_views(cache: &Cache) -> Vec<crate::routines::RoutineView> {
    use crate::routines::{RoutineActionView, RoutineRunView, RoutineView};
    let mut out: Vec<RoutineView> = cache
        .routines
        .values()
        .map(|r| {
            let id = r.id.to_string();
            let runs = cache
                .routine_runs
                .get(&id)
                .map(|runs| {
                    runs.values()
                        .rev()
                        .map(|run| RoutineRunView {
                            id: run.id,
                            routine_id: id.clone(),
                            started_at: run.started_at.as_millis(),
                            ended_at: run.ended_at.map(|t| t.as_millis()),
                            status: run.status,
                            manual: run.manual,
                            message_id: run.message_id.map(|m| m.to_string()),
                            task_id: run.task_id.map(|t| t.to_string()),
                            error: run.error.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            RoutineView {
                runs_loaded: cache.runs_loaded.contains(&id),
                id,
                channel_id: r.channel_id.to_string(),
                name: r.name.clone(),
                enabled: r.enabled,
                schedule: r.schedule.clone(),
                prompt: r.prompt.clone(),
                action: match &r.action {
                    model::RoutineAction::CoordinatorTurn => RoutineActionView::Ask,
                    model::RoutineAction::StartTask {
                        title,
                        model_selection,
                        ..
                    } => RoutineActionView::Task {
                        title: title.clone(),
                        selection: model_selection.as_ref().map(|s| selection_view(cache, s)),
                    },
                },
                next_run_at: r.next_run_at.map(|t| t.as_millis()),
                last_run_at: r.last_run_at.map(|t| t.as_millis()),
                last_run_status: r.last_run_status,
                runs,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (&a.channel_id, a.name.to_lowercase(), &a.id).cmp(&(
            &b.channel_id,
            b.name.to_lowercase(),
            &b.id,
        ))
    });
    out
}

/// The title of a task or the name of a channel that made a memory
/// suggestion. Empty when the cache does not hold it.
fn suggestion_name(cache: &Cache, source: &consort_proto::turn_input::SuggestionSource) -> String {
    use consort_proto::turn_input::SuggestionSource;
    match source {
        SuggestionSource::Task(id) => cache
            .tasks
            .get(&id.to_string())
            .map(|t| t.title.clone())
            .unwrap_or_default(),
        SuggestionSource::Channel(id) => cache
            .channels
            .get(&id.to_string())
            .map(shown_name)
            .unwrap_or_default(),
    }
}

/// The memory notes in the cache, as views.
fn memory_note_views(cache: &Cache) -> Vec<crate::memory::MemoryNoteView> {
    cache
        .memory_notes
        .values()
        .map(|cached| memory_note_view(cache, &cached.note, cached.has_content))
        .collect()
}

/// One memory note as a view. `has_content` says whether `note.content` is
/// its text.
pub(crate) fn memory_note_view(
    cache: &Cache,
    note: &model::MemoryNote,
    has_content: bool,
) -> crate::memory::MemoryNoteView {
    crate::memory::MemoryNoteView {
        scope: note.scope.to_string(),
        path: note.path.clone(),
        content: has_content.then(|| note.content.clone()),
        sha256: note.sha256.to_string(),
        version: note.version,
        by_user: note.updated_by == model::MemoryAuthor::User,
        updated_at: note.updated_at.as_millis(),
        suggested_by: note
            .suggested_by
            .map(|source| (source.to_string(), suggestion_name(cache, &source))),
        deleted_at: note.deleted_at.map(|t| t.as_millis()),
    }
}

fn channel_file_views(cache: &Cache, now: i64) -> Vec<crate::files::ChannelFileView> {
    cache
        .channel_files
        .values()
        .map(|f| {
            let (folder, name) = crate::files::split_path(&f.path);
            let sha = f.blob_sha256.to_string();
            crate::files::ChannelFileView {
                channel_id: f.channel_id.to_string(),
                path: f.path.clone(),
                name: name.to_owned(),
                folder: folder.to_owned(),
                local_path: cache.local_blobs.get(&sha).cloned(),
                sha256: sha,
                size: f.size,
                mime: f.mime.clone(),
                image: crate::uploads::is_image(&f.mime),
                size_label: format::file_size_label(f.size),
                date_label: format::file_date_label(f.updated_at.as_millis(), now),
                updated_at: f.updated_at.as_millis(),
                task_id: f.task_id.map(|t| t.to_string()),
            }
        })
        .collect()
}

pub fn view_state(cache: &Cache, connection: Connection, user: UserView, now: i64) -> ViewState {
    let online = connection == Connection::Online;
    let artifacts = artifact_tasks(cache);
    let workers_by_id: BTreeMap<String, &model::Worker> =
        cache.workers.iter().map(|(k, v)| (k.clone(), v)).collect();
    let worker_name = |id: &consort_proto::ids::WorkerId| {
        workers_by_id
            .get(&id.to_string())
            .map(|w| w.name.clone())
            .unwrap_or_else(|| "a worker".to_owned())
    };
    let tutti = cache.channels.values().find(|c| c.kind == PK::Tutti);
    let fallback_selection = tutti.and_then(|c| c.coordinator_selection.clone());
    let tutti_selection = fallback_selection
        .as_ref()
        .map(|s| selection_view(cache, s));

    // Channels, Tutti first, then by group and position.
    let mut channels: Vec<&model::Channel> = cache
        .channels
        .values()
        .filter(|c| c.archived_at.is_none())
        .collect();
    channels.sort_by_key(|c| (c.kind != PK::Tutti, c.position, c.name.clone()));
    let channel_views: Vec<ChannelView> = channels
        .iter()
        .map(|c| {
            let selection = c
                .coordinator_selection
                .clone()
                .or(fallback_selection.clone());
            let account = selection.as_ref().map(|s| s.account_label.clone());
            let worker = c
                .preferred_workers
                .iter()
                .filter_map(|id| workers_by_id.get(&id.to_string()))
                .find(|w| w.state == WorkerState::Online)
                .or_else(|| {
                    workers_by_id.values().find(|w| {
                        w.state == WorkerState::Online
                            && account.as_ref().is_some_and(|label| {
                                w.inventory.as_ref().is_some_and(|inv| {
                                    inv.accounts.iter().any(|a| &a.label == label)
                                })
                            })
                    })
                })
                .map(|w| w.name.clone());
            let own = c
                .coordinator_selection
                .as_ref()
                .map(|s| selection_view(cache, s));
            ChannelView {
                id: c.id.to_string(),
                kind: if c.kind == PK::Tutti {
                    ChannelKind::Tutti
                } else {
                    ChannelKind::Standard
                },
                name: shown_name(c),
                purpose: c.purpose.clone(),
                instructions: c.instructions.clone(),
                icon: c.icon.clone().unwrap_or_else(|| {
                    if c.kind == PK::Tutti { "brace" } else { "hash" }.to_owned()
                }),
                group_id: c.group_id.map(|g| g.to_string()),
                coordinator_worker: worker,
                coordinator_selection: crate::picker::effective_selection(
                    own,
                    tutti_selection.clone(),
                ),
                repos: Vec::new(),
                preferred_workers: c.preferred_workers.iter().map(|w| w.to_string()).collect(),
                task_selection: c.task_selection.as_ref().map(|s| selection_view(cache, s)),
                default_isolation: isolation(c.default_isolation),
                tags: c.tags.clone(),
                disabled_connections: c
                    .disabled_connections
                    .iter()
                    .map(|id| id.to_string())
                    .collect(),
                disabled_skills: c.disabled_skills.iter().map(|id| id.to_string()).collect(),
            }
        })
        .collect();
    let mut archived: Vec<&model::Channel> = cache
        .channels
        .values()
        .filter(|c| c.archived_at.is_some() && c.kind != PK::Tutti)
        .collect();
    archived.sort_by_key(|c| (c.name.to_lowercase(), c.id.to_string()));
    let archived_channels: Vec<ArchivedChannelView> = archived
        .iter()
        .map(|c| ArchivedChannelView {
            id: c.id.to_string(),
            name: c.name.clone(),
            icon: c.icon.clone().unwrap_or_else(|| "hash".to_owned()),
        })
        .collect();
    let channel_name = |id: &consort_proto::ids::ChannelId| {
        cache
            .channels
            .get(&id.to_string())
            .map(shown_name)
            .unwrap_or_default()
    };

    let mut groups: Vec<&model::Group> = cache.groups.values().collect();
    groups.sort_by_key(|g| (g.position, g.name.clone()));

    // Tasks.
    let mut tasks: Vec<TaskView> = cache
        .tasks
        .values()
        .filter(|t| t.archived_at.is_none())
        .map(|t| {
            let status = task_status(t.status);
            let reason = match status {
                TaskStatus::Scheduled => t.start_at.map(|at| {
                    format!(
                        "Starts {} at {}",
                        format::day_label(at.as_millis(), now).to_lowercase_first(),
                        format::time_label(at.as_millis())
                    )
                }),
                _ => t.status_reason.clone(),
            };
            TaskView {
                id: t.id.to_string(),
                channel_id: t.channel_id.to_string(),
                channel_name: channel_name(&t.channel_id),
                title: t.title.clone(),
                status,
                meta: String::new(),
                worker_name: t.worker_id.as_ref().map(worker_name),
                account_label: t.model_selection.as_ref().map(|s| s.account_label.clone()),
                status_reason: reason,
                age_label: format::age_label(t.updated_at.as_millis(), now),
                updated_at: t.updated_at.as_millis(),
                card_message_id: t.card_message_id.to_string(),
                approval_mode: approval_mode(t.approval_mode),
            }
        })
        .collect();
    tasks.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
    let task_by_card: BTreeMap<String, &model::Task> = cache
        .tasks
        .values()
        .map(|t| (t.card_message_id.to_string(), t))
        .collect();

    // Indexes, so a timeline of many messages does not scan every approval,
    // decision and reply for each one.
    let approval_by_message: BTreeMap<MessageId, &model::Approval> = cache
        .approvals
        .values()
        .map(|a| (a.message_id, a))
        .collect();
    let decision_by_message: BTreeMap<MessageId, &model::Decision> = cache
        .decisions
        .values()
        .map(|d| (d.message_id, d))
        .collect();
    // The oldest pending approval of each task, for the inline card.
    let mut first_pending: BTreeMap<TaskId, &model::Approval> = BTreeMap::new();
    for a in cache
        .approvals
        .values()
        .filter(|a| a.status == ApprovalStatus::Pending)
    {
        if let Some(task) = a.task_id {
            let slot = first_pending.entry(task).or_insert(a);
            if a.created_at < slot.created_at {
                *slot = a;
            }
        }
    }
    let mut replies_by_root: BTreeMap<MessageId, Vec<&model::Message>> = BTreeMap::new();
    for m in cache.messages.values() {
        if let Some(root) = m.thread_root_id
            && m.deleted_at.is_none()
        {
            replies_by_root.entry(root).or_default().push(m);
        }
    }

    // Timelines.
    let mut by_channel: BTreeMap<String, Vec<&model::Message>> = BTreeMap::new();
    for m in cache.messages.values() {
        if m.deleted_at.is_none() && m.shows_in_timeline() {
            by_channel
                .entry(m.channel_id.to_string())
                .or_default()
                .push(m);
        }
    }
    let mut timelines = Vec::new();
    let mut unread: BTreeMap<String, u32> = BTreeMap::new();
    for channel in &channel_views {
        let mut messages = by_channel.remove(&channel.id).unwrap_or_default();
        messages.sort_by_key(|m| (m.created_at, m.id));
        // Where the read marker sits in time. A marker whose message is not
        // loaded (it is older than the newest page) sits before every
        // loaded message, so the count starts at the oldest one.
        let marker_at = cache
            .read_markers
            .get(&channel.id)
            .and_then(|m| cache.messages.get(&m.last_read_message_id.to_string()))
            .map(|m| (m.created_at, m.id));
        let mut count = 0u32;
        let mut items = Vec::new();
        let mut last_day = String::new();
        let mut push_day = |items: &mut Vec<TimelineItem>, at: i64| {
            let key = format::day_key(at);
            if key != last_day {
                items.push(TimelineItem::Day(DayView {
                    key: format!("day-{}-{key}", channel.id),
                    label: format::day_label(at, now),
                }));
                last_day = key;
            }
        };
        for m in &messages {
            let at = m.created_at.as_millis();
            let after_marker = marker_at.is_none_or(|at| (m.created_at, m.id) > at);
            // A system notice is not something to read: it never counts.
            if after_marker && m.author != Author::User && m.kind != MessageKind::Notice {
                count += 1;
            }
            push_day(&mut items, at);
            if m.kind == MessageKind::TaskCard
                && let Some(task) = task_by_card.get(&m.id.to_string())
            {
                let status = task_status(task.status);
                let approval = first_pending
                    .get(&task.id)
                    .copied()
                    .map(|a| approval_view(cache, a));
                let worker = task
                    .worker_id
                    .as_ref()
                    .and_then(|id| workers_by_id.get(&id.to_string()))
                    .map(|w| WorkerChip {
                        name: w.name.clone(),
                        online: w.state == WorkerState::Online,
                        account: task
                            .model_selection
                            .as_ref()
                            .map(|s| s.account_label.clone())
                            .unwrap_or_default(),
                    });
                let replies = m.reply_count;
                let summary = match replies {
                    0 => status.label().to_owned(),
                    1 => format!("{}, 1 reply", status.label()),
                    n => format!("{}, {n} replies", status.label()),
                };
                items.push(TimelineItem::TaskCard(TaskCardView {
                    message_id: m.id.to_string(),
                    task_id: task.id.to_string(),
                    time_label: format::time_label(at),
                    created_at: at,
                    title: task.title.clone(),
                    status,
                    channel_name: (task.channel_id.to_string() != channel.id)
                        .then(|| channel_name(&task.channel_id)),
                    worker,
                    approval,
                    summary,
                    reply_count: replies,
                    latest: match status {
                        TaskStatus::Review => task.review_summary.clone(),
                        TaskStatus::Failed | TaskStatus::UpNext => task.status_reason.clone(),
                        _ => None,
                    },
                    approval_mode: approval_mode(task.approval_mode),
                }));
            } else if m.kind == MessageKind::Approval {
                // Snapshots hold only pending approvals, so an answered one
                // may have just its message left.
                let approval = approval_by_message
                    .get(&m.id)
                    .copied()
                    .map(|a| approval_view(cache, a))
                    .unwrap_or_else(|| closed_approval(m));
                items.push(TimelineItem::Approval(ApprovalCardView {
                    message_id: m.id.to_string(),
                    speaker: speaker(m),
                    time_label: format::time_label(at),
                    created_at: at,
                    approval,
                }));
            } else if m.kind == MessageKind::Decision {
                let decision = decision_by_message
                    .get(&m.id)
                    .copied()
                    .map(decision_view)
                    .unwrap_or_else(|| closed_decision(m));
                items.push(TimelineItem::Decision(DecisionCardView {
                    message_id: m.id.to_string(),
                    speaker: speaker(m),
                    time_label: format::time_label(at),
                    created_at: at,
                    decision,
                }));
            } else if m.kind == MessageKind::Notice {
                let (kind, text) = notice_parts(&m.body);
                let memory = (kind == "memory_saved")
                    .then(|| memory_saved(&m.body))
                    .flatten();
                items.push(TimelineItem::Notice(NoticeView {
                    message_id: m.id.to_string(),
                    time_label: format::time_label(at),
                    created_at: at,
                    kind,
                    text,
                    memory,
                }));
            } else {
                items.push(TimelineItem::Message(MessageView {
                    id: m.id.to_string(),
                    channel_id: channel.id.clone(),
                    speaker: speaker(m),
                    time_label: format::time_label(at),
                    created_at: at,
                    title: None,
                    body: m.body.clone(),
                    items: Vec::new(),
                    edited: m.edited_at.is_some(),
                    reply_count: m.reply_count,
                    pending: false,
                    attachments: attachment_views(&m.attachments, &artifacts, cache),
                }));
            }
        }
        for p in cache
            .pending
            .iter()
            .filter(|p| p.channel_id == channel.id && p.thread_root_id.is_none())
        {
            push_day(&mut items, p.created_at);
            items.push(TimelineItem::Message(MessageView {
                id: format!("pending-{}", p.client_message_id),
                channel_id: channel.id.clone(),
                speaker: Speaker::You,
                time_label: format::time_label(p.created_at),
                created_at: p.created_at,
                body: p.body.clone(),
                pending: true,
                attachments: attachment_views(&p.attachments, &artifacts, cache),
                ..MessageView::default()
            }));
        }
        unread.insert(channel.id.clone(), count);
        timelines.push(TimelineView {
            channel_id: channel.id.clone(),
            items,
            has_older: cache.has_older.get(&channel.id).copied().unwrap_or(false),
            loaded: cache.channels_loaded.contains(&channel.id),
        });
    }

    // Task threads.
    let mut threads = Vec::new();
    // An archived task has no thread to open.
    for task in cache.tasks.values().filter(|t| t.archived_at.is_none()) {
        let root = task.card_message_id;
        let mut replies: Vec<&model::Message> =
            replies_by_root.get(&root).cloned().unwrap_or_default();
        replies.sort_by_key(|m| m.created_at);
        let mut posts = Vec::new();
        let (mut decisions, mut approvals) = (0, 0);
        for m in replies {
            let body = match m.kind {
                MessageKind::Approval => Some(match approval_by_message.get(&m.id).copied() {
                    Some(a) => {
                        if a.status == ApprovalStatus::Pending {
                            approvals += 1;
                        }
                        PostBody::Approval(approval_view(cache, a))
                    }
                    None => PostBody::Approval(closed_approval(m)),
                }),
                MessageKind::Decision => Some(match decision_by_message.get(&m.id).copied() {
                    Some(d) => {
                        if d.status == DecisionStatus::Pending {
                            decisions += 1;
                        }
                        PostBody::Decision(decision_view(d))
                    }
                    None => PostBody::Decision(closed_decision(m)),
                }),
                MessageKind::Notice => {
                    let (kind, text) = notice_parts(&m.body);
                    Some(PostBody::Notice { kind, text })
                }
                _ => Some(PostBody::Text(m.body.clone())),
            };
            if let Some(body) = body {
                let mut bodies = vec![body];
                if !m.attachments.is_empty() {
                    bodies.push(PostBody::Attachments(attachment_views(
                        &m.attachments,
                        &artifacts,
                        cache,
                    )));
                }
                posts.push(ThreadPost {
                    id: m.id.to_string(),
                    speaker: speaker(m),
                    time_label: format::time_label(m.created_at.as_millis()),
                    bodies,
                    pending: false,
                });
            }
        }
        for p in cache
            .pending
            .iter()
            .filter(|p| p.thread_root_id.as_deref() == Some(root.to_string().as_str()))
        {
            let mut bodies = vec![PostBody::Text(p.body.clone())];
            if !p.attachments.is_empty() {
                bodies.push(PostBody::Attachments(attachment_views(
                    &p.attachments,
                    &artifacts,
                    cache,
                )));
            }
            posts.push(ThreadPost {
                id: format!("pending-{}", p.client_message_id),
                speaker: Speaker::You,
                time_label: format::time_label(p.created_at),
                bodies,
                pending: true,
            });
        }
        // Folds the session's commands and file changes into one line, as
        // the first post.
        if let Some(session) = task.current_session_id {
            let (mut commands, mut files) = (0, std::collections::BTreeSet::new());
            let mut lines = Vec::new();
            for item in cache
                .session_items
                .get(&session.to_string())
                .into_iter()
                .flat_map(|items| items.values())
            {
                match item {
                    ItemSummary::Command(command) => {
                        commands += 1;
                        lines.push(command.clone());
                    }
                    ItemSummary::Files(paths) => {
                        for path in paths {
                            files.insert(path.clone());
                            lines.push(format!("Changed {path}"));
                        }
                    }
                }
            }
            if commands + files.len() > 0 {
                let summary = match (commands, files.len()) {
                    (0, f) => format!("Changed {f} {}", plural(f, "file", "files")),
                    (c, 0) => format!("Ran {c} {}", plural(c, "command", "commands")),
                    (c, f) => format!(
                        "Ran {c} {} and changed {f} {}",
                        plural(c, "command", "commands"),
                        plural(f, "file", "files")
                    ),
                };
                posts.insert(
                    0,
                    ThreadPost {
                        id: format!("activity-{session}"),
                        speaker: Speaker::Task,
                        time_label: format::time_label(task.created_at.as_millis()),
                        bodies: vec![PostBody::Activity {
                            summary,
                            items: lines,
                        }],
                        pending: false,
                    },
                );
            }
        }
        let worker = task
            .worker_id
            .as_ref()
            .and_then(|id| workers_by_id.get(&id.to_string()));
        let channel_isolation = cache
            .channels
            .get(&task.channel_id.to_string())
            .map(|c| c.default_isolation)
            .unwrap_or_default();
        threads.push(TaskThreadView {
            task_id: task.id.to_string(),
            title: task.title.clone(),
            status: task_status(task.status),
            status_detail: waiting_detail(decisions, approvals),
            reason: match task_status(task.status) {
                TaskStatus::Failed | TaskStatus::UpNext => task.status_reason.clone(),
                _ => None,
            },
            placement: worker.map(|w| PlacementView {
                worker_name: w.name.clone(),
                worker_os: os(w.os).label().to_owned(),
                worker_online: w.state == WorkerState::Online,
                harness: task_harness(cache, task, w),
                isolation: isolation(task.placement.isolation.unwrap_or(channel_isolation)),
                path: String::new(),
                branch: None,
                repo: task_repo(cache, task),
            }),
            posts,
            brief: task.brief.clone(),
            review_summary: task
                .review_summary
                .clone()
                .filter(|summary| !summary.trim().is_empty()),
            text_is_preview: cache.task_previews.contains(&task.id.to_string()),
            approval_mode: approval_mode(task.approval_mode),
            model_label: task_model_label(cache, task),
        });
    }

    // Workers.
    let mut workers: Vec<WorkerView> = cache
        .workers
        .values()
        .filter(|w| w.revoked_at.is_none())
        .map(|w| {
            let online = w.state == WorkerState::Online;
            let tools: &[model::ToolStatus] = w
                .inventory
                .as_ref()
                .map(|inv| inv.tools.as_slice())
                .unwrap_or_default();
            let accounts: Vec<HarnessAccountView> = w
                .inventory
                .as_ref()
                .map(|inv| {
                    inv.accounts
                        .iter()
                        .map(|a| account_view(cache, w, tools, a))
                        .collect()
                })
                .unwrap_or_default();
            let mut tool_rows: Vec<&model::ToolStatus> = tools.iter().collect();
            tool_rows.sort_by_key(|t| tool_rank(t.tool.as_str()));
            let tool_views: Vec<ToolView> = tool_rows
                .into_iter()
                .map(|t| tool_view(cache, &w.name, t))
                .collect();
            let mut detected_tags: Vec<String> = w
                .inventory
                .iter()
                .flat_map(|i| i.tags.iter())
                .filter(|t| !w.tags.contains(t))
                .cloned()
                .collect();
            detected_tags.dedup();
            let running = cache
                .tasks
                .values()
                .filter(|t| t.worker_id == Some(w.id) && t.status == PS::Working)
                .count();
            let os_label = os(w.os).label();
            let arch = format::arch_label(&w.arch);
            let mut list = vec![os_label.to_owned()];
            list.push(format!(
                "{} {}",
                accounts.len(),
                plural(accounts.len(), "account", "accounts")
            ));
            if running > 0 {
                list.push(format!(
                    "{running} {} running",
                    plural(running, "task", "tasks")
                ));
            }
            let last_seen = w.last_seen_at.map(|t| format::time_label(t.as_millis()));
            let policy = w.inventory.as_ref().map(|i| &i.policy);
            WorkerView {
                id: w.id.to_string(),
                name: w.name.clone(),
                os: os(w.os),
                online,
                paused: w.paused,
                sidebar_detail: if online {
                    os_label.to_owned()
                } else {
                    "Offline".to_owned()
                },
                list_detail: if online {
                    list.join(", ")
                } else {
                    match &last_seen {
                        Some(t) => format!("Offline since {t}"),
                        None => "Offline".to_owned(),
                    }
                },
                status_line: if online {
                    format!("Online. {os_label}, {arch}, worker {}.", w.version)
                } else {
                    match &last_seen {
                        Some(t) => format!(
                            "Offline since {t}. {os_label}, {arch}, worker {}.",
                            w.version
                        ),
                        None => format!("Offline. {os_label}, {arch}, worker {}.", w.version),
                    }
                },
                tags: w.tags.clone(),
                detected_tags,
                accounts,
                tools: tool_views,
                tools_folder: if w.os == model::Os::Windows {
                    r"%USERPROFILE%\.consort\tools".to_owned()
                } else {
                    "~/.consort/tools".to_owned()
                },
                policy: PolicyView {
                    max_approval_mode: policy
                        .map(|p| approval_mode(p.max_approval_mode))
                        .unwrap_or_default(),
                    allowed_roots: policy.map(|p| p.allowed_roots.clone()).unwrap_or_default(),
                    allow_shell: policy.is_some_and(|p| p.allow_shell),
                    shell_name: if w.os == model::Os::Windows {
                        "PowerShell"
                    } else {
                        "the shell"
                    }
                    .to_owned(),
                },
            }
        })
        .collect();
    workers.sort_by_key(|w| (!w.online, w.name.to_lowercase()));

    // The picker groups models by account label. A paused or offline
    // worker still counts: placement waits for it.
    let model_groups = crate::picker::model_groups(
        cache.workers.values().filter_map(|w| {
            w.inventory
                .as_ref()
                .map(|i| (w.name.as_str(), i.accounts.as_slice()))
        }),
        |harness| provider_logo_of(cache, harness),
    );
    let mut sign_ins: Vec<&model::SignIn> = cache
        .sign_ins
        .values()
        .filter(|s| !cache.sign_ins_dismissed.contains(&s.id.to_string()))
        .collect();
    sign_ins.sort_by_key(|s| s.created_at);
    let sign_ins: Vec<SignInView> = sign_ins
        .into_iter()
        .map(|s| SignInView {
            id: s.id.to_string(),
            worker_id: s.worker_id.to_string(),
            account_id: s.account_id.to_string(),
            method: Some(sign_in_choice(&s.method)),
            step: sign_in_step(&s.state),
        })
        .collect();
    let acp_registry = AcpRegistryView {
        load: cache.acp_registry_load.clone(),
        agents: cache
            .acp_registry
            .iter()
            .map(|a| AcpAgentView {
                id: a.id.clone(),
                name: a.name.clone(),
                description: a.description.clone(),
                version: a.version.clone(),
                logo: ProviderLogo::Registry {
                    id: a.id.clone(),
                    svg: cache.acp_icons.get(&a.id).cloned(),
                },
                supported: a.supported,
            })
            .collect(),
    };

    let mut state = ViewState {
        connection,
        user,
        sidebar: SidebarView::default(),
        channels: channel_views,
        groups: groups
            .iter()
            .map(|g| GroupView {
                id: g.id.to_string(),
                name: g.name.clone(),
            })
            .collect(),
        archived_channels,
        timelines,
        tasks,
        threads,
        tasks_panel: TasksPanelView::default(),
        workers,
        tags: Vec::new(),
        model_groups,
        favorite_models: cache
            .preferences
            .favorite_models
            .iter()
            .map(|f| FavoriteModelView {
                account_label: f.account_label.clone(),
                model_id: f.model.clone(),
            })
            .collect(),
        sign_ins,
        acp_registry,
        worker_pairing: None,
        send_failures: cache
            .send_failures
            .iter()
            .map(|f| SendFailureView {
                id: f.id.clone(),
                channel_id: f.channel_id.clone(),
                task_id: f
                    .thread_root_id
                    .as_ref()
                    .and_then(|root| task_by_card.get(root).map(|t| t.id.to_string())),
                channel_name: cache
                    .channels
                    .get(&f.channel_id)
                    .map(shown_name)
                    .unwrap_or_else(|| f.channel_name.clone()),
                body: f.body.clone(),
                message: format!(
                    "{} Put your text back in the message box, then send it again.",
                    sentence(&f.message)
                ),
            })
            .collect(),
        connections: {
            let mut list: Vec<&model::Connection> = cache.connections.values().collect();
            list.sort_by_key(|c| (c.created_at.as_millis(), c.id.to_string()));
            list.into_iter()
                .map(|c| crate::extensions_view::connection_view(c, cache))
                .collect()
        },
        skills: {
            let mut list: Vec<&model::Skill> = cache.skills.values().collect();
            list.sort_by_key(|s| (s.installed_at.as_millis(), s.id.to_string()));
            list.into_iter()
                .map(|s| crate::extensions_view::skill_view(s, now))
                .collect()
        },
        composio: cache.extensions.composio.clone(),
        connector_catalog: {
            let mut catalog = cache.extensions.catalog.clone();
            for service in &mut catalog.services {
                service.logo =
                    crate::extensions_view::with_logo(std::mem::take(&mut service.logo), cache);
                // The count follows the connections that the feed brings.
                let connected = cache
                    .connections
                    .values()
                    .filter(|c| c.service == service.slug)
                    .count();
                service.connected_count = u32::try_from(connected).unwrap_or(u32::MAX);
            }
            catalog
        },
        service_tools: cache.extensions.service_tools.values().cloned().collect(),
        skill_directory: cache.extensions.directory.clone(),
        activity: Vec::new(),
        // The uploads live in the hub backend, not in the cache. It adds
        // them after this.
        uploads: Vec::new(),
        channel_files: channel_file_views(cache, now),
        files_loaded: cache.files_loaded.iter().cloned().collect(),
        admin: crate::admin::admin_view(cache, online, now),
        routines: routine_views(cache),
        memory_notes: memory_note_views(cache),
        memory_loaded: cache.memory_loaded.iter().cloned().collect(),
        memory_deleted_loaded: cache.memory_deleted_loaded.iter().cloned().collect(),
    };
    state.activity = channel_activities(cache, &state);
    // Collapsed groups and unread counts come from the hub, so seed the
    // sidebar before the shared rules rebuild it.
    state.sidebar.groups = groups
        .iter()
        .map(|g| SidebarGroup {
            id: g.id.to_string(),
            name: g.name.clone(),
            collapsed: g.collapsed,
            channels: Vec::new(),
        })
        .collect();
    state.sidebar.ungrouped = state
        .channels
        .iter()
        .map(|c| SidebarChannel {
            id: c.id.clone(),
            unread: unread.get(&c.id).copied().unwrap_or(0),
            ..SidebarChannel::default()
        })
        .collect();
    if let Some(e) = state.tutti() {
        state.sidebar.tutti = SidebarChannel {
            id: e.id.clone(),
            unread: unread.get(&e.id).copied().unwrap_or(0),
            ..SidebarChannel::default()
        };
    }
    refresh(&mut state);
    state
}

/// The detail line of a harness account: the harness and its version, then
/// what the worker said about the sign-in. Words in backticks come back as
/// code words, without the backticks.
fn account_detail(
    harness: &str,
    version: Option<&str>,
    auth_detail: Option<&str>,
) -> (String, Vec<String>) {
    let head = match version {
        Some(v) => format!("{harness} {v}"),
        None => harness.to_owned(),
    };
    let text = [Some(head.as_str()), auth_detail]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(sentence)
        .collect::<Vec<_>>()
        .join(" ");
    let mut plain = String::with_capacity(text.len());
    let mut code = Vec::new();
    for (i, part) in text.split('`').enumerate() {
        if i % 2 == 1 && !part.is_empty() {
            code.push(part.to_owned());
        }
        plain.push_str(part);
    }
    (plain, code)
}

/// Splits the words in backticks out of a detail line, as code words.
fn account_detail_code(text: &str) -> (String, Vec<String>) {
    let mut plain = String::with_capacity(text.len());
    let mut code = Vec::new();
    for (i, part) in text.split('`').enumerate() {
        if i % 2 == 1 && !part.is_empty() {
            code.push(part.to_owned());
        }
        plain.push_str(part);
    }
    (plain, code)
}

/// The reason of an account without the step "Sign in to this account in
/// Settings > Workers." at its end. A reason that is only that step becomes
/// "Not signed in." for a signed-out account (see `account_view`).
fn strip_settings_step(detail: &str) -> &str {
    detail
        .strip_suffix(model::SIGN_IN_IN_SETTINGS)
        .map_or(detail, str::trim_end)
}

/// The text as a sentence: a period goes on the end only when the text
/// does not already end with punctuation.
pub(crate) fn sentence(text: &str) -> String {
    let text = text.trim_end();
    let stop = text
        .trim_end_matches(['`', '"', '\'', ')', ']'])
        .ends_with(['.', '!', '?', ':', ';', '\u{2026}']);
    if stop {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

/// The model of a task in words, from the task's own selection, its
/// channel's default or the Tutti default. The task keeps the model it
/// started with, so the thread shows it and does not offer to change it.
fn task_model_label(cache: &Cache, task: &model::Task) -> Option<String> {
    let channel = cache.channels.get(&task.channel_id.to_string());
    let tutti = cache.channels.values().find(|c| c.kind == PK::Tutti);
    let chosen = task
        .model_selection
        .as_ref()
        .or(channel.and_then(|c| c.task_selection.as_ref()))
        .or(tutti.and_then(|c| c.task_selection.as_ref()))?;
    let view = selection_view(cache, chosen);
    let effort = model_effort(cache, &chosen.account_label, &chosen.model);
    Some(crate::picker::selection_words(&view, effort.as_ref()))
}

/// The effort levels of a model, from the first worker that reports it.
fn model_effort(cache: &Cache, account_label: &str, model_id: &str) -> Option<EffortView> {
    cache
        .workers
        .values()
        .filter_map(|w| w.inventory.as_ref())
        .flat_map(|i| i.accounts.iter())
        .filter(|a| a.label == account_label)
        .flat_map(|a| a.models.iter())
        .find(|m| m.id == model_id)
        .and_then(|m| crate::picker::effort_view(&m.options))
}

/// The provider name and logo of a harness, with the icon of a registry
/// agent once the client has it.
fn provider_logo_of(cache: &Cache, harness: &str) -> (String, ProviderLogo) {
    let (name, logo) = provider_of(harness);
    let logo = match logo {
        ProviderLogo::Registry { id, .. } => {
            let svg = cache.acp_icons.get(&id).cloned();
            ProviderLogo::Registry { id, svg }
        }
        other => other,
    };
    (name, logo)
}

/// The account and model of a task, best known first: the task's own
/// selection, its channel's default, the Tutti default, and, once the
/// task runs on a worker with one signed-in account, that account.
fn task_harness(cache: &Cache, task: &model::Task, worker: &model::Worker) -> String {
    let channel = cache.channels.get(&task.channel_id.to_string());
    let tutti = cache.channels.values().find(|c| c.kind == PK::Tutti);
    let chosen = task
        .model_selection
        .as_ref()
        .or(channel.and_then(|c| c.task_selection.as_ref()))
        .or(tutti.and_then(|c| c.task_selection.as_ref()));
    if let Some(selection) = chosen {
        return selection_view(cache, selection).label();
    }
    if task.current_session_id.is_some() {
        let signed_in: Vec<&model::HarnessAccount> = worker
            .inventory
            .iter()
            .flat_map(|i| i.accounts.iter())
            .filter(|a| a.enabled && a.auth_state == ProtoAuth::SignedIn)
            .collect();
        if let [only] = signed_in.as_slice() {
            return only.label.clone();
        }
    }
    "The channel's default".to_owned()
}

/// The repo a task's workspace comes from: the task's own repos, else its
/// channel's.
fn task_repo(cache: &Cache, task: &model::Task) -> Option<String> {
    let channel = cache.channels.get(&task.channel_id.to_string());
    let ids = task
        .placement
        .repos
        .clone()
        .or_else(|| channel.map(|c| c.repo_ids.clone()))
        .unwrap_or_default();
    ids.first().map(|id| {
        cache
            .repos
            .get(&id.to_string())
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "a repo".to_owned())
    })
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 { one } else { many }
}

trait LowerFirst {
    fn to_lowercase_first(&self) -> String;
}

impl LowerFirst for String {
    /// "Today" becomes "today", for use inside a sentence. Day names keep
    /// their capital.
    fn to_lowercase_first(&self) -> String {
        match self.as_str() {
            "Today" => "today".to_owned(),
            "Yesterday" => "yesterday".to_owned(),
            _ => self.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::{
        client_api::{EntityPayload, FeedEntry},
        ids::{
            ApprovalId, ChannelId, DecisionId, GroupId, MessageId, RequestId, SessionId, TaskId,
        },
        model::{
            Approval, ApprovalDetails, ApprovalMode, ApprovalStatus, Author, Channel, ChannelKind,
            Decision, DecisionOption as PO, DecisionStatus, Group, Isolation, Message, MessageKind,
            PlacementRequest, ReadMarker, RequestOption, RequestOptionKind, Task, TaskCreator,
        },
        status::TaskStatus as PS,
        time::Timestamp,
    };

    use super::*;
    use crate::cache::PendingSend;
    use crate::view::ApprovalMode as VMode;
    use consort_proto::client_api::{EntityKind, FeedChange};

    const NOW: i64 = 1_793_300_000_000;

    fn channel(id: ChannelId, kind: ChannelKind, name: &str, group: Option<GroupId>) -> Channel {
        Channel {
            id,
            kind,
            name: name.into(),
            icon: None,
            color: None,
            group_id: group,
            position: 0,
            purpose: String::new(),
            instructions: String::new(),
            coordinator_selection: None,
            task_selection: None,
            preferred_workers: vec![],
            repo_ids: vec![],
            default_isolation: Isolation::Worktree,
            default_approval_mode: ApprovalMode::AutoEdits,
            allowed_local_mcp_servers: vec![],
            memory_enabled: true,
            disabled_connections: Vec::new(),
            disabled_skills: Vec::new(),
            archived_at: None,
            created_at: Timestamp::from_millis(NOW - 100_000),
            updated_at: Timestamp::from_millis(NOW - 100_000),
            tags: Vec::new(),
        }
    }

    fn message(
        id: MessageId,
        channel_id: ChannelId,
        author: Author,
        kind: MessageKind,
        root: Option<MessageId>,
        at: i64,
    ) -> Message {
        Message {
            id,
            channel_id,
            thread_root_id: root,
            author,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind,
            body: format!("message at {at}"),
            attachments: vec![],
            in_timeline: false,
            client_message_id: None,
            reply_count: 0,
            last_reply_at: None,
            reactions: vec![],
            edited_at: None,
            deleted_at: None,
            created_at: Timestamp::from_millis(at),
        }
    }

    struct World {
        cache: Cache,
        tutti: ChannelId,
        sayso: ChannelId,
        task: TaskId,
        card: MessageId,
    }

    fn world() -> World {
        let mut cache = Cache::default();
        let group = GroupId::new();
        let tutti = ChannelId::new();
        let sayso = ChannelId::new();
        cache.groups.insert(
            group.to_string(),
            Group {
                id: group,
                name: "Personal projects".into(),
                position: 0,
                collapsed: false,
            },
        );
        for c in [
            channel(sayso, ChannelKind::Standard, "sayso", Some(group)),
            channel(tutti, ChannelKind::Tutti, "Tutti", None),
        ] {
            cache.channels.insert(c.id.to_string(), c);
        }
        let task = TaskId::new();
        let card = MessageId::new();
        let session = SessionId::new();
        let first = message(
            MessageId::new(),
            sayso,
            Author::Assistant,
            MessageKind::Text,
            None,
            NOW - 50_000,
        );
        let card_msg = message(
            card,
            sayso,
            Author::Assistant,
            MessageKind::TaskCard,
            None,
            NOW - 40_000,
        );
        let approval_msg = message(
            MessageId::new(),
            sayso,
            Author::Assistant,
            MessageKind::Approval,
            Some(card),
            NOW - 30_000,
        );
        let decision_msg = message(
            MessageId::new(),
            sayso,
            Author::Assistant,
            MessageKind::Decision,
            Some(card),
            NOW - 20_000,
        );
        for m in [
            first.clone(),
            card_msg,
            approval_msg.clone(),
            decision_msg.clone(),
        ] {
            cache.messages.insert(m.id.to_string(), m);
        }
        cache.read_markers.insert(
            sayso.to_string(),
            ReadMarker {
                channel_id: sayso,
                thread_root_id: None,
                last_read_message_id: first.id,
            },
        );
        cache.tasks.insert(
            task.to_string(),
            Task {
                id: task,
                channel_id: sayso,
                title: "Go through open Sayso issues".into(),
                brief: String::new(),
                status: PS::NeedsInput,
                status_reason: None,
                created_by: TaskCreator::Assistant,
                card_message_id: card,
                model_selection: None,
                placement: PlacementRequest::default(),
                approval_mode: ApprovalMode::Ask,
                start_at: None,
                current_session_id: Some(session),
                worker_id: None,
                review_summary: None,
                resolved_at: None,
                archived_at: None,
                created_at: Timestamp::from_millis(NOW - 40_000),
                updated_at: Timestamp::from_millis(NOW - 120_000),
            },
        );
        let approval = ApprovalId::new();
        cache.approvals.insert(
            approval.to_string(),
            Approval {
                id: approval,
                channel_id: sayso,
                session_id: session,
                task_id: Some(task),
                message_id: approval_msg.id,
                request_id: RequestId::new("r"),
                summary: "Codex wants to run a command".into(),
                details: ApprovalDetails::Command {
                    command: "cargo build".into(),
                    cwd: Some("C:\\Code".into()),
                },
                options: vec![
                    RequestOption {
                        id: "d".into(),
                        label: "Deny".into(),
                        kind: RequestOptionKind::DenyOnce,
                        description: None,
                    },
                    RequestOption {
                        id: "a".into(),
                        label: "Allow".into(),
                        kind: RequestOptionKind::AllowOnce,
                        description: None,
                    },
                ],
                status: ApprovalStatus::Pending,
                answer_option_id: None,
                answered_at: None,
                answered_by_client_id: None,
                created_at: Timestamp::from_millis(NOW - 30_000),
            },
        );
        let decision = DecisionId::new();
        cache.decisions.insert(
            decision.to_string(),
            Decision {
                id: decision,
                channel_id: sayso,
                task_id: Some(task),
                session_id: session,
                message_id: decision_msg.id,
                request_id: None,
                question: "Ship now?".into(),
                context: None,
                options: vec![
                    PO {
                        label: "Yes".into(),
                        consequence: "It ships.".into(),
                        id: None,
                    },
                    PO {
                        label: "No".into(),
                        consequence: "It waits.".into(),
                        id: None,
                    },
                ],
                recommended_index: Some(1),
                allow_free_text: true,
                status: DecisionStatus::Pending,
                answer: None,
                answered_at: None,
                created_at: Timestamp::from_millis(NOW - 20_000),
            },
        );
        World {
            cache,
            tutti,
            sayso,
            task,
            card,
        }
    }

    fn view(cache: &Cache) -> ViewState {
        view_state(cache, Connection::Online, UserView::default(), NOW)
    }

    #[test]
    fn tutti_comes_first_and_groups_hold_their_channels() {
        let w = world();
        let state = view(&w.cache);
        assert_eq!(state.channels[0].id, w.tutti.to_string());
        assert_eq!(state.channels[0].icon, "brace");
        assert_eq!(state.sidebar.groups[0].channels[0].name, "sayso");
        assert_eq!(
            state.sidebar.groups[0].channels[0].status,
            Some(TaskStatus::NeedsInput)
        );
    }

    #[test]
    fn a_task_card_shows_its_pending_approval_inline() {
        let w = world();
        let state = view(&w.cache);
        let timeline = state.timeline(&w.sayso.to_string()).expect("timeline");
        assert!(matches!(timeline.items[0], TimelineItem::Day(_)));
        let card = timeline
            .items
            .iter()
            .find_map(|i| match i {
                TimelineItem::TaskCard(c) => Some(c),
                _ => None,
            })
            .expect("card");
        assert_eq!(card.task_id, w.task.to_string());
        let approval = card.approval.as_ref().expect("inline approval");
        assert_eq!(
            approval.choices,
            vec![ApprovalChoice::AllowOnce, ApprovalChoice::Deny]
        );
        assert_eq!(approval.folder.as_deref(), Some("C:\\Code"));
        // Thread replies are not in the timeline.
        assert_eq!(timeline.items.len(), 3);
    }

    #[test]
    fn the_thread_holds_the_approval_and_the_decision() {
        let w = world();
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        assert_eq!(thread.status_detail, "1 decision, 1 approval");
        assert!(matches!(thread.posts[0].bodies[0], PostBody::Approval(_)));
        match &thread.posts[1].bodies[0] {
            PostBody::Decision(d) => {
                assert_eq!(d.recommended, Some(1));
                assert_eq!(d.state, RequestState::Pending);
            }
            other => panic!("not a decision: {other:?}"),
        }
        assert_eq!(state.tasks_panel.need_you, 1);
        assert_eq!(state.tasks[0].meta, "Needs input. sayso");
        assert_eq!(state.tasks[0].age_label, "2 min");
    }

    #[test]
    fn unread_counts_follow_the_read_marker() {
        let w = world();
        let state = view(&w.cache);
        let row = &state.sidebar.groups[0].channels[0];
        // The task card comes after the marker. Thread replies do not count.
        assert_eq!(row.unread, 1);
        assert!(row.attention);
    }

    #[test]
    fn a_pending_send_shows_until_the_hub_stores_it() {
        let mut w = world();
        w.cache.pending.push(PendingSend {
            client_message_id: "c-1".into(),
            channel_id: w.tutti.to_string(),
            thread_root_id: None,
            body: "Hi".into(),
            created_at: NOW,
            attachments: Vec::new(),
        });
        let state = view(&w.cache);
        let items = &state
            .timeline(&w.tutti.to_string())
            .expect("timeline")
            .items;
        assert!(
            matches!(items.last(), Some(TimelineItem::Message(m)) if m.pending && m.body == "Hi")
        );

        let mut stored = message(
            MessageId::new(),
            w.tutti,
            Author::User,
            MessageKind::Text,
            None,
            NOW,
        );
        stored.body = "Hi".into();
        stored.client_message_id = Some("c-1".into());
        assert!(w.cache.apply(FeedEntry::upsert(
            1,
            Timestamp::from_millis(NOW),
            EntityPayload::Message(stored)
        )));
        let state = view(&w.cache);
        let items = &state
            .timeline(&w.tutti.to_string())
            .expect("timeline")
            .items;
        let mine: Vec<_> = items
            .iter()
            .filter(|i| matches!(i, TimelineItem::Message(m) if m.body == "Hi"))
            .collect();
        assert_eq!(mine.len(), 1, "no duplicate after the hub stored it");
        assert!(
            matches!(mine[0], TimelineItem::Message(m) if !m.pending && m.speaker == Speaker::You)
        );
    }

    /// What the hub stores when Claude Code asks to use a hub tool in a
    /// coordinator turn: a top-level approval message with no task.
    fn coordinator_approval(w: &mut World) -> ApprovalId {
        let mut message = message(
            MessageId::new(),
            w.tutti,
            Author::Assistant,
            MessageKind::Approval,
            None,
            NOW - 1_000,
        );
        message.body = "mcp__hub__post_message".into();
        let id = ApprovalId::new();
        let option = |id: &str, label: &str, kind| RequestOption {
            id: id.into(),
            label: label.into(),
            kind,
            description: None,
        };
        w.cache.approvals.insert(
            id.to_string(),
            Approval {
                id,
                channel_id: w.tutti,
                session_id: SessionId::new(),
                task_id: None,
                message_id: message.id,
                request_id: RequestId::new("tool-1"),
                summary: "mcp__hub__post_message".into(),
                details: ApprovalDetails::Tool {
                    server: Some("hub".into()),
                    tool: "post_message".into(),
                    arguments: serde_json::json!({ "body": "Good morning" }),
                },
                options: vec![
                    option("yes", "Yes", RequestOptionKind::AllowOnce),
                    option(
                        "always",
                        "Yes, and don't ask again",
                        RequestOptionKind::AllowAlways,
                    ),
                    option("no", "No", RequestOptionKind::DenyOnce),
                ],
                status: ApprovalStatus::Pending,
                answer_option_id: None,
                answered_at: None,
                answered_by_client_id: None,
                created_at: Timestamp::from_millis(NOW - 1_000),
            },
        );
        w.cache.messages.insert(message.id.to_string(), message);
        id
    }

    #[test]
    fn a_coordinator_approval_is_a_card_in_the_timeline() {
        let mut w = world();
        let id = coordinator_approval(&mut w);
        let state = view(&w.cache);
        let items = &state
            .timeline(&w.tutti.to_string())
            .expect("timeline")
            .items;
        let card = items
            .iter()
            .find_map(|i| match i {
                TimelineItem::Approval(card) => Some(card),
                _ => None,
            })
            .expect("an approval card, not a plain message");
        assert!(
            !items
                .iter()
                .any(|i| matches!(i, TimelineItem::Message(m) if m.body.contains("mcp__"))),
            "the raw tool name is not a message"
        );
        let approval = &card.approval;
        assert_eq!(approval.id, id.to_string());
        assert_eq!(approval.task_id, None);
        assert_eq!(
            approval.summary,
            "Consort wants to use post_message from hub"
        );
        assert!(
            approval
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("\"body\": \"Good morning\""))
        );
        assert_eq!(
            approval.choices,
            vec![
                ApprovalChoice::AllowOnce,
                ApprovalChoice::AllowForTask,
                ApprovalChoice::Deny
            ]
        );
        // The harness's own words stand on the wide button, so the user sees
        // the scope it has (review cycle 3, S4).
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowForTask),
            "Yes, and don't ask again"
        );
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowOnce),
            "Allow once"
        );
        assert_eq!(card.speaker, Speaker::Consort);
        // Tutti waits for the user.
        assert_eq!(state.sidebar.tutti.status, Some(TaskStatus::NeedsInput));
        assert!(state.sidebar.tutti.attention);
        // The answer goes out by approval ID, with no task.
        let stored = w.cache.approvals.get(&id.to_string()).expect("stored");
        assert_eq!(
            crate::hub::option_for(stored, ApprovalChoice::AllowForTask).as_deref(),
            Some("always")
        );
        assert_eq!(
            crate::hub::option_for(stored, ApprovalChoice::Deny).as_deref(),
            Some("no")
        );
    }

    #[test]
    fn a_raw_tool_name_never_becomes_the_summary() {
        let mut w = world();
        let id = coordinator_approval(&mut w);
        if let Some(a) = w.cache.approvals.get_mut(&id.to_string()) {
            a.details = ApprovalDetails::Tool {
                server: None,
                tool: "mcp__github__create_issue".into(),
                arguments: serde_json::Value::Null,
            };
        }
        let state = view(&w.cache);
        let summary = state
            .timeline(&w.tutti.to_string())
            .and_then(|t| {
                t.items.iter().find_map(|i| match i {
                    TimelineItem::Approval(card) => Some(card.approval.summary.clone()),
                    _ => None,
                })
            })
            .expect("card");
        assert_eq!(summary, "Consort wants to use create_issue from github");
    }

    #[test]
    fn a_task_approval_keeps_its_label_and_its_summary() {
        let w = world();
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        let PostBody::Approval(approval) = &thread.posts[0].bodies[0] else {
            panic!("not an approval");
        };
        assert_eq!(
            approval.task_id.as_deref(),
            Some(w.task.to_string().as_str())
        );
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowForTask),
            "Allow for this task"
        );
        assert_eq!(approval.summary, "Codex wants to run a command");
    }

    #[test]
    fn a_channel_decision_is_a_card_and_marks_the_channel() {
        let mut w = world();
        // The sayso task no longer waits, so only the decision can mark it.
        if let Some(task) = w.cache.tasks.get_mut(&w.task.to_string()) {
            task.status = PS::Working;
        }
        w.cache.approvals.clear();
        w.cache.decisions.clear();
        let msg = message(
            MessageId::new(),
            w.sayso,
            Author::Assistant,
            MessageKind::Decision,
            None,
            NOW - 500,
        );
        let id = DecisionId::new();
        let option = |label: &str, consequence: &str| PO {
            label: label.into(),
            consequence: consequence.into(),
            id: None,
        };
        w.cache.decisions.insert(
            id.to_string(),
            Decision {
                id,
                channel_id: w.sayso,
                task_id: None,
                session_id: SessionId::new(),
                message_id: msg.id,
                request_id: None,
                question: "Which build ships first?".into(),
                context: Some("Both pass.".into()),
                options: vec![
                    option("x86_64", "Most users get it today."),
                    option("arm64", "Mac users get it today."),
                ],
                recommended_index: Some(0),
                allow_free_text: true,
                status: DecisionStatus::Pending,
                answer: None,
                answered_at: None,
                created_at: Timestamp::from_millis(NOW - 500),
            },
        );
        w.cache.messages.insert(msg.id.to_string(), msg);
        let state = view(&w.cache);
        let items = &state
            .timeline(&w.sayso.to_string())
            .expect("timeline")
            .items;
        let card = items
            .iter()
            .find_map(|i| match i {
                TimelineItem::Decision(card) => Some(card),
                _ => None,
            })
            .expect("a decision card");
        assert_eq!(card.decision.question, "Which build ships first?");
        assert_eq!(card.decision.task_id, None);
        assert_eq!(card.decision.recommended, Some(0));
        let row = &state.sidebar.groups[0].channels[0];
        assert_eq!(row.status, Some(TaskStatus::NeedsInput));
        assert!(row.attention);
        // Once answered, the channel shows its task's glyph again.
        let mut answered = w.cache.clone();
        if let Some(d) = answered.decisions.get_mut(&id.to_string()) {
            d.status = DecisionStatus::Answered;
        }
        let state = view(&answered);
        assert_eq!(
            state.sidebar.groups[0].channels[0].status,
            Some(TaskStatus::Working)
        );
    }

    #[test]
    fn an_answered_request_with_no_row_shows_as_closed() {
        let mut w = world();
        let id = coordinator_approval(&mut w);
        // After a restart the snapshot holds only pending approvals.
        w.cache.approvals.remove(&id.to_string());
        let mut decision = message(
            MessageId::new(),
            w.tutti,
            Author::Assistant,
            MessageKind::Decision,
            None,
            NOW,
        );
        decision.body = "Ship now?".into();
        w.cache.messages.insert(decision.id.to_string(), decision);
        let state = view(&w.cache);
        let items = &state
            .timeline(&w.tutti.to_string())
            .expect("timeline")
            .items;
        let approval = items
            .iter()
            .find_map(|i| match i {
                TimelineItem::Approval(c) => Some(&c.approval),
                _ => None,
            })
            .expect("a closed approval card");
        assert_eq!(approval.state, RequestState::Closed);
        assert_eq!(
            approval.summary,
            "Consort wanted to use post_message from hub"
        );
        assert!(approval.choices.is_empty());
        let decision = items
            .iter()
            .find_map(|i| match i {
                TimelineItem::Decision(c) => Some(&c.decision),
                _ => None,
            })
            .expect("a closed decision card");
        assert_eq!(decision.state, RequestState::Closed);
        assert_eq!(decision.question, "Ship now?");
        // Closed requests do not wait for the user.
        assert_eq!(state.sidebar.tutti.status, None);
    }

    #[test]
    fn an_account_detail_ends_with_one_period_and_keeps_code_words() {
        assert_eq!(
            account_detail(
                "codex",
                Some("0.5.1"),
                Some("Run `codex login` on this machine.")
            ),
            (
                "codex 0.5.1. Run codex login on this machine.".to_owned(),
                vec!["codex login".to_owned()]
            )
        );
        assert_eq!(
            account_detail("opencode", None, Some("Run `opencode auth list`")).0,
            "opencode. Run opencode auth list."
        );
        assert_eq!(sentence("Signed in (Pro)"), "Signed in (Pro).");
        assert_eq!(sentence("Is it signed in?"), "Is it signed in?");
        assert_eq!(sentence("See the \"docs.\""), "See the \"docs.\"");
    }

    #[test]
    fn a_file_change_summary_names_the_file_and_keeps_the_path() {
        let mut w = world();
        let path = "/private/tmp/consort-e2e-data/worker/workspaces/tsk_1/hello.txt";
        if let Some(a) = w.cache.approvals.values_mut().next() {
            a.summary = format!("Write {path}");
            a.details = ApprovalDetails::FileChange {
                paths: vec![path.into()],
            };
        }
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        let PostBody::Approval(approval) = &thread.posts[0].bodies[0] else {
            panic!("not an approval");
        };
        assert_eq!(approval.summary, "Write hello.txt");
        assert_eq!(approval.detail.as_deref(), Some(path));
        assert_eq!(
            file_change_summary("", &["C:\\Code\\a.rs".into()]),
            "Change a.rs"
        );
    }

    #[test]
    fn the_placement_names_the_repo_or_a_new_folder_and_the_harness() {
        let mut w = world();
        let worker = consort_proto::model::Worker {
            id: consort_proto::ids::WorkerId::new(),
            name: "bigboi".into(),
            hostname: "bigboi".into(),
            os: consort_proto::model::Os::Windows,
            arch: "x64".into(),
            public_key: consort_proto::auth::PublicKey::from_bytes([1; 32]),
            version: "0.1.0".into(),
            protocol_version: 1,
            tags: vec![],
            paused: false,
            state: consort_proto::model::WorkerState::Online,
            last_seen_at: None,
            inventory: None,
            paired_at: Timestamp::from_millis(NOW),
            revoked_at: None,
        };
        let worker_id = worker.id;
        w.cache.workers.insert(worker_id.to_string(), worker);
        if let Some(task) = w.cache.tasks.get_mut(&w.task.to_string()) {
            task.worker_id = Some(worker_id);
        }
        let state = view(&w.cache);
        let placement = state
            .thread(&w.task.to_string())
            .and_then(|t| t.placement.clone())
            .expect("placement");
        assert_eq!(placement.repo, None, "no repo: a new empty folder");
        assert_eq!(placement.harness, "The channel's default");

        if let Some(task) = w.cache.tasks.get_mut(&w.task.to_string()) {
            task.model_selection = Some(consort_proto::model::ModelSelection {
                account_label: "Claude Personal".into(),
                model: "Opus 5.5".into(),
                options: Default::default(),
            });
        }
        let state = view(&w.cache);
        let placement = state
            .thread(&w.task.to_string())
            .and_then(|t| t.placement.clone())
            .expect("placement");
        assert_eq!(placement.harness, "Claude Personal, Opus 5.5");
    }

    #[test]
    fn old_feed_entries_are_ignored_and_deletes_apply() {
        let mut w = world();
        w.cache.seq = 10;
        let late = message(
            MessageId::new(),
            w.sayso,
            Author::User,
            MessageKind::Text,
            None,
            NOW,
        );
        assert!(!w.cache.apply(FeedEntry::upsert(
            9,
            Timestamp::from_millis(NOW),
            EntityPayload::Message(late)
        )));
        assert!(w.cache.apply(FeedEntry::delete(
            11,
            Timestamp::from_millis(NOW),
            consort_proto::client_api::EntityKind::Task,
            w.task.to_string(),
            Some(w.sayso),
        )));
        let state = view(&w.cache);
        assert!(state.task(&w.task.to_string()).is_none());
        let _ = w.card;
    }

    /// Finding 11: with the read marker older than the loaded page, five
    /// unread messages counted as zero.
    #[test]
    fn unread_counts_from_the_oldest_loaded_message_when_the_marker_is_older() {
        let mut cache = Cache::default();
        let tutti = ChannelId::new();
        let c = channel(tutti, ChannelKind::Tutti, "Tutti", None);
        cache.channels.insert(c.id.to_string(), c);
        for i in 0..5 {
            let m = message(
                MessageId::new(),
                tutti,
                Author::Assistant,
                MessageKind::Text,
                None,
                NOW - 10_000 + i,
            );
            cache.messages.insert(m.id.to_string(), m);
        }
        let mine = message(
            MessageId::new(),
            tutti,
            Author::User,
            MessageKind::Text,
            None,
            NOW - 9_000,
        );
        cache.messages.insert(mine.id.to_string(), mine);
        // The marker names a message that this client has not loaded.
        cache.read_markers.insert(
            tutti.to_string(),
            ReadMarker {
                channel_id: tutti,
                thread_root_id: None,
                last_read_message_id: MessageId::new(),
            },
        );
        let state = view(&cache);
        assert_eq!(state.sidebar.tutti.unread, 5);
    }

    #[test]
    fn unread_counts_only_the_messages_after_a_marker_that_was_deleted() {
        let mut w = world();
        let ids: Vec<MessageId> = (0..4)
            .map(|i| {
                let m = message(
                    MessageId::new(),
                    w.tutti,
                    Author::Assistant,
                    MessageKind::Text,
                    None,
                    NOW - 8_000 + i * 1_000,
                );
                let id = m.id;
                w.cache.messages.insert(id.to_string(), m);
                id
            })
            .collect();
        // The second message is the marker, and it was deleted since.
        if let Some(m) = w.cache.messages.get_mut(&ids[1].to_string()) {
            m.deleted_at = Some(Timestamp::from_millis(NOW));
        }
        w.cache.read_markers.insert(
            w.tutti.to_string(),
            ReadMarker {
                channel_id: w.tutti,
                thread_root_id: None,
                last_read_message_id: ids[1],
            },
        );
        let state = view(&w.cache);
        assert_eq!(state.sidebar.tutti.unread, 2);
    }

    /// Finding 3: deleting a channel left its tasks, messages, approvals and
    /// the rest in the cache, and the Tasks panel still showed the tasks.
    #[test]
    fn a_channel_delete_drops_everything_that_belongs_to_the_channel() {
        let mut w = world();
        // A running command in the task's session, and a message that waits.
        let session = w
            .cache
            .tasks
            .get(&w.task.to_string())
            .and_then(|t| t.current_session_id);
        let session = session.expect("session");
        let item: consort_proto::model::SessionItem = serde_json::from_value(serde_json::json!({
            "id": consort_proto::ids::ItemId::new(),
            "session_id": session,
            "turn_id": "turn-1",
            "kind": "command",
            "payload": {"command": "cargo build", "cwd": null, "output": "x".repeat(10_000), "exit_code": null, "duration_ms": null},
            "status": "in_progress",
            "created_at": NOW,
            "updated_at": NOW
        }))
        .expect("item");
        assert!(w.cache.apply(FeedEntry::upsert(
            1,
            Timestamp::from_millis(NOW),
            EntityPayload::SessionItem(item)
        )));
        w.cache.pending.push(PendingSend {
            client_message_id: "c-1".into(),
            channel_id: w.sayso.to_string(),
            thread_root_id: None,
            body: "Hi".into(),
            created_at: NOW,
            attachments: Vec::new(),
        });
        w.cache.has_older.insert(w.sayso.to_string(), true);
        w.cache.channels_loaded.insert(w.sayso.to_string());
        w.cache.loaded_threads.insert(w.card.to_string());
        assert!(w.cache.apply(FeedEntry::delete(
            2,
            Timestamp::from_millis(NOW),
            consort_proto::client_api::EntityKind::Channel,
            w.sayso.to_string(),
            Some(w.sayso),
        )));
        let c = &w.cache;
        assert_eq!(c.channels.len(), 1, "only Tutti is left");
        assert!(c.messages.is_empty(), "{:?}", c.messages.len());
        assert!(c.tasks.is_empty());
        assert!(c.approvals.is_empty());
        assert!(c.decisions.is_empty());
        assert!(c.read_markers.is_empty());
        assert!(c.session_items.is_empty());
        assert!(c.pending.is_empty());
        assert!(c.has_older.is_empty());
        assert!(c.channels_loaded.is_empty());
        assert!(c.loaded_threads.is_empty());
        let state = view(&w.cache);
        assert!(state.tasks.is_empty());
        assert_eq!(state.tasks_panel.need_you, 0);
        assert!(state.timeline(&w.sayso.to_string()).is_none());
    }

    /// A session item keeps a command line and not the output.
    #[test]
    fn a_session_item_keeps_only_what_the_view_reads() {
        let mut w = world();
        let session = w
            .cache
            .tasks
            .get(&w.task.to_string())
            .and_then(|t| t.current_session_id)
            .expect("session");
        let item: consort_proto::model::SessionItem = serde_json::from_value(serde_json::json!({
            "id": consort_proto::ids::ItemId::new(),
            "session_id": session,
            "turn_id": "turn-1",
            "kind": "command",
            "payload": {"command": "cargo test", "cwd": null, "output": "x".repeat(10_000), "exit_code": 0, "duration_ms": 5},
            "status": "completed",
            "created_at": NOW,
            "updated_at": NOW
        }))
        .expect("item");
        w.cache.apply(FeedEntry::upsert(
            1,
            Timestamp::from_millis(NOW),
            EntityPayload::SessionItem(item),
        ));
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        let PostBody::Activity { summary, items } = &thread.posts[0].bodies[0] else {
            panic!("no activity line");
        };
        assert_eq!(summary, "Ran 1 command");
        assert_eq!(items, &vec!["cargo test".to_owned()]);
    }

    /// Finding 14: the detail block names the real action, whatever the
    /// summary says.
    #[test]
    fn the_detail_block_names_the_tool_the_server_and_the_network_target() {
        let mut w = world();
        let id = coordinator_approval(&mut w);
        let detail = |w: &World| {
            view(&w.cache)
                .timeline(&w.tutti.to_string())
                .and_then(|t| {
                    t.items.iter().find_map(|i| match i {
                        TimelineItem::Approval(card) => Some(card.approval.clone()),
                        _ => None,
                    })
                })
                .expect("card")
        };
        // A tool with a readable, agent-written summary.
        if let Some(a) = w.cache.approvals.get_mut(&id.to_string()) {
            a.summary = "Look up one harmless record".into();
            a.details = ApprovalDetails::Tool {
                server: Some("payments".into()),
                tool: "mcp__payments__refund_all".into(),
                arguments: serde_json::json!({"scope": "everyone"}),
            };
        }
        let card = detail(&w);
        assert_eq!(card.summary, "Look up one harmless record");
        let text = card.detail.expect("detail");
        assert!(text.contains("Tool: refund_all"), "{text}");
        assert!(text.contains("Server: payments"), "{text}");
        assert!(text.contains("\"scope\": \"everyone\""), "{text}");

        if let Some(a) = w.cache.approvals.get_mut(&id.to_string()) {
            a.summary = "Fetch the docs".into();
            a.details = ApprovalDetails::Network {
                target: "evil.example.com".into(),
            };
        }
        let card = detail(&w);
        assert_eq!(card.summary, "Fetch the docs");
        assert_eq!(card.detail.as_deref(), Some("Network: evil.example.com"));

        if let Some(a) = w.cache.approvals.get_mut(&id.to_string()) {
            a.details = ApprovalDetails::Other {
                text: Some("Delete the build folder".into()),
            };
        }
        assert_eq!(
            detail(&w).detail.as_deref(),
            Some("Delete the build folder")
        );
    }

    /// Finding 24: Deny takes `deny_once` before `deny_always`, whatever the
    /// order of the options.
    #[test]
    fn deny_prefers_deny_once_in_any_order() {
        let mut w = world();
        let id = coordinator_approval(&mut w);
        let option = |id: &str, kind| RequestOption {
            id: id.into(),
            label: id.into(),
            kind,
            description: None,
        };
        if let Some(a) = w.cache.approvals.get_mut(&id.to_string()) {
            a.options = vec![
                option("never", RequestOptionKind::DenyAlways),
                option("no", RequestOptionKind::DenyOnce),
                option("yes", RequestOptionKind::AllowOnce),
            ];
        }
        let stored = w.cache.approvals.get(&id.to_string()).expect("stored");
        assert_eq!(
            crate::hub::option_for(stored, ApprovalChoice::Deny).as_deref(),
            Some("no")
        );
        let mut only_always = stored.clone();
        only_always.options = vec![option("never", RequestOptionKind::DenyAlways)];
        assert_eq!(
            crate::hub::option_for(&only_always, ApprovalChoice::Deny).as_deref(),
            Some("never")
        );
    }

    /// Finding 18: archived tasks have no thread, and the view of a large
    /// cache stays correct with the indexes.
    #[test]
    fn an_archived_task_has_no_thread_view() {
        let mut w = world();
        if let Some(t) = w.cache.tasks.get_mut(&w.task.to_string()) {
            t.archived_at = Some(Timestamp::from_millis(NOW));
        }
        let state = view(&w.cache);
        assert!(state.thread(&w.task.to_string()).is_none());
        assert!(state.task(&w.task.to_string()).is_none());
    }

    fn notice(w: &World, root: Option<MessageId>, body: &str, at: i64) -> Message {
        let mut m = message(
            MessageId::new(),
            w.sayso,
            Author::System,
            MessageKind::Notice,
            root,
            at,
        );
        m.body = body.to_owned();
        m
    }

    /// Finding 3 of review cycle 2: a notice body is JSON, and the timeline
    /// showed it raw as Markdown under "Consort".
    #[test]
    fn a_notice_shows_its_text_and_kind_and_is_not_unread() {
        let mut w = world();
        let before = view(&w.cache).sidebar.groups[0].channels[0].unread;
        let n = notice(
            &w,
            None,
            r#"{"kind":"turn_failed","text":"The turn failed. Try again."}"#,
            NOW - 10_000,
        );
        w.cache.messages.insert(n.id.to_string(), n.clone());
        let state = view(&w.cache);
        let timeline = state.timeline(&w.sayso.to_string()).expect("timeline");
        let found = timeline
            .items
            .iter()
            .find_map(|i| match i {
                TimelineItem::Notice(n) => Some(n),
                _ => None,
            })
            .expect("a notice item");
        assert_eq!(found.kind, "turn_failed");
        assert_eq!(found.text, "The turn failed. Try again.");
        assert_eq!(found.message_id, n.id.to_string());
        assert!(
            !timeline
                .items
                .iter()
                .any(|i| matches!(i, TimelineItem::Message(m) if m.body.contains("turn_failed"))),
            "no raw JSON in a message"
        );
        assert_eq!(
            state.sidebar.groups[0].channels[0].unread, before,
            "a notice is not an unread message"
        );
    }

    #[test]
    fn a_notice_that_does_not_parse_shows_its_raw_body() {
        assert_eq!(
            notice_parts("The assistant is not available."),
            (String::new(), "The assistant is not available.".to_owned())
        );
        // JSON with no text, or with an empty text, is shown as it is too.
        assert_eq!(
            notice_parts(r#"{"kind":"x"}"#),
            (String::new(), r#"{"kind":"x"}"#.to_owned())
        );
        assert_eq!(
            notice_parts(r#"{"kind":"x","text":"  "}"#),
            (String::new(), r#"{"kind":"x","text":"  "}"#.to_owned())
        );
        // An unknown extra field does not matter.
        assert_eq!(
            notice_parts(r#"{"kind":"message_not_sent","text":"Not sent.","future":1}"#),
            ("message_not_sent".to_owned(), "Not sent.".to_owned())
        );
    }

    #[test]
    fn a_notice_in_a_thread_is_a_notice_post() {
        let mut w = world();
        let n = notice(
            &w,
            Some(w.card),
            r#"{"kind":"answer_not_delivered","text":"Your answer did not reach the task."}"#,
            NOW - 5_000,
        );
        w.cache.messages.insert(n.id.to_string(), n);
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        let post = thread.posts.last().expect("a post");
        assert_eq!(post.speaker, Speaker::System);
        assert_eq!(
            post.bodies,
            vec![PostBody::Notice {
                kind: "answer_not_delivered".into(),
                text: "Your answer did not reach the task.".into(),
            }]
        );
    }

    /// Finding 7 of review cycle 2: a channel that never loaded showed as
    /// empty, and the first live message looked like the newest one.
    #[test]
    fn a_timeline_is_loaded_only_after_its_channel_snapshot() {
        let mut w = world();
        let id = w.sayso.to_string();
        assert!(!view(&w.cache).timeline(&id).expect("timeline").loaded);
        w.cache.channels_loaded.insert(id.clone());
        assert!(view(&w.cache).timeline(&id).expect("timeline").loaded);
        assert!(
            !view(&w.cache)
                .timeline(&w.tutti.to_string())
                .expect("timeline")
                .loaded
        );
    }

    /// Finding 9 of review cycle 2: the reason of a Failed or Up next task
    /// was only in the card.
    #[test]
    fn a_failed_or_up_next_task_has_its_reason_in_the_thread() {
        let mut w = world();
        let reason = "Code signing failed: no identity matches `Developer ID`. Install the certificate, then retry.";
        let id = w.task.to_string();
        for (status, expected) in [
            (PS::Failed, Some(reason)),
            (PS::UpNext, Some(reason)),
            (PS::Working, None),
            (PS::Review, None),
        ] {
            if let Some(t) = w.cache.tasks.get_mut(&id) {
                t.status = status;
                t.status_reason = Some(reason.to_owned());
            }
            let state = view(&w.cache);
            assert_eq!(
                state.thread(&id).expect("thread").reason.as_deref(),
                expected,
                "{status:?}"
            );
        }
    }

    #[test]
    fn a_send_failure_names_its_task_and_says_what_to_do() {
        let mut w = world();
        w.cache.send_failures.push(crate::cache::SendFailure {
            id: "c-9".into(),
            channel_id: w.sayso.to_string(),
            thread_root_id: Some(w.card.to_string()),
            body: "Please retry".into(),
            channel_name: String::new(),
            message: "That task does not exist".into(),
        });
        let state = view(&w.cache);
        let failure = &state.send_failures[0];
        assert_eq!(
            failure.task_id.as_deref(),
            Some(w.task.to_string().as_str())
        );
        assert_eq!(failure.channel_name, "sayso");
        assert_eq!(failure.body, "Please retry");
        assert!(failure.message.starts_with("That task does not exist."));
        assert!(failure.message.contains("Put your text back"));
    }

    /// Review cycle 3, finding 1: the thread had no brief and no review
    /// summary, so the user could not read either one in full.
    #[test]
    fn the_thread_holds_the_full_brief_and_the_full_review_summary() {
        let mut w = world();
        let brief = "Fix the flaky signing step.\n\n- Check the CI logs.\n- Do not touch the release branch.\n\nThis line sits far beyond a short preview. ".repeat(40);
        let summary = (1..=8)
            .map(|n| format!("Paragraph {n} of the review summary says what changed."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let task = w.cache.tasks.get_mut(&w.task.to_string()).expect("task");
        task.status = PS::Review;
        task.brief = brief.clone();
        task.review_summary = Some(summary.clone());
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        assert_eq!(thread.brief, brief);
        assert_eq!(thread.review_summary.as_deref(), Some(summary.as_str()));
        assert!(!thread.text_is_preview);
        assert_eq!(thread.status, TaskStatus::Review);
        // The card keeps the whole summary too, and the card clamps it.
        assert!(format!("{state:?}").contains("Paragraph 8"));
    }

    #[test]
    fn a_thread_says_when_it_holds_only_a_preview() {
        let mut w = world();
        w.cache.task_previews.insert(w.task.to_string());
        let state = view(&w.cache);
        assert!(
            state
                .thread(&w.task.to_string())
                .expect("thread")
                .text_is_preview
        );
        // An empty summary is no summary.
        let task = w.cache.tasks.get_mut(&w.task.to_string()).expect("task");
        task.review_summary = Some("  \n".into());
        let state = view(&w.cache);
        assert_eq!(
            state
                .thread(&w.task.to_string())
                .expect("thread")
                .review_summary,
            None
        );
    }

    /// Review cycle 3, S2: the card and the thread show the approval mode
    /// that the task asked for.
    #[test]
    fn the_card_and_the_thread_carry_the_approval_mode_of_the_task() {
        let mut w = world();
        let state = view(&w.cache);
        let id = w.task.to_string();
        assert_eq!(state.task(&id).expect("task").approval_mode, VMode::Ask);
        assert_eq!(state.thread(&id).expect("thread").approval_mode, VMode::Ask);
        let card = state
            .timeline(&w.sayso.to_string())
            .expect("timeline")
            .items
            .iter()
            .find_map(|i| match i {
                TimelineItem::TaskCard(c) => Some(c.clone()),
                _ => None,
            })
            .expect("card");
        assert_eq!(card.approval_mode, VMode::Ask);
        let task = w.cache.tasks.get_mut(&id).expect("task");
        task.approval_mode = model::ApprovalMode::FullAccess;
        let state = view(&w.cache);
        assert_eq!(
            state.thread(&id).expect("thread").approval_mode,
            VMode::FullAccess
        );
        assert_eq!(VMode::FullAccess.tag(), "Full access mode");
        assert_eq!(VMode::Ask.tag(), "Ask mode");
    }

    fn claude_approval(w: &mut World) -> ApprovalId {
        let id = *w
            .cache
            .approvals
            .keys()
            .next()
            .map(|k| k.parse::<ApprovalId>())
            .transpose()
            .expect("id")
            .as_ref()
            .expect("an approval");
        let approval = w
            .cache
            .approvals
            .get_mut(&id.to_string())
            .expect("approval");
        approval.details = ApprovalDetails::Command {
            command: "python3 build.py".into(),
            cwd: Some("/work/app".into()),
        };
        approval.options = vec![
            RequestOption {
                id: "allow".into(),
                label: "Yes".into(),
                kind: RequestOptionKind::AllowOnce,
                description: None,
            },
            RequestOption {
                id: "allow-always".into(),
                label: "Yes, and don't ask again for python3 commands".into(),
                kind: RequestOptionKind::AllowAlways,
                description: None,
            },
            RequestOption {
                id: "deny".into(),
                label: "No".into(),
                kind: RequestOptionKind::DenyOnce,
                description: None,
            },
        ];
        id
    }

    /// Review cycle 3, S4: "Allow for this task" sent claude-agent-acp's
    /// option that adds a permanent Claude rule, and the harness label that
    /// says so was thrown away.
    #[test]
    fn the_wide_choice_shows_the_harness_label_so_the_user_sees_the_scope() {
        let mut w = world();
        let id = claude_approval(&mut w);
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        let approval = thread
            .posts
            .iter()
            .flat_map(|p| p.bodies.iter())
            .find_map(|b| match b {
                PostBody::Approval(a) if a.id == id.to_string() => Some(a.clone()),
                _ => None,
            })
            .expect("approval");
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowForTask),
            "Yes, and don't ask again for python3 commands"
        );
        // Allow once and Deny keep our words.
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowOnce),
            "Allow once"
        );
        assert_eq!(approval.choice_label(ApprovalChoice::Deny), "Deny");
        // The button still sends the option that carries that label.
        let option = crate::hub::option_for(
            w.cache.approvals.get(&id.to_string()).expect("approval"),
            ApprovalChoice::AllowForTask,
        );
        assert_eq!(option.as_deref(), Some("allow-always"));
    }

    #[test]
    fn without_a_harness_label_the_wide_choice_keeps_our_words() {
        let mut approval = ApprovalView {
            task_id: Some("tsk_1".into()),
            ..ApprovalView::default()
        };
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowForTask),
            "Allow for this task"
        );
        approval.task_id = None;
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowForTask),
            "Always allow"
        );
        // A label of white space is no label.
        approval.allow_always_label = Some("  ".into());
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowForTask),
            "Always allow"
        );
        approval.allow_always_label = Some(" Yes, for the session ".into());
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowForTask),
            "Yes, for the session"
        );
    }

    /// Review cycle 3, finding 7: nothing listed the channels that the user
    /// archived, so there was no way back.
    #[test]
    fn archived_channels_are_listed_by_name_and_leave_the_channel_list() {
        let mut w = world();
        let mut old = channel(ChannelId::new(), ChannelKind::Standard, "zebra", None);
        old.archived_at = Some(Timestamp::from_millis(NOW - 5_000));
        let mut older = channel(ChannelId::new(), ChannelKind::Standard, "Apple", None);
        older.archived_at = Some(Timestamp::from_millis(NOW - 9_000));
        older.icon = Some("mic".into());
        let (old_id, older_id) = (old.id.to_string(), older.id.to_string());
        w.cache.channels.insert(old_id.clone(), old);
        w.cache.channels.insert(older_id.clone(), older);
        let state = view(&w.cache);
        assert!(state.channel(&old_id).is_none());
        assert!(state.timeline(&old_id).is_none());
        let names: Vec<&str> = state
            .archived_channels
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["Apple", "zebra"]);
        assert_eq!(state.archived_channels[0].id, older_id);
        assert_eq!(state.archived_channels[0].icon, "mic");
        assert_eq!(state.archived_channels[1].icon, "hash");
        // Open channels are not in the list.
        assert!(
            state
                .archived_channels
                .iter()
                .all(|c| c.id != w.sayso.to_string())
        );
    }

    /// Review cycle 3, finding 12: the messages that waited for a channel
    /// that someone deleted were dropped with no trace.
    #[test]
    fn a_message_that_waited_for_a_deleted_channel_becomes_a_failed_send() {
        let mut w = world();
        w.cache.pending.push(PendingSend {
            client_message_id: "cm-1".into(),
            channel_id: w.sayso.to_string(),
            thread_root_id: None,
            body: "A long message that the user wrote offline".into(),
            created_at: NOW,
            attachments: Vec::new(),
        });
        w.cache.pending.push(PendingSend {
            client_message_id: "cm-2".into(),
            channel_id: w.tutti.to_string(),
            thread_root_id: None,
            body: "Stays".into(),
            created_at: NOW,
            attachments: Vec::new(),
        });
        w.cache.seq = 5;
        let changed = w.cache.apply(FeedEntry {
            seq: 6,
            at: Timestamp::from_millis(NOW),
            channel_id: Some(w.sayso),
            entity_id: w.sayso.to_string(),
            change: FeedChange::Delete(EntityKind::Channel),
        });
        assert!(changed);
        assert_eq!(w.cache.pending.len(), 1);
        assert_eq!(w.cache.pending[0].client_message_id, "cm-2");
        assert_eq!(w.cache.send_failures.len(), 1);
        let failure = &w.cache.send_failures[0];
        assert_eq!(failure.id, "cm-1");
        assert_eq!(failure.body, "A long message that the user wrote offline");
        assert_eq!(failure.message, "The channel was deleted.");
        // The view names the channel, although it is gone.
        let state = view(&w.cache);
        let shown = &state.send_failures[0];
        assert_eq!(shown.channel_name, "sayso");
        assert!(shown.message.starts_with("The channel was deleted."));
        assert!(shown.message.contains("Put your text back"));
        assert_eq!(shown.body, "A long message that the user wrote offline");
    }

    mod accounts {
        use consort_proto::{
            ids::{AccountId, SignInId, WorkerId},
            model::{
                AcpRegistryAgent, AuthState as PA, Capabilities, HarnessAccount, Inventory,
                ModelInfo, ModelSelection, OptionValue, Os as POs, SignIn, SignInMethod,
                SignInState, ToolStatus, WorkerPolicy,
            },
        };

        use super::*;

        fn account(label: &str, harness: &str, auth: PA) -> HarnessAccount {
            HarnessAccount {
                account_id: AccountId::new(),
                harness: harness.into(),
                label: label.into(),
                harness_version: Some("2.1.312".into()),
                auth_state: auth,
                auth_detail: None,
                models: vec![],
                capabilities: Capabilities::default(),
                enabled: true,
                profile: true,
                email: None,
                plan: None,
                secret_names: vec![],
                sign_in_methods: vec![],
                extras_enabled: true,
                extras: None,
            }
        }

        fn worker(name: &str, user: &[&str], detected: &[&str]) -> model::Worker {
            model::Worker {
                id: WorkerId::new(),
                name: name.into(),
                hostname: name.into(),
                os: POs::Windows,
                arch: "x64".into(),
                public_key: consort_proto::auth::PublicKey::from_bytes([1; 32]),
                version: "0.1.0".into(),
                protocol_version: 1,
                tags: user.iter().map(|t| (*t).to_owned()).collect(),
                paused: false,
                state: WorkerState::Online,
                last_seen_at: None,
                inventory: Some(Inventory {
                    hash: String::new(),
                    hostname: name.into(),
                    os: POs::Windows,
                    arch: "x64".into(),
                    tags: detected.iter().map(|t| (*t).to_owned()).collect(),
                    accounts: vec![],
                    local_mcp_servers: vec![],
                    checkouts: vec![],
                    policy: WorkerPolicy::default(),
                    load: Default::default(),
                    tools: vec![],
                }),
                paired_at: Timestamp::from_millis(NOW),
                revoked_at: None,
            }
        }

        fn inventory(w: &mut model::Worker) -> &mut Inventory {
            w.inventory.as_mut().expect("inventory")
        }

        fn row<'a>(state: &'a ViewState, label: &str) -> &'a HarnessAccountView {
            state
                .workers
                .iter()
                .flat_map(|w| w.accounts.iter())
                .find(|a| a.label == label)
                .expect("account row")
        }

        #[test]
        fn account_rows_show_the_plan_or_the_problem_and_one_action() {
            let mut w = world();
            let mut bigboi = worker("bigboi", &["personal"], &["windows"]);
            let mut personal = account("Claude Personal", "claude", PA::SignedIn);
            personal.plan = Some("Claude Max".into());
            personal.sign_in_methods = vec![SignInMethod::ClaudePlan];
            let mut work = account("Claude Work", "claude", PA::Error);
            work.auth_detail = Some("The login expired on Oct 27".into());
            work.sign_in_methods = vec![SignInMethod::ClaudePlan];
            let mut fireworks = account("OpenCode with Fireworks", "opencode", PA::SignedIn);
            fireworks.secret_names = vec!["FIREWORKS_API_KEY".into()];
            let mut amp = account("Amp", "acp:amp-acp", PA::SignedOut);
            amp.sign_in_methods = vec![SignInMethod::Terminal {
                command: "amp login".into(),
            }];
            // An account written by hand in worker.toml offers no method.
            let by_hand = account("Pi with Kimi", "pi", PA::SignedOut);
            // The worker adds the step to Settings > Workers, and the row is
            // in Settings > Workers already.
            let mut signed_out = account("Claude Spare", "claude", PA::SignedOut);
            signed_out.auth_detail =
                Some("Not signed in. Sign in to this account in Settings > Workers.".into());
            signed_out.sign_in_methods = vec![SignInMethod::ClaudePlan];
            let mut step_only = account("Claude Step", "claude", PA::SignedOut);
            step_only.auth_detail = Some("Sign in to this account in Settings > Workers.".into());
            inventory(&mut bigboi).accounts = vec![
                personal, work, fireworks, amp, by_hand, signed_out, step_only,
            ];
            w.cache.workers.insert(bigboi.id.to_string(), bigboi);
            let state = view(&w.cache);

            let personal = row(&state, "Claude Personal");
            assert_eq!(personal.logo, ProviderLogo::ClaudeCode);
            assert_eq!(personal.detail, "Claude Max plan.");
            assert_eq!(personal.status, AccountStatus::SignedIn);
            assert_eq!(personal.action, AccountAction::SignOut);

            let work = row(&state, "Claude Work");
            assert_eq!(work.status, AccountStatus::Expired);
            assert_eq!(work.action, AccountAction::SignInAgain);
            assert_eq!(work.detail, "The login expired on Oct 27.");

            let fireworks = row(&state, "OpenCode with Fireworks");
            assert_eq!(fireworks.status, AccountStatus::Ready);
            assert_eq!(fireworks.detail, "API key saved on bigboi.");
            assert_eq!(fireworks.action, AccountAction::None);

            let amp = row(&state, "Amp");
            assert_eq!(amp.status, AccountStatus::NotSignedIn);
            assert_eq!(amp.action, AccountAction::SignIn);
            assert_eq!(
                amp.detail,
                "ACP agent from the registry. Not signed in yet."
            );
            assert!(
                matches!(&amp.logo, ProviderLogo::Registry { id, svg: None } if id == "amp-acp")
            );

            assert_eq!(
                state.account_logo("Claude Personal"),
                Some(&ProviderLogo::ClaudeCode),
                "a task account shows its provider logo"
            );
            assert_eq!(state.account_logo("No such account"), None);

            let spare = row(&state, "Claude Spare");
            assert_eq!(spare.detail, "Not signed in.");
            assert_eq!(spare.status, AccountStatus::NotSignedIn);
            assert_eq!(
                row(&state, "Claude Step").detail,
                "Not signed in yet.",
                "a reason that is only the step has nothing to show"
            );

            let by_hand = row(&state, "Pi with Kimi");
            assert_eq!(
                by_hand.action,
                AccountAction::None,
                "no method to sign in from the app"
            );
        }

        #[test]
        fn an_account_whose_tool_installs_says_so() {
            let mut w = world();
            let mut bigboi = worker("bigboi", &[], &[]);
            inventory(&mut bigboi).accounts = vec![account("Gemini", "gemini", PA::SignedOut)];
            inventory(&mut bigboi).tools = vec![ToolStatus {
                tool: ToolId::gemini_cli(),
                version: None,
                latest: Some("0.66.0".into()),
                state: ToolStateProto::Installing,
                detail: None,
            }];
            w.cache.workers.insert(bigboi.id.to_string(), bigboi);
            let state = view(&w.cache);
            let gemini = row(&state, "Gemini");
            assert_eq!(gemini.status, AccountStatus::Installing);
            assert_eq!(gemini.detail, "Installing Gemini CLI.");
        }

        #[test]
        fn tools_list_the_providers_first_with_their_state() {
            let mut w = world();
            let mut bigboi = worker("bigboi", &[], &[]);
            let tool = |id: &str, version: Option<&str>, latest: Option<&str>, state| ToolStatus {
                tool: ToolId::new(id),
                version: version.map(Into::into),
                latest: latest.map(Into::into),
                state,
                detail: None,
            };
            inventory(&mut bigboi).tools = vec![
                tool("node", Some("24.11.0"), None, ToolStateProto::Ready),
                tool("gemini-cli", None, None, ToolStateProto::Missing),
                tool(
                    "codex",
                    Some("0.164.0"),
                    Some("0.165.0"),
                    ToolStateProto::UpdateAvailable,
                ),
                tool("claude-code", Some("2.1.312"), None, ToolStateProto::Ready),
            ];
            w.cache.workers.insert(bigboi.id.to_string(), bigboi);
            let state = view(&w.cache);
            let tools = &state.workers[0].tools;
            let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert_eq!(names, ["Claude Code", "Codex", "Gemini CLI", "Node.js"]);
            assert_eq!(tools[0].action_label(), "Up to date");
            assert_eq!(tools[1].action_label(), "Update to 0.165.0");
            assert!(tools[1].can_install());
            assert_eq!(tools[2].action_label(), "Install now");
            assert_eq!(
                tools[2].detail.as_deref(),
                Some("Not installed. bigboi installs it when an account needs it.")
            );
            assert_eq!(
                state.workers[0].tools_folder,
                r"%USERPROFILE%\.consort\tools"
            );
        }

        #[test]
        fn tags_list_user_tags_first_and_detected_tags_apart() {
            let mut w = world();
            let bigboi = worker(
                "bigboi",
                &["windows-test", "personal"],
                &["windows", "personal", "gpu-nvidia"],
            );
            let mac = worker("MacBook Pro", &["work"], &["macos", "arm64"]);
            w.cache.workers.insert(bigboi.id.to_string(), bigboi);
            w.cache.workers.insert(mac.id.to_string(), mac);
            let state = view(&w.cache);
            assert_eq!(
                state.tags,
                [
                    "personal",
                    "windows-test",
                    "work",
                    "arm64",
                    "gpu-nvidia",
                    "macos",
                    "windows"
                ]
            );
            let bigboi = state
                .workers
                .iter()
                .find(|w| w.name == "bigboi")
                .expect("worker");
            assert_eq!(
                bigboi.detected_tags,
                ["windows", "gpu-nvidia"],
                "a user tag is not repeated"
            );
            let wanted = vec!["personal".to_owned(), "gpu-nvidia".to_owned()];
            let names: Vec<&str> = state
                .workers_with_tags(&wanted)
                .iter()
                .map(|w| w.name.as_str())
                .collect();
            assert_eq!(names, ["bigboi"]);
            assert_eq!(state.workers_with_tags(&[]).len(), 2);
        }

        #[test]
        fn the_status_line_names_the_architecture_like_the_detected_tags() {
            let mut w = world();
            for (arch, label) in [
                ("aarch64", "arm64"),
                ("x86_64", "x64"),
                ("riscv64", "riscv64"),
            ] {
                let mut worker = worker("MacBook Pro", &[], &["macos", "arm64"]);
                worker.arch = arch.into();
                w.cache.workers.clear();
                w.cache.workers.insert(worker.id.to_string(), worker);
                let state = view(&w.cache);
                assert_eq!(
                    state.workers[0].status_line,
                    format!("Online. Windows, {label}, worker 0.1.0."),
                    "{arch}"
                );
            }
        }

        #[test]
        fn a_worker_suggests_only_tags_that_it_does_not_have() {
            let mut w = world();
            let bigboi = worker("bigboi", &["personal"], &["windows", "x64", "hub"]);
            let mac = worker("MacBook Pro", &["work"], &["macos", "arm64"]);
            w.cache.workers.insert(bigboi.id.to_string(), bigboi);
            w.cache.workers.insert(mac.id.to_string(), mac);
            let state = view(&w.cache);
            let bigboi = state
                .workers
                .iter()
                .find(|w| w.name == "bigboi")
                .expect("worker");
            // Not its user tag, and not its detected tags (the `hub` tag too).
            assert_eq!(
                bigboi.suggested_tags(&state.tags),
                ["work", "arm64", "macos"]
            );
            let tutti = state.tags.clone();
            let mut all_taken = bigboi.clone();
            all_taken.tags = tutti;
            assert!(all_taken.suggested_tags(&state.tags).is_empty());
        }

        #[test]
        fn a_channel_shows_its_tags() {
            let mut w = world();
            if let Some(channel) = w.cache.channels.get_mut(&w.sayso.to_string()) {
                channel.tags = vec!["work".into()];
            }
            let state = view(&w.cache);
            assert_eq!(
                state.channel(&w.sayso.to_string()).map(|c| c.tags.clone()),
                Some(vec!["work".to_owned()])
            );
        }

        #[test]
        fn selections_keep_the_model_id_and_show_the_name() {
            let mut w = world();
            let mut bigboi = worker("bigboi", &[], &[]);
            let mut claude = account("Claude Personal", "claude", PA::SignedIn);
            claude.models = vec![ModelInfo {
                id: "claude-opus-5-5".into(),
                name: "Opus 5.5".into(),
                options: vec![consort_proto::model::OptionDescriptor::Select {
                    id: "effort".into(),
                    label: "Effort".into(),
                    choices: ["low", "high"]
                        .iter()
                        .map(|v| consort_proto::model::OptionChoice {
                            value: (*v).into(),
                            label: v.to_uppercase(),
                        })
                        .collect(),
                    default: Some("low".into()),
                }],
            }];
            inventory(&mut bigboi).accounts = vec![claude];
            w.cache.workers.insert(bigboi.id.to_string(), bigboi);
            let chosen = ModelSelection {
                account_label: "Claude Personal".into(),
                model: "claude-opus-5-5".into(),
                options: [("effort".to_owned(), OptionValue::Text("high".into()))]
                    .into_iter()
                    .collect(),
            };
            if let Some(channel) = w.cache.channels.get_mut(&w.sayso.to_string()) {
                channel.task_selection = Some(chosen.clone());
            }
            let state = view(&w.cache);
            let group = &state.model_groups[0];
            assert_eq!(group.account_label, "Claude Personal");
            assert_eq!(group.workers, ["bigboi"]);
            assert_eq!(group.models[0].id, "claude-opus-5-5");
            assert_eq!(group.models[0].name, "Opus 5.5");
            let chosen_view = state
                .channel(&w.sayso.to_string())
                .and_then(|c| c.task_selection.clone())
                .expect("selection");
            assert_eq!(chosen_view.label(), "Claude Personal, Opus 5.5");
            assert_eq!(chosen_view.model_id, "claude-opus-5-5");
            assert_eq!(
                chosen_view.options,
                [("effort".to_owned(), SelectionOption::Text("high".into()))]
            );
            assert_eq!(
                state.effort_for(&chosen_view).map(|e| e.levels.len()),
                Some(2)
            );
        }

        #[test]
        fn a_channel_without_its_own_model_uses_tutti_and_says_so() {
            let mut w = world();
            let sel = |model: &str| ModelSelection {
                account_label: "Claude Personal".into(),
                model: model.into(),
                options: Default::default(),
            };
            for channel in w.cache.channels.values_mut() {
                channel.coordinator_selection = match channel.kind {
                    PK::Tutti => Some(sel("opus")),
                    PK::Standard => None,
                };
            }
            let state = view(&w.cache);
            let tutti = state.tutti().unwrap();
            let own = tutti.coordinator_selection.clone().unwrap();
            assert_eq!(own.source, SelectionSource::Channel);
            assert_eq!(own.selection.model_id, "opus");
            let sayso = state.channel(&w.sayso.to_string()).unwrap();
            let inherited = sayso.coordinator_selection.clone().unwrap();
            assert_eq!(inherited.source, SelectionSource::Tutti);
            assert_eq!(inherited.selection.model_id, "opus");

            if let Some(channel) = w.cache.channels.get_mut(&w.sayso.to_string()) {
                channel.coordinator_selection = Some(sel("sonnet"));
            }
            let state = view(&w.cache);
            let sayso = state.channel(&w.sayso.to_string()).unwrap();
            let own = sayso.coordinator_selection.clone().unwrap();
            assert_eq!(own.source, SelectionSource::Channel);
            assert_eq!(own.selection.model_id, "sonnet");
        }

        #[test]
        fn a_signed_out_account_is_not_in_the_picker() {
            let mut w = world();
            let mut bigboi = worker("bigboi", &[], &[]);
            let mut out = account("Claude Work", "claude", PA::SignedOut);
            out.models = vec![ModelInfo {
                id: "opus".into(),
                name: "Opus".into(),
                options: vec![],
            }];
            inventory(&mut bigboi).accounts = vec![out];
            w.cache.workers.insert(bigboi.id.to_string(), bigboi);
            let state = view(&w.cache);
            assert!(
                state
                    .model_groups
                    .iter()
                    .all(|g| g.account_label != "Claude Work")
            );
        }

        fn sign_in(state: SignInState) -> SignIn {
            SignIn {
                id: SignInId::new(),
                worker_id: WorkerId::new(),
                account_id: AccountId::new(),
                method: SignInMethod::ClaudePlan,
                state,
                created_at: Timestamp::from_millis(NOW),
                updated_at: Timestamp::from_millis(NOW),
                expires_at: Timestamp::from_millis(NOW + 1),
            }
        }

        #[test]
        fn only_sign_ins_that_this_client_started_follow_the_feed() {
            let mut w = world();
            let other = sign_in(SignInState::Starting);
            assert!(
                !w.cache.apply(FeedEntry::upsert(
                    1,
                    Timestamp::from_millis(NOW),
                    EntityPayload::SignIn(other)
                )),
                "a sign-in from another client does not show"
            );
            let mine = sign_in(SignInState::Starting);
            let id = mine.id.to_string();
            w.cache.sign_ins.insert(id.clone(), mine.clone());
            let mut next = mine.clone();
            next.state = SignInState::NeedsCode {
                url: "https://claude.ai/oauth".into(),
            };
            assert!(w.cache.apply(FeedEntry::upsert(
                2,
                Timestamp::from_millis(NOW),
                EntityPayload::SignIn(next)
            )));
            let state = view(&w.cache);
            assert_eq!(state.sign_ins.len(), 1);
            assert_eq!(state.sign_ins[0].method, Some(SignInChoice::ClaudePlan));
            assert_eq!(
                state.sign_in(&id).map(|s| s.step.clone()),
                Some(SignInStep::NeedsCode {
                    url: "https://claude.ai/oauth".into()
                })
            );
            // A dismissed sign-in leaves, and a late entry does not bring it
            // back.
            w.cache.sign_ins.remove(&id);
            w.cache.sign_ins_dismissed.insert(id.clone());
            let mut late = mine;
            late.state = SignInState::Verifying;
            assert!(!w.cache.apply(FeedEntry::upsert(
                3,
                Timestamp::from_millis(NOW),
                EntityPayload::SignIn(late)
            )));
            assert!(view(&w.cache).sign_ins.is_empty());
        }

        #[test]
        fn a_reload_during_a_sign_in_shows_the_sign_in_from_the_snapshot() {
            let mut w = world();
            let running = sign_in(SignInState::NeedsCode {
                url: "https://claude.ai/oauth".into(),
            });
            let ended = sign_in(SignInState::Succeeded { detail: None });
            let id = running.id.to_string();
            w.cache.load_snapshot(consort_proto::client_api::Snapshot {
                seq: 4,
                sign_ins: vec![running.clone(), ended],
                ..Default::default()
            });
            let state = view(&w.cache);
            assert_eq!(state.sign_ins.len(), 1, "an ended sign-in does not show");
            assert_eq!(
                state.sign_in(&id).map(|s| s.step.clone()),
                Some(SignInStep::NeedsCode {
                    url: "https://claude.ai/oauth".into()
                })
            );
            // Its feed entries now update it.
            let mut next = running;
            next.state = SignInState::Verifying;
            assert!(w.cache.apply(FeedEntry::upsert(
                5,
                Timestamp::from_millis(NOW),
                EntityPayload::SignIn(next)
            )));
            assert_eq!(
                view(&w.cache).sign_in(&id).map(|s| s.step.clone()),
                Some(SignInStep::Verifying)
            );
        }

        #[test]
        fn a_sign_in_that_the_user_closed_stays_closed_after_a_reload() {
            let mut w = world();
            let running = sign_in(SignInState::Starting);
            w.cache.sign_ins_dismissed.insert(running.id.to_string());
            w.cache.load_snapshot(consort_proto::client_api::Snapshot {
                seq: 4,
                sign_ins: vec![running],
                ..Default::default()
            });
            assert!(view(&w.cache).sign_ins.is_empty());
        }

        #[test]
        fn registry_icons_reach_the_agents_and_the_accounts() {
            let mut w = world();
            w.cache.acp_registry = vec![AcpRegistryAgent {
                id: "amp-acp".into(),
                name: "Amp".into(),
                description: "An agent.".into(),
                version: "0.10.0".into(),
                icon_url: Some("https://cdn.example/amp.svg".into()),
                supported: true,
            }];
            w.cache.acp_registry_load = RegistryLoad::Loaded;
            w.cache
                .acp_icons
                .insert("amp-acp".into(), "<svg></svg>".into());
            let mut bigboi = worker("bigboi", &[], &[]);
            inventory(&mut bigboi).accounts = vec![account("Amp", "acp:amp-acp", PA::SignedOut)];
            w.cache.workers.insert(bigboi.id.to_string(), bigboi);
            let state = view(&w.cache);
            let logo = ProviderLogo::Registry {
                id: "amp-acp".into(),
                svg: Some("<svg></svg>".into()),
            };
            assert_eq!(state.acp_registry.load, RegistryLoad::Loaded);
            assert_eq!(state.acp_registry.agents[0].logo, logo);
            assert_eq!(row(&state, "Amp").logo, logo);
            assert_eq!(row(&state, "Amp").provider, "Amp");
        }
    }
}
