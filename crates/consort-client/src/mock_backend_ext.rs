//! The mock backend for connectors and skills. It
//! changes the mock view state the way the hub would change the real one.

use std::time::Duration;

use crate::{
    ClientError, Command, CommandOutput, SecretText,
    connectors::*,
    mock_extensions as mx,
    skills::*,
    view::{ApprovalChoice, RequestState, TimelineItem, ViewState},
};

/// A change that the mock makes a while after a command.
pub(crate) type Later = (Duration, Box<dyn FnOnce(&mut ViewState) + Send>);

/// How long the mock takes to finish a browser sign-in.
const MOCK_SIGN_IN: Duration = Duration::from_millis(2_500);

fn invalid(message: &str) -> ClientError {
    ClientError::Hub {
        code: "invalid".into(),
        message: message.into(),
    }
}

fn not_found(what: &str) -> ClientError {
    ClientError::Hub {
        code: "not_found".into(),
        message: format!("This {what} is gone. It may have been removed in another client."),
    }
}

/// The page of the mock catalog that starts at `offset`.
fn catalog_page(state: &mut ViewState, offset: usize) {
    let all = mx::catalog_services();
    let catalog = &mut state.connector_catalog;
    let found: Vec<ServiceView> =
        filter_services(&all, &catalog.search, catalog.category.as_deref())
            .into_iter()
            .cloned()
            .collect();
    let end = (offset + mx::CATALOG_PAGE).min(found.len());
    let page = found.get(offset..end).unwrap_or_default().to_vec();
    if offset == 0 {
        catalog.services = page;
    } else {
        catalog.services.extend(page);
    }
    catalog.next_cursor = (end < found.len()).then(|| end.to_string());
    catalog.load = LoadState::Loaded;
    catalog.more = LoadState::NotLoaded;
    let counts: Vec<(String, u32)> = catalog
        .services
        .iter()
        .map(|s| (s.slug.clone(), 0))
        .collect();
    for (slug, _) in counts {
        let n = state
            .connections
            .iter()
            .filter(|c| c.service == slug)
            .count();
        if let Some(s) = state
            .connector_catalog
            .services
            .iter_mut()
            .find(|s| s.slug == slug)
        {
            s.connected_count = u32::try_from(n).unwrap_or(u32::MAX);
        }
    }
}

/// A connection that the browser finishes later, the way Composio does.
#[allow(clippy::too_many_arguments)]
fn start_connection(
    state: &mut ViewState,
    id: &str,
    service: &str,
    name: &str,
    use_in: UseInView,
    approvals: ToolApprovalsView,
    browser: bool,
    later: &mut Vec<Later>,
) -> Option<String> {
    let service_view = mx::catalog_services()
        .into_iter()
        .find(|s| s.slug == service);
    let service_name = service_view
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| service.to_owned());
    let handle = handle_for(name, state, None);
    let redirect_url = browser.then(|| format!("https://connect.composio.dev/link/{id}"));
    state.connections.push(ConnectionView {
        id: id.to_owned(),
        name: name.to_owned(),
        handle,
        service: service.to_owned(),
        service_name: service_name.clone(),
        logo: mx::logo(service, &service_name),
        identity: None,
        use_in,
        approvals,
        status: if browser {
            ConnectionStatusView::Connecting {
                redirect_url: redirect_url.clone(),
                expires_at: crate::format::now_ms() + 10 * 60_000,
            }
        } else {
            ConnectionStatusView::Active
        },
        created_at: crate::format::now_ms(),
    });
    if browser {
        let id = id.to_owned();
        later.push((
            MOCK_SIGN_IN,
            Box::new(move |state: &mut ViewState| {
                if let Some(c) = state.connections.iter_mut().find(|c| c.id == id) {
                    c.status = ConnectionStatusView::Active;
                    c.identity = Some("chris@acme.dev".into());
                }
            }),
        ));
    }
    redirect_url
}

