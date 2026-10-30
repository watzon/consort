//! The connector and skill commands: what each asks of the hub, and where
//! the answer goes in the cache. Also the loader of service logos.

use std::sync::Arc;

use consort_proto::{
    client_api::{
        AccountUpdate, ChannelSetConnectionEnabled, ChannelSetSkillEnabled, ComposioGetStatus,
        ComposioRemoveKey, ComposioSetKey, ConnectionCalls, ConnectionConnect,
        ConnectionDisconnect, ConnectionReconnect, ConnectionUpdate, ConnectorCatalogCategories,
        ConnectorCatalogSearch, ConnectorCatalogService, SkillDirectoryAddSource,
        SkillDirectoryRemoveSource, SkillDirectorySearch, SkillDirectorySources, SkillFiles,
        SkillInstall, SkillPreview, SkillReadFile, SkillRemove, SkillUpdate, SkillVersion,
    },
    model::{CallDecision, CallOutcome, ComposioStatus, SecretEnv, SkillDirectorySource},
};
use futures_util::StreamExt as _;

use super::{Shared, invalid};
use crate::{
    ClientError, Command, CommandOutput,
    connectors::{
        CategoryView, ConnectionCallsView, ConnectorCallView, ConnectorToolView, LoadState,
        LogoImage, ServiceToolsView,
    },
    extensions_view::*,
    format,
    skills::{DirectorySourceView, SkillFileContentView, SkillFileView, SkillPreviewView},
};

/// The size of one page of the catalog and of the skill directory.
const PAGE: u32 = 30;

fn parse<T: std::str::FromStr>(id: &str, what: &str) -> Result<T, ClientError> {
    id.parse().map_err(|_| {
        ClientError::Other(format!(
            "Consort cannot use this {what}, because its ID is not valid. Restart Consort, then try again"
        ))
    })
}

/// An `unsupported` answer in words that say what the hub cannot do yet.
fn unsupported(err: ClientError, what: &str) -> ClientError {
    match err {
        ClientError::Hub { code, .. } if code == "unsupported" => ClientError::Hub {
            code,
            message: format!("This hub cannot {what} yet. Update the hub, then try again."),
        },
        other => other,
    }
}

/// The load state of a failed load.
fn failed(err: ClientError) -> LoadState {
    match &err {
        ClientError::Hub { code, message } if code == "unsupported" => {
            LoadState::Unsupported(message.clone())
        }
        _ => LoadState::Failed(err.to_string()),
    }
}

fn set_composio(shared: &Shared, status: ComposioStatus) {
    if let Ok(mut cache) = shared.cache.lock() {
        let view = &mut cache.extensions.composio;
        view.load = LoadState::Loaded;
        view.key_present = status.key_present;
        view.key_hint = status.key_hint;
        view.plan = status.plan;
        view.used = status.usage.as_ref().map(|u| u.used);
        view.limit = status.usage.as_ref().and_then(|u| u.limit);
        view.error = status.error;
    }
    shared.publish();
}

fn set_sources(shared: &Shared, sources: Vec<SkillDirectorySource>) {
    if let Ok(mut cache) = shared.cache.lock() {
        cache.extensions.directory.sources = sources
            .into_iter()
            .map(|s| DirectorySourceView {
                id: s.id,
                kind: directory_kind(s.kind),
                name: s.name,
                url: s.url,
                removable: s.removable,
            })
            .collect();
    }
    shared.publish();
}

fn decision_label(decision: CallDecision) -> &'static str {
    match decision {
        CallDecision::RanWithoutAsking => "Ran without asking",
        CallDecision::AllowedByUser => "Allowed by you",
        CallDecision::AllowedEarlierInTask => "Allowed earlier in the task",
        CallDecision::DeniedByUser => "Denied by you",
        CallDecision::Expired => "Nobody answered in time",
    }
}