/// Marks a hub card as answered with Allow, and runs `f` on its card.
fn answer_card(state: &mut ViewState, approval_id: &str, f: impl Fn(&mut HubCardView)) {
    for timeline in &mut state.timelines {
        for item in &mut timeline.items {
            if let TimelineItem::Approval(card) = item
                && card.approval.id == approval_id
            {
                card.approval.state = RequestState::Answered;
                card.approval.answer = Some(ApprovalChoice::AllowOnce);
                if let Some(hub_card) = card.approval.card.as_mut() {
                    f(hub_card);
                }
            }
        }
    }
}

/// What the hub does when the user allows a hub card in the mock: the
/// connect card connects, and the install card installs.
pub(crate) fn after_card_answer(
    state: &mut ViewState,
    approval_id: &str,
    choice: ApprovalChoice,
    new_id: &str,
    later: &mut Vec<Later>,
) {
    if choice != ApprovalChoice::AllowOnce {
        return;
    }
    let card = state.timelines.iter().find_map(|t| {
        t.items.iter().find_map(|i| match i {
            TimelineItem::Approval(c) if c.approval.id == approval_id => {
                c.approval.card.as_deref().cloned()
            }
            _ => None,
        })
    });
    match card {
        Some(HubCardView::ConnectService(card)) if card.connection_id.is_none() => {
            let id = format!("con_{new_id}");
            start_connection(
                state,
                &id,
                &card.service,
                &card.name,
                card.use_in.clone(),
                card.approvals,
                true,
                later,
            );
            answer_card(state, approval_id, |c| {
                if let HubCardView::ConnectService(c) = c {
                    c.connection_id = Some(id.clone());
                }
            });
        }
        Some(HubCardView::SkillInstall(card)) if card.skill_id.is_none() => {
            let id = format!("skl_{new_id}");
            state.skills.push(SkillView {
                id: id.clone(),
                name: card.name.clone(),
                description: card.description.clone(),
                source: SkillSourceView::Git {
                    url: format!("https://{}", card.source),
                    path: None,
                    git_ref: None,
                },
                commit: card.commit.clone(),
                license: card.license.clone(),
                scripts: card.scripts.clone(),
                audits: card.audits.clone(),
                // A skill with scripts stays off until the user turns it on.
                enabled: card.scripts.is_empty(),
                use_in: card.use_in.clone(),
                file_count: 3,
                installed_label: "Today".into(),
                ..SkillView::default()
            });
            answer_card(state, approval_id, |c| {
                if let HubCardView::SkillInstall(c) = c {
                    c.skill_id = Some(id.clone());
                }
            });
        }
        // The hub starts the sign-in with the card's method and names it on
        // the card.
        Some(HubCardView::SignIn(card)) if card.sign_in_id.is_none() => {
            let Some(method) = card.method.clone() else {
                return;
            };
            let id = format!("sgn_{new_id}");
            let claude = matches!(
                method,
                crate::view::SignInChoice::ClaudePlan | crate::view::SignInChoice::ClaudeConsole
            );
            state.sign_ins.push(crate::view::SignInView {
                id: id.clone(),
                worker_id: card.worker_id.clone(),
                account_id: card.account_id.clone(),
                method: Some(method),
                step: if claude {
                    crate::view::SignInStep::NeedsCode {
                        url: "https://claude.ai/oauth/authorize?code=true&client_id=consort-mock"
                            .into(),
                    }
                } else {
                    crate::view::SignInStep::OpenUrl {
                        url: "https://example.com/sign-in".into(),
                    }
                },
            });
            if !claude {
                let (sign_in, worker, account) =
                    (id.clone(), card.worker_id.clone(), card.account_id.clone());
                later.push((
                    MOCK_SIGN_IN,
                    Box::new(move |state: &mut ViewState| {
                        crate::mock_backend::finish_sign_in(state, &sign_in, &worker, &account)
                    }),
                ));
            }
            answer_card(state, approval_id, |c| {
                if let HubCardView::SignIn(c) = c {
                    c.sign_in_id = Some(id.clone());
                }
            });
        }
        _ => {}
    }
}

fn set_disabled(list: &mut Vec<String>, id: &str, enabled: bool) {
    if enabled {
        list.retain(|x| x != id);
    } else if !list.iter().any(|x| x == id) {
        list.push(id.to_owned());
    }
}

/// Runs one connector or skill command on the mock state.
pub(crate) fn apply(
    state: &mut ViewState,
    command: Command,
    new_id: &str,
    later: &mut Vec<Later>,
) -> Result<CommandOutput, ClientError> {
    match command {
        Command::LoadComposioStatus => state.composio.load = LoadState::Loaded,
        Command::SetComposioKey {
            api_key: SecretText(key),
        } => {
            let key = key.trim().to_owned();
            if key.is_empty() {
                return Err(invalid("Paste your Composio project key."));
            }
            let tail: String = key
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            state.composio = ComposioView {
                key_hint: Some(format!("…{tail}")),
                key_present: true,
                ..mx::composio()
            };
        }
        Command::RemoveComposioKey => {
            state.composio = ComposioView {
                load: LoadState::Loaded,
                ..ComposioView::default()
            };
        }
        Command::LoadConnectorCategories => {
            state.connector_catalog.categories = mx::categories();
        }
        Command::SearchConnectorCatalog { search, category } => {
            state.connector_catalog.search = search;
            state.connector_catalog.category = category;
            catalog_page(state, 0);
        }
        Command::LoadMoreConnectorCatalog => {
            if let Some(offset) = state
                .connector_catalog
                .next_cursor
                .as_deref()
                .and_then(|c| c.parse::<usize>().ok())
            {
                catalog_page(state, offset);
            }
        }
        Command::LoadServiceTools { service } => {
            if state.tools_of(&service).is_none() {
                state.service_tools.push(mx::service_tools(&service));
            }
            // As the hub's detail: the service with its sign-in schemes.
            if !state
                .connector_catalog
                .services
                .iter()
                .any(|s| s.slug == service && !s.auth_schemes.is_empty())
                && let Some(found) = mx::catalog_services()
                    .into_iter()
                    .find(|s| s.slug == service)
            {
                state
                    .connector_catalog
                    .services
                    .retain(|s| s.slug != service);
                state.connector_catalog.services.push(found);
            }
        }
        Command::Connect(new) => {
            let name = new.name.trim().to_owned();
            consort_proto::model::validate_connection_name(&name).map_err(|_| {
                invalid("Enter a name of at most 80 characters, such as Gmail Work.")
            })?;
            if matches!(&new.use_in, UseInView::Tags(t) if t.is_empty()) {
                return Err(invalid("Choose at least one tag, or choose Every channel."));
            }
            let browser = new.auth_scheme.is_none_or(AuthKind::uses_browser);
            let id = format!("con_{new_id}");
            let redirect_url = start_connection(
                state,
                &id,
                &new.service,
                &name,
                new.use_in,
                new.approvals,
                browser,
                later,
            );
            if let Some(approval_id) = &new.approval_id {
                answer_card(state, approval_id, |c| {
                    if let HubCardView::ConnectService(c) = c {
                        c.connection_id = Some(id.clone());
                    }
                });
            }
            return Ok(CommandOutput::ConnectStarted {
                connection_id: id,
                redirect_url,
            });
        }
        Command::ReconnectConnection { connection_id, .. } => {
            let connection = state
                .connections
                .iter_mut()
                .find(|c| c.id == connection_id)
                .ok_or_else(|| not_found("connector"))?;
            let redirect_url = format!("https://connect.composio.dev/link/{connection_id}");
            connection.status = ConnectionStatusView::Connecting {
                redirect_url: Some(redirect_url.clone()),
                expires_at: crate::format::now_ms() + 10 * 60_000,
            };
            let id = connection_id.clone();
            later.push((
                MOCK_SIGN_IN,
                Box::new(move |state: &mut ViewState| {
                    if let Some(c) = state.connections.iter_mut().find(|c| c.id == id) {
                        c.status = ConnectionStatusView::Active;
                    }
                }),
            ));
            return Ok(CommandOutput::ConnectStarted {
                connection_id,
                redirect_url: Some(redirect_url),
            });
        }
        Command::UpdateConnection {
            connection_id,
            changes,
        } => {
            if matches!(&changes.use_in, Some(UseInView::Tags(t)) if t.is_empty()) {
                return Err(invalid("Choose at least one tag, or choose Every channel."));
            }
            let handle = changes
                .name
                .as_deref()
                .map(|n| handle_for(n.trim(), state, Some(&connection_id)));
            let connection = state
                .connections
                .iter_mut()
                .find(|c| c.id == connection_id)
                .ok_or_else(|| not_found("connector"))?;
            if let (Some(name), Some(handle)) = (changes.name, handle) {
                let name = name.trim().to_owned();
                consort_proto::model::validate_connection_name(&name).map_err(|_| {
                    invalid("Enter a name of at most 80 characters, such as Gmail Work.")
                })?;
                connection.name = name;
                connection.handle = handle;
            }
            if let Some(use_in) = changes.use_in {
                connection.use_in = use_in;
            }
            if let Some(approvals) = changes.approvals {
                connection.approvals = approvals;
            }
        }
        Command::DisconnectConnection { connection_id } => {
            state.connections.retain(|c| c.id != connection_id);
            for channel in &mut state.channels {
                channel.disabled_connections.retain(|c| c != &connection_id);
            }
        }
        Command::LoadConnectionCalls {
            connection_id,
            before,
        } => {
            // Two pages: the newest calls, then older ones.
            let call = |n: u32,
                        tool: &str,
                        kind: ToolKindView,
                        decision: &str,
                        error: Option<&str>,
                        day: &str,
                        time: &str,
                        size: &str| ConnectorCallView {
                id: format!("ccl_mock_{n}"),
                tool: tool.into(),
                kind,
                decision: decision.into(),
                succeeded: error.is_none(),
                error: error.map(str::to_owned),
                time_label: time.into(),
                day_label: day.into(),
                size_label: size.into(),
            };
            let (calls, has_more) = if before.is_none() {
                (
                    vec![
                        call(
                            3,
                            "GMAIL_FETCH_EMAILS",
                            ToolKindView::Read,
                            "Ran without asking",
                            None,
                            "Today",
                            "2:40 PM",
                            "12.4 KB",
                        ),
                        call(
                            2,
                            "GMAIL_SEND_EMAIL",
                            ToolKindView::Change,
                            "Allowed by you",
                            None,
                            "Today",
                            "11:02 AM",
                            "412 B",
                        ),
                        call(
                            1,
                            "GMAIL_DELETE_THREAD",
                            ToolKindView::Change,
                            "Denied by you",
                            Some("You said no."),
                            "Yesterday",
                            "4:15 PM",
                            "0 B",
                        ),
                    ],
                    true,
                )
            } else {
                (
                    vec![call(
                        0,
                        "GMAIL_LIST_LABELS",
                        ToolKindView::Read,
                        "Ran without asking",
                        None,
                        "October 12",
                        "9:30 AM",
                        "1.2 KB",
                    )],
                    false,
                )
            };
            return Ok(CommandOutput::ConnectionCalls(ConnectionCallsView {
                connection_id,
                calls,
                has_more,
            }));
        }
        Command::SetChannelConnectionEnabled {
            channel_id,
            connection_id,
            enabled,
        } => {
            let connection = state
                .connection(&connection_id)
                .cloned()
                .ok_or_else(|| not_found("connector"))?;
            let channel = state
                .channels
                .iter_mut()
                .find(|c| c.id == channel_id)
                .ok_or_else(|| not_found("channel"))?;
            if enabled && !use_in_matches(&connection.use_in, channel) {
                return Err(invalid(
                    "This connector is not in the channel's scope. Change its Use in in Settings > Connectors.",
                ));
            }
            set_disabled(&mut channel.disabled_connections, &connection_id, enabled);
        }
        Command::SetChannelSkillEnabled {
            channel_id,
            skill_id,
            enabled,
        } => {
            let skill = state
                .skill(&skill_id)
                .cloned()
                .ok_or_else(|| not_found("skill"))?;
            let channel = state
                .channels
                .iter_mut()
                .find(|c| c.id == channel_id)
                .ok_or_else(|| not_found("channel"))?;
            if enabled && !use_in_matches(&skill.use_in, channel) {
                return Err(invalid(
                    "This skill is not in the channel's scope. Change its Use in in Settings > Skills.",
                ));
            }
            set_disabled(&mut channel.disabled_skills, &skill_id, enabled);
        }
        // A mock hub without the skill pipeline (`--screen
        // add-skill-unsupported`) answers the way a step 2 hub does.
        Command::SearchSkillDirectory { search, .. }
            if matches!(state.skill_directory.load, LoadState::Unsupported(_)) =>
        {
            state.skill_directory.search = search;
        }
        Command::PreviewSkill { .. } | Command::InstallSkill { .. }
            if matches!(state.skill_directory.load, LoadState::Unsupported(_)) =>
        {
            return Err(ClientError::Hub {
                code: "unsupported".into(),
                message: "This hub cannot add skills yet. Update the hub, then try again.".into(),
            });
        }
        Command::SearchSkillDirectory { search, source } => {
            let query = search.trim().to_lowercase();
            let installed: Vec<(String, String)> = state
                .skills
                .iter()
                .map(|s| (s.name.clone(), s.id.clone()))
                .collect();
            let directory = &mut state.skill_directory;
            directory.search = search;
            directory.source = source.clone();
            directory.entries = mx::directory_entries()
                .into_iter()
                .filter(|e| {
                    source
                        .as_deref()
                        .is_none_or(|s| e.source_id.as_deref() == Some(s))
                })
                .filter(|e| {
                    query.is_empty()
                        || e.name.to_lowercase().contains(&query)
                        || e.description.to_lowercase().contains(&query)
                })
                .map(|mut e| {
                    e.installed_skill_id = installed
                        .iter()
                        .find(|(name, _)| *name == e.name)
                        .map(|(_, id)| id.clone());
                    e
                })
                .collect();
            directory.next_cursor = None;
            directory.load = LoadState::Loaded;
        }
        Command::LoadMoreSkillDirectory => {}
        Command::LoadSkillDirectorySources => {
            state.skill_directory.sources = mx::directory_sources();
        }
        Command::AddSkillDirectorySource { url } => {
            let url = url.trim().to_owned();
            if url.is_empty() {
                return Err(invalid(
                    "Paste the URL of a git repo or a marketplace.json file.",
                ));
            }
            state.skill_directory.sources.push(DirectorySourceView {
                id: format!("src_{new_id}"),
                kind: DirectoryKind::Marketplace,
                name: repo_name(&url),
                url,
                removable: true,
            });
        }
        Command::RemoveSkillDirectorySource { id } => {
            state
                .skill_directory
                .sources
                .retain(|s| s.id != id || !s.removable);
        }
        Command::PreviewSkill { source } => {
            let preview = match &source {
                SkillInstallInput::Directory { entry_id, .. } => {
                    let entry = mx::directory_entries()
                        .into_iter()
                        .find(|e| &e.entry_id == entry_id)
                        .ok_or_else(|| not_found("skill"))?;
                    let blocked = entry
                        .audits
                        .iter()
                        .any(|a| a.status == AuditStatusView::Failed)
                        .then(|| "An audit failed, so Consort blocks this skill.".to_owned());
                    SkillPreviewView {
                        name: entry.name.clone(),
                        description: entry.description,
                        commit: Some("a1b2c3d".into()),
                        license: entry.license,
                        scripts: if entry.name == "crate-release" {
                            vec!["scripts/release.sh".into()]
                        } else {
                            Vec::new()
                        },
                        audits: entry.audits,
                        file_count: 3,
                        has_hooks: false,
                        blocked_reason: blocked,
                        installed_skill_id: state
                            .skills
                            .iter()
                            .find(|s| s.name == entry.name)
                            .map(|s| s.id.clone()),
                    }
                }
                SkillInstallInput::Git { url, path, .. } => {
                    crate::extensions_view::install_source(&source, None)?;
                    let name = path
                        .as_deref()
                        .and_then(|p| p.trim_matches('/').rsplit('/').next())
                        .filter(|n| !n.is_empty())
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            repo_name(url)
                                .rsplit('/')
                                .next()
                                .unwrap_or("skill")
                                .to_owned()
                        });
                    SkillPreviewView {
                        name,
                        description: "A skill from your git repo.".into(),
                        commit: Some("e4f5a6b".into()),
                        license: None,
                        file_count: 2,
                        ..SkillPreviewView::default()
                    }
                }
                SkillInstallInput::Upload { file_path } => {
                    // The mock checks the zip as the hub backend does, then
                    // shows the skill that the zip would bring.
                    let size = std::fs::metadata(file_path).map(|m| m.len()).map_err(|e| {
                        ClientError::Storage {
                            path: file_path.clone(),
                            message: e.to_string(),
                        }
                    })?;
                    crate::extensions_view::check_skill_zip(file_path, size)?;
                    let name = crate::extensions_view::zip_file_name(file_path);
                    SkillPreviewView {
                        name: name.trim_end_matches(".zip").to_owned(),
                        description: "A skill from a zip file on this computer.".into(),
                        file_count: 2,
                        ..SkillPreviewView::default()
                    }
                }
            };
            return Ok(CommandOutput::SkillPreview(preview));
        }
        Command::InstallSkill {
            source,
            use_in,
            enabled,
            approval_id,
        } => {
            let CommandOutput::SkillPreview(preview) = apply(
                state,
                Command::PreviewSkill {
                    source: source.clone(),
                },
                new_id,
                later,
            )?
            else {
                return Err(invalid("This skill did not load. Try again."));
            };
            if let Some(reason) = preview.blocked_reason {
                return Err(ClientError::Hub {
                    code: "skill_blocked".into(),
                    message: reason,
                });
            }
            let id = format!("skl_{new_id}");
            let source = match source {
                SkillInstallInput::Directory { entry_id, .. } => SkillSourceView::Directory {
                    directory: DirectoryKind::Smithery,
                    entry_id: entry_id.clone(),
                    url: format!("https://github.com/smithery-ai/{entry_id}"),
                    path: None,
                },
                SkillInstallInput::Git { url, path, git_ref } => {
                    SkillSourceView::Git { url, path, git_ref }
                }
                SkillInstallInput::Upload { file_path } => SkillSourceView::Upload {
                    file_name: file_path,
                },
            };
            let has_scripts = !preview.scripts.is_empty();
            state.skills.push(SkillView {
                id: id.clone(),
                name: preview.name,
                description: preview.description,
                source,
                commit: preview.commit,
                license: preview.license,
                scripts: preview.scripts,
                audits: preview.audits,
                enabled: enabled.unwrap_or(!has_scripts),
                use_in,
                file_count: preview.file_count,
                installed_label: "Today".into(),
                ..SkillView::default()
            });
            if let Some(approval_id) = &approval_id {
                answer_card(state, approval_id, |c| {
                    if let HubCardView::SkillInstall(c) = c {
                        c.skill_id = Some(id.clone());
                    }
                });
            }
            return Ok(CommandOutput::SkillInstalled(id));
        }
        Command::UpdateSkill { skill_id, changes } => {
            if matches!(&changes.use_in, Some(UseInView::Tags(t)) if t.is_empty()) {
                return Err(invalid("Choose at least one tag, or choose Every channel."));
            }
            let skill = state
                .skills
                .iter_mut()
                .find(|s| s.id == skill_id)
                .ok_or_else(|| not_found("skill"))?;
            if let Some(enabled) = changes.enabled {
                skill.enabled = enabled;
            }
            if let Some(use_in) = changes.use_in {
                skill.use_in = use_in;
            }
            if changes.accept_new_version
                && let SkillUpdateView::Available { commit, .. } = &skill.update
            {
                skill.commit = commit.clone();
                skill.update = SkillUpdateView::UpToDate;
            }
        }
        Command::RemoveSkill { skill_id } => {
            state.skills.retain(|s| s.id != skill_id);
            for channel in &mut state.channels {
                channel.disabled_skills.retain(|s| s != &skill_id);
            }
        }
        Command::LoadSkillFiles { skill_id, .. } => {
            let skill = state.skill(&skill_id).ok_or_else(|| not_found("skill"))?;
            let mut files = vec![SkillFileView {
                path: "SKILL.md".into(),
                size: 2_400,
                executable: false,
            }];
            files.extend(skill.scripts.iter().map(|s| SkillFileView {
                path: s.clone(),
                size: 1_100,
                executable: true,
            }));
            return Ok(CommandOutput::SkillFiles(files));
        }
        Command::ReadSkillFile {
            skill_id,
            path,
            pending,
        } => {
            let skill = state.skill(&skill_id).ok_or_else(|| not_found("skill"))?;
            let content = if path == "SKILL.md" {
                let mut text = format!(
                    "---\nname: {}\ndescription: {}\n---\n\n# {}\n\nFollow these steps when the task fits this skill.\n",
                    skill.name, skill.description, skill.name
                );
                // The new version of a skill adds a step, so a review has a
                // change to show.
                if pending {
                    text.push_str("Run the linter before you finish.\n");
                }
                text
            } else {
                "#!/usr/bin/env python3\nprint(\"mock script\")\n".to_owned()
            };
            return Ok(CommandOutput::SkillFile(SkillFileContentView {
                path,
                content: Some(content),
                binary: false,
                truncated: false,
            }));
        }
        other => {
            return Err(ClientError::Other(format!(
                "The mock cannot run {other:?}."
            )));
        }
    }
    Ok(CommandOutput::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewConnection, mock};

    #[test]
    fn a_connect_starts_in_the_browser_and_turns_active_later() {
        let mut state = mock::view_state();
        let mut later = Vec::new();
        let out = apply(
            &mut state,
            Command::Connect(NewConnection {
                service: "github".into(),
                name: "GitHub Work".into(),
                use_in: UseInView::Tags(vec!["work".into()]),
                ..NewConnection::default()
            }),
            "1",
            &mut later,
        )
        .unwrap();
        let CommandOutput::ConnectStarted {
            connection_id,
            redirect_url,
        } = out
        else {
            panic!("a connect attempt");
        };
        assert!(redirect_url.is_some());
        let connection = state.connection(&connection_id).unwrap();
        assert_eq!(connection.handle, "github_work");
        assert!(matches!(
            connection.status,
            ConnectionStatusView::Connecting { .. }
        ));
        assert_eq!(later.len(), 1);
        for (_, change) in later {
            change(&mut state);
        }
        assert!(state.connection(&connection_id).unwrap().status.is_active());
    }

    #[test]
    fn a_channel_cannot_turn_on_what_its_scope_does_not_give() {
        let mut state = mock::view_state();
        let mut later = Vec::new();
        let err = apply(
            &mut state,
            Command::SetChannelConnectionEnabled {
                channel_id: mx::ACME.into(),
                connection_id: mx::CON_NOTION.into(),
                enabled: true,
            },
            "1",
            &mut later,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Use in"));
        apply(
            &mut state,
            Command::SetChannelConnectionEnabled {
                channel_id: mx::ACME.into(),
                connection_id: mx::CON_GMAIL_PERSONAL.into(),
                enabled: true,
            },
            "1",
            &mut later,
        )
        .unwrap();
        assert!(
            state
                .channel(mx::ACME)
                .unwrap()
                .disabled_connections
                .is_empty()
        );
    }
}