/// Runs one connector or skill command.
pub(super) async fn run(
    shared: &Arc<Shared>,
    command: Command,
) -> Result<CommandOutput, ClientError> {
    match command {
        Command::SetAccountExtras {
            worker_id,
            account_id,
            enabled,
        } => {
            shared
                .call(AccountUpdate {
                    worker_id: parse(&worker_id, "worker")?,
                    account_id: parse(&account_id, "account")?,
                    label: None,
                    env_set: SecretEnv::new(),
                    env_remove: Vec::new(),
                    extras_enabled: Some(enabled),
                })
                .await?;
        }
        Command::LoadComposioStatus => {
            if let Ok(mut cache) = shared.cache.lock() {
                cache.extensions.composio.load = LoadState::Loading;
            }
            shared.publish();
            match shared.call(ComposioGetStatus {}).await {
                Ok(result) => set_composio(shared, result.status),
                Err(err) => {
                    let err = unsupported(err, "connect services");
                    if let Ok(mut cache) = shared.cache.lock() {
                        cache.extensions.composio.load = failed(err);
                    }
                    shared.publish();
                }
            }
        }
        Command::SetComposioKey { api_key } => {
            let api_key = api_key.0.trim().to_owned();
            if api_key.is_empty() {
                return Err(invalid("Paste your Composio project key."));
            }
            let result = shared
                .call(ComposioSetKey { api_key })
                .await
                .map_err(|e| unsupported(e, "connect services"))?;
            set_composio(shared, result.status);
        }
        Command::RemoveComposioKey => {
            let result = shared.call(ComposioRemoveKey {}).await?;
            set_composio(shared, result.status);
        }
        Command::LoadConnectorCategories => {
            match shared.call(ConnectorCatalogCategories {}).await {
                Ok(result) => {
                    if let Ok(mut cache) = shared.cache.lock() {
                        cache.extensions.catalog.categories = result
                            .categories
                            .into_iter()
                            .map(|c| CategoryView {
                                slug: c.slug,
                                name: c.name,
                            })
                            .collect();
                    }
                    shared.publish();
                }
                // The dialog works without categories.
                Err(err) => tracing::debug!(error = %err, "no connector categories"),
            }
        }
        Command::SearchConnectorCatalog { search, category } => {
            let serial = match shared.cache.lock() {
                Ok(mut cache) => {
                    let ext = &mut cache.extensions;
                    ext.catalog_serial += 1;
                    ext.catalog.search = search.clone();
                    ext.catalog.category = category.clone();
                    ext.catalog.load = LoadState::Loading;
                    ext.catalog.more = LoadState::NotLoaded;
                    ext.catalog_serial
                }
                Err(_) => return Ok(CommandOutput::Done),
            };
            shared.publish();
            let result = shared
                .call(ConnectorCatalogSearch {
                    search: Some(search.trim().to_owned()).filter(|s| !s.is_empty()),
                    category,
                    cursor: None,
                    limit: Some(PAGE),
                })
                .await;
            if let Ok(mut cache) = shared.cache.lock()
                && cache.extensions.catalog_serial == serial
            {
                let catalog = &mut cache.extensions.catalog;
                match result {
                    Ok(page) => {
                        catalog.services = page.services.iter().map(service_view).collect();
                        catalog.next_cursor = page.next_cursor;
                        catalog.load = LoadState::Loaded;
                    }
                    Err(err) => {
                        catalog.services.clear();
                        catalog.next_cursor = None;
                        catalog.load = failed(unsupported(err, "search the connector catalog"));
                    }
                }
            }
            shared.publish();
        }
        Command::LoadMoreConnectorCatalog => {
            let start = shared.cache.lock().ok().and_then(|mut cache| {
                let serial = cache.extensions.catalog_serial;
                let catalog = &mut cache.extensions.catalog;
                if catalog.more.is_loading() || catalog.load != LoadState::Loaded {
                    return None;
                }
                let cursor = catalog.next_cursor.clone()?;
                catalog.more = LoadState::Loading;
                Some((
                    serial,
                    cursor,
                    catalog.search.clone(),
                    catalog.category.clone(),
                ))
            });
            let Some((serial, cursor, search, category)) = start else {
                return Ok(CommandOutput::Done);
            };
            shared.publish();
            let result = shared
                .call(ConnectorCatalogSearch {
                    search: Some(search).filter(|s| !s.trim().is_empty()),
                    category,
                    cursor: Some(cursor),
                    limit: Some(PAGE),
                })
                .await;
            if let Ok(mut cache) = shared.cache.lock()
                && cache.extensions.catalog_serial == serial
            {
                let catalog = &mut cache.extensions.catalog;
                match result {
                    Ok(page) => {
                        for service in &page.services {
                            if !catalog.services.iter().any(|s| s.slug == service.slug) {
                                catalog.services.push(service_view(service));
                            }
                        }
                        catalog.next_cursor = page.next_cursor;
                        catalog.more = LoadState::NotLoaded;
                    }
                    Err(err) => catalog.more = failed(err),
                }
            }
            shared.publish();
        }
        Command::LoadServiceTools { service } => {
            let start = shared.cache.lock().is_ok_and(|mut cache| {
                let tools = &mut cache.extensions.service_tools;
                if tools
                    .get(&service)
                    .is_some_and(|t| matches!(t.load, LoadState::Loading | LoadState::Loaded))
                {
                    return false;
                }
                tools.insert(
                    service.clone(),
                    ServiceToolsView {
                        service: service.clone(),
                        tools: Vec::new(),
                        load: LoadState::Loading,
                    },
                );
                true
            });
            if !start {
                return Ok(CommandOutput::Done);
            }
            let result = shared
                .call(ConnectorCatalogService {
                    service: service.clone(),
                })
                .await;
            if let Ok(mut cache) = shared.cache.lock() {
                let entry = ServiceToolsView {
                    service: service.clone(),
                    tools: result
                        .as_ref()
                        .map(|detail| {
                            detail
                                .tools
                                .iter()
                                .map(|t| ConnectorToolView {
                                    slug: t.slug.clone(),
                                    description: t.description.clone(),
                                    kind: kind_view(t.kind),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    load: match &result {
                        Ok(_) => LoadState::Loaded,
                        Err(err) => failed(err.clone()),
                    },
                };
                cache.extensions.service_tools.insert(service, entry);
            }
            shared.publish();
        }
        Command::Connect(new) => {
            let name = new.name.trim().to_owned();
            consort_proto::model::validate_connection_name(&name).map_err(|_| {
                invalid("Enter a name of at most 80 characters, such as Gmail Work.")
            })?;
            let attempt = shared
                .call(ConnectionConnect {
                    service: new.service,
                    name,
                    use_in: use_in_proto(&new.use_in)?,
                    approvals: approvals_proto(&new.approvals),
                    auth_scheme: new.auth_scheme.map(auth_kind_proto),
                    credentials: new.credentials,
                    approval_id: new.approval_id.map(|id| parse(&id, "card")).transpose()?,
                })
                .await
                .map_err(|e| unsupported(e, "connect services"))?;
            let connection_id = attempt.connection.id.to_string();
            if let Ok(mut cache) = shared.cache.lock() {
                cache.merge_connection(attempt.connection);
            }
            shared.publish();
            return Ok(CommandOutput::ConnectStarted {
                connection_id,
                redirect_url: attempt.redirect_url.filter(|u| !u.is_empty()),
            });
        }
        Command::ReconnectConnection {
            connection_id,
            credentials,
        } => {
            let attempt = shared
                .call(ConnectionReconnect {
                    connection_id: parse(&connection_id, "connection")?,
                    credentials,
                })
                .await
                .map_err(|e| unsupported(e, "connect services"))?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.merge_connection(attempt.connection);
            }
            shared.publish();
            return Ok(CommandOutput::ConnectStarted {
                connection_id,
                redirect_url: attempt.redirect_url.filter(|u| !u.is_empty()),
            });
        }
        Command::UpdateConnection {
            connection_id,
            changes,
        } => {
            let name = changes.name.map(|n| n.trim().to_owned());
            if let Some(name) = &name {
                consort_proto::model::validate_connection_name(name).map_err(|_| {
                    invalid("Enter a name of at most 80 characters, such as Gmail Work.")
                })?;
            }
            let result = shared
                .call(ConnectionUpdate {
                    connection_id: parse(&connection_id, "connection")?,
                    name,
                    use_in: changes.use_in.as_ref().map(use_in_proto).transpose()?,
                    approvals: changes.approvals.as_ref().map(approvals_proto),
                })
                .await?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.merge_connection(result.connection);
            }
            shared.publish();
        }
        Command::DisconnectConnection { connection_id } => {
            shared
                .call(ConnectionDisconnect {
                    connection_id: parse(&connection_id, "connection")?,
                })
                .await?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.connections.remove(&connection_id);
            }
            shared.publish();
        }
        Command::LoadConnectionCalls {
            connection_id,
            before,
        } => {
            let page = shared
                .call(ConnectionCalls {
                    connection_id: parse(&connection_id, "connection")?,
                    before: before.map(|b| parse(&b, "call")).transpose()?,
                    limit: None,
                })
                .await?;
            return Ok(CommandOutput::ConnectionCalls(ConnectionCallsView {
                connection_id,
                calls: page
                    .calls
                    .iter()
                    .map(|c| ConnectorCallView {
                        id: c.id.to_string(),
                        tool: c.tool.clone(),
                        kind: kind_view(c.tool_kind),
                        decision: decision_label(c.decision).to_owned(),
                        succeeded: c.outcome == CallOutcome::Succeeded,
                        error: c.error.clone(),
                        time_label: format::time_label(c.at.as_millis()),
                        day_label: format::day_label(c.at.as_millis(), format::now_ms()),
                        size_label: format::size_label(c.result_bytes),
                    })
                    .collect(),
                has_more: page.has_more,
            }));
        }
        Command::SetChannelConnectionEnabled {
            channel_id,
            connection_id,
            enabled,
        } => {
            let result = shared
                .call(ChannelSetConnectionEnabled {
                    channel_id: parse(&channel_id, "channel")?,
                    connection_id: parse(&connection_id, "connection")?,
                    enabled,
                })
                .await?;
            merge_channel(shared, result.channel);
        }
        Command::SetChannelSkillEnabled {
            channel_id,
            skill_id,
            enabled,
        } => {
            let result = shared
                .call(ChannelSetSkillEnabled {
                    channel_id: parse(&channel_id, "channel")?,
                    skill_id: parse(&skill_id, "skill")?,
                    enabled,
                })
                .await?;
            merge_channel(shared, result.channel);
        }
        Command::SearchSkillDirectory { search, source } => {
            let serial = match shared.cache.lock() {
                Ok(mut cache) => {
                    let ext = &mut cache.extensions;
                    ext.directory_serial += 1;
                    ext.directory.search = search.clone();
                    ext.directory.source = source.clone();
                    ext.directory.load = LoadState::Loading;
                    ext.directory.more = LoadState::NotLoaded;
                    ext.directory_serial
                }
                Err(_) => return Ok(CommandOutput::Done),
            };
            shared.publish();
            let result = shared
                .call(SkillDirectorySearch {
                    search: Some(search.trim().to_owned()).filter(|s| !s.is_empty()),
                    source,
                    cursor: None,
                    limit: Some(PAGE),
                })
                .await;
            if let Ok(mut cache) = shared.cache.lock()
                && cache.extensions.directory_serial == serial
            {
                let directory = &mut cache.extensions.directory;
                match result {
                    Ok(page) => {
                        directory.entries = page.entries.iter().map(directory_entry_view).collect();
                        directory.next_cursor = page.next_cursor;
                        directory.unavailable = page.unavailable;
                        directory.load = LoadState::Loaded;
                    }
                    Err(err) => {
                        directory.entries.clear();
                        directory.next_cursor = None;
                        directory.load = failed(unsupported(err, "search skill directories"));
                    }
                }
            }
            shared.publish();
        }
        Command::LoadMoreSkillDirectory => {
            let start = shared.cache.lock().ok().and_then(|mut cache| {
                let serial = cache.extensions.directory_serial;
                let directory = &mut cache.extensions.directory;
                if directory.more.is_loading() || directory.load != LoadState::Loaded {
                    return None;
                }
                let cursor = directory.next_cursor.clone()?;
                directory.more = LoadState::Loading;
                Some((
                    serial,
                    cursor,
                    directory.search.clone(),
                    directory.source.clone(),
                ))
            });
            let Some((serial, cursor, search, source)) = start else {
                return Ok(CommandOutput::Done);
            };
            shared.publish();
            let result = shared
                .call(SkillDirectorySearch {
                    search: Some(search).filter(|s| !s.trim().is_empty()),
                    source,
                    cursor: Some(cursor),
                    limit: Some(PAGE),
                })
                .await;
            if let Ok(mut cache) = shared.cache.lock()
                && cache.extensions.directory_serial == serial
            {
                let directory = &mut cache.extensions.directory;
                match result {
                    Ok(page) => {
                        directory
                            .entries
                            .extend(page.entries.iter().map(directory_entry_view));
                        directory.next_cursor = page.next_cursor;
                        directory.more = LoadState::NotLoaded;
                    }
                    Err(err) => directory.more = failed(err),
                }
            }
            shared.publish();
        }
        Command::LoadSkillDirectorySources => match shared.call(SkillDirectorySources {}).await {
            Ok(result) => set_sources(shared, result.sources),
            Err(err) => tracing::debug!(error = %err, "no skill directory sources"),
        },
        Command::AddSkillDirectorySource { url } => {
            let url = url.trim().to_owned();
            if url.is_empty() {
                return Err(invalid(
                    "Paste the URL of a git repo or a marketplace.json file.",
                ));
            }
            let result = shared
                .call(SkillDirectoryAddSource { url })
                .await
                .map_err(|e| unsupported(e, "add skill directories"))?;
            set_sources(shared, result.sources);
        }
        Command::RemoveSkillDirectorySource { id } => {
            let result = shared
                .call(SkillDirectoryRemoveSource { id })
                .await
                .map_err(|e| unsupported(e, "remove skill directories"))?;
            set_sources(shared, result.sources);
        }
        Command::PreviewSkill { source } => {
            let preview = shared
                .call(SkillPreview {
                    source: install_source(&source)?,
                })
                .await
                .map_err(|e| unsupported(e, "add skills"))?;
            return Ok(CommandOutput::SkillPreview(SkillPreviewView {
                name: preview.name,
                description: preview.description,
                commit: preview.commit.as_deref().map(crate::skills::short_commit),
                license: preview.license,
                scripts: preview.scripts,
                audits: preview.audits.iter().map(audit_view).collect(),
                file_count: u32::try_from(preview.files.len()).unwrap_or(u32::MAX),
                has_hooks: preview.has_hooks,
                blocked_reason: preview.blocked_reason,
                installed_skill_id: preview.installed_skill_id.map(|i| i.to_string()),
            }));
        }
        Command::InstallSkill {
            source,
            use_in,
            enabled,
            approval_id,
        } => {
            let result = shared
                .call(SkillInstall {
                    source: install_source(&source)?,
                    use_in: use_in_proto(&use_in)?,
                    enabled,
                    approval_id: approval_id.map(|id| parse(&id, "card")).transpose()?,
                })
                .await
                .map_err(|e| unsupported(e, "add skills"))?;
            let id = result.skill.id.to_string();
            if let Ok(mut cache) = shared.cache.lock() {
                cache.merge_skill(result.skill);
            }
            shared.publish();
            return Ok(CommandOutput::SkillInstalled(id));
        }
        Command::UpdateSkill { skill_id, changes } => {
            let result = shared
                .call(SkillUpdate {
                    skill_id: parse(&skill_id, "skill")?,
                    enabled: changes.enabled,
                    use_in: changes.use_in.as_ref().map(use_in_proto).transpose()?,
                    accept_new_version: changes.accept_new_version,
                })
                .await?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.merge_skill(result.skill);
            }
            shared.publish();
        }
        Command::RemoveSkill { skill_id } => {
            shared
                .call(SkillRemove {
                    skill_id: parse(&skill_id, "skill")?,
                })
                .await?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.skills.remove(&skill_id);
            }
            shared.publish();
        }
        Command::LoadSkillFiles { skill_id, pending } => {
            let result = shared
                .call(SkillFiles {
                    skill_id: parse(&skill_id, "skill")?,
                    version: if pending {
                        SkillVersion::Pending
                    } else {
                        SkillVersion::Installed
                    },
                })
                .await
                .map_err(|e| unsupported(e, "show the files of skills"))?;
            return Ok(CommandOutput::SkillFiles(
                result
                    .files
                    .into_iter()
                    .map(|f| SkillFileView {
                        path: f.path,
                        size: f.size,
                        executable: f.executable,
                    })
                    .collect(),
            ));
        }
        Command::ReadSkillFile {
            skill_id,
            path,
            pending,
        } => {
            let file = shared
                .call(SkillReadFile {
                    skill_id: parse(&skill_id, "skill")?,
                    path,
                    version: if pending {
                        SkillVersion::Pending
                    } else {
                        SkillVersion::Installed
                    },
                })
                .await
                .map_err(|e| unsupported(e, "show the files of skills"))?;
            return Ok(CommandOutput::SkillFile(SkillFileContentView {
                path: file.path,
                content: file.content,
                binary: file.binary,
                truncated: file.truncated,
            }));
        }
        other => {
            return Err(ClientError::Other(format!(
                "Consort cannot run {other:?} here. Restart Consort, then try again"
            )));
        }
    }
    Ok(CommandOutput::Done)
}

/// Puts the channel from a `channel.*` answer in the cache, unless the feed
/// already brought a newer one.
fn merge_channel(shared: &Shared, channel: consort_proto::model::Channel) {
    if let Ok(mut cache) = shared.cache.lock() {
        let id = channel.id.to_string();
        let newer = cache
            .channels
            .get(&id)
            .is_some_and(|c| c.updated_at > channel.updated_at);
        if !newer {
            cache.channels.insert(id, channel);
        }
    }
    shared.publish();
}

/// The largest logo that the client loads.
const LOGO_MAX_BYTES: usize = 256 * 1024;
/// How many logos load at once.
const LOGO_LOADS: usize = 6;

/// The bytes when they are an image that GPUI can draw: SVG, PNG, JPEG,
/// GIF or WebP, and not too large.
pub(crate) fn checked_image(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.is_empty() || bytes.len() > LOGO_MAX_BYTES {
        return None;
    }
    let raster = bytes.starts_with(b"\x89PNG")
        || bytes.starts_with(b"\xFF\xD8\xFF")
        || bytes.starts_with(b"GIF8")
        || (bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP");
    if raster {
        return Some(bytes.to_vec());
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let head = text.trim_start_matches('\u{feff}').trim_start();
    let is_svg = head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg"));
    is_svg.then(|| bytes.to_vec())
}

/// A short stable key for a URL: FNV-1a in hex.
fn url_key(url: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

async fn load_logo(client: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let response = client.get(parsed).send().await.ok()?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|len| len > LOGO_MAX_BYTES as u64)
    {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    checked_image(&bytes)
}

/// Loads the logos that the view uses, once each. A logo that does not load
/// stays a monogram, and the client does not ask for it again.
pub(crate) async fn run_logos(shared: Arc<Shared>) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!(error = %err, "cannot make the logo client");
            return;
        }
    };
    loop {
        shared.logo_wake.notified().await;
        let wanted = match shared.cache.lock() {
            Ok(mut cache) => {
                let wanted = cache.wanted_logos();
                cache.extensions.logos_tried.extend(wanted.iter().cloned());
                wanted
            }
            Err(_) => return,
        };
        if wanted.is_empty() {
            continue;
        }
        let shared = shared.clone();
        let client = client.clone();
        tokio::spawn(async move {
            futures_util::stream::iter(wanted)
                .for_each_concurrent(LOGO_LOADS, |url| {
                    let client = client.clone();
                    let shared = shared.clone();
                    async move {
                        match load_logo(&client, &url).await {
                            Some(bytes) => {
                                if let Ok(mut cache) = shared.cache.lock() {
                                    let image = LogoImage {
                                        key: url_key(&url),
                                        bytes: Arc::new(bytes),
                                    };
                                    cache.extensions.logos.insert(url, image);
                                }
                                shared.publish();
                            }
                            None => tracing::debug!(%url, "a service logo did not load"),
                        }
                    }
                })
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_small_image_counts_as_a_logo() {
        assert!(checked_image(b"<svg viewBox=\"0 0 1 1\"></svg>").is_some());
        assert!(checked_image(b"\x89PNG\r\n\x1a\n....").is_some());
        assert!(checked_image(b"RIFF\0\0\0\0WEBPVP8 ").is_some());
        assert!(checked_image(b"<html>not a logo</html>").is_none());
        assert!(checked_image(&vec![b'a'; LOGO_MAX_BYTES + 1]).is_none());
        assert!(checked_image(b"").is_none());
    }

    #[test]
    fn an_unsupported_answer_says_what_the_hub_cannot_do() {
        let err = unsupported(
            ClientError::Hub {
                code: "unsupported".into(),
                message: "Not built yet: skill.install.".into(),
            },
            "add skills",
        );
        assert_eq!(
            err.to_string(),
            "This hub cannot add skills yet. Update the hub, then try again."
        );
        assert!(matches!(failed(err), LoadState::Unsupported(_)));
        let other = ClientError::TimedOut;
        assert_eq!(unsupported(other.clone(), "x"), other);
    }

    #[test]
    fn a_url_key_is_stable_and_short() {
        assert_eq!(url_key("https://a"), url_key("https://a"));
        assert_ne!(url_key("https://a"), url_key("https://b"));
        assert_eq!(url_key("https://a").len(), 16);
    }
}
