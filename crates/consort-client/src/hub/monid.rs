//! The Monid commands: what each asks of the hub, and where the answer goes
//! in the cache (ADR 0016).

use std::sync::Arc;

use consort_proto::{
    client_api::{
        ChannelEndAccess, ChannelSetMonid, MonidCallGet, MonidCallList, MonidCallStop,
        MonidCatalogSearch, MonidGetStatus, MonidRemoveKey, MonidSetKey, MonidUpdateSettings,
    },
    model::MonidStatus,
};

use super::{Shared, extensions::merge_channel, invalid};
use crate::{
    ClientError, Command, CommandOutput,
    connectors::LoadState,
    monid::{MonidCallDetailView, MonidCallsView, MonidSearchHit, recent_call},
};

/// The most recent calls that the panel shows.
const RECENT_CALLS: u32 = 20;

fn parse<T: std::str::FromStr>(id: &str, what: &str) -> Result<T, ClientError> {
    id.parse().map_err(|_| {
        ClientError::Other(format!(
            "Consort cannot use this {what}, because its ID is not valid. Restart Consort, then try again"
        ))
    })
}

/// A failed load in words that say what to do. A hub that does not know
/// Monid says so.
fn failed(err: ClientError) -> LoadState {
    match &err {
        ClientError::Hub { code, .. } if code == "unsupported" => LoadState::Unsupported(
            "This hub does not have Monid yet. Update the hub, then try again.".to_owned(),
        ),
        _ => LoadState::Failed(err.to_string()),
    }
}

fn set_status(shared: &Shared, status: MonidStatus) {
    if let Ok(mut cache) = shared.cache.lock() {
        let channels: Vec<(String, String)> = cache
            .channels
            .values()
            .map(|c| (c.id.to_string(), c.name.clone()))
            .collect();
        cache.extensions.monid.set_status(status, &channels);
    }
    shared.publish();
}

fn set_detail(shared: &Shared, call: &consort_proto::model::MonidCall) {
    if let Ok(mut cache) = shared.cache.lock() {
        cache
            .extensions
            .monid_calls
            .insert(call.id.to_string(), MonidCallDetailView::from_call(call));
    }
    shared.publish();
}

/// Runs one Monid command.
pub(super) async fn run(
    shared: &Arc<Shared>,
    command: Command,
) -> Result<CommandOutput, ClientError> {
    match command {
        Command::LoadMonidStatus => {
            if let Ok(mut cache) = shared.cache.lock() {
                cache.extensions.monid.load = LoadState::Loading;
            }
            shared.publish();
            match shared.call(MonidGetStatus {}).await {
                Ok(result) => set_status(shared, result.status),
                Err(err) => {
                    if let Ok(mut cache) = shared.cache.lock() {
                        cache.extensions.monid.load = failed(err);
                    }
                    shared.publish();
                }
            }
        }
        Command::SetMonidKey { api_key, settings } => {
            let api_key = api_key.0.trim().to_owned();
            if api_key.is_empty() {
                return Err(invalid(
                    "Paste your Monid API key. Make one at app.monid.ai.",
                ));
            }
            let settings = settings.map(|s| s.to_proto()).transpose()?;
            let result = shared.call(MonidSetKey { api_key, settings }).await?;
            set_status(shared, result.status);
        }
        Command::RemoveMonidKey => {
            let result = shared.call(MonidRemoveKey {}).await?;
            set_status(shared, result.status);
        }
        Command::UpdateMonidSettings { settings } => {
            let result = shared
                .call(MonidUpdateSettings {
                    settings: settings.to_proto()?,
                })
                .await?;
            set_status(shared, result.status);
        }
        Command::SetChannelMonid { channel_id, access } => {
            let result = shared
                .call(ChannelSetMonid {
                    channel_id: parse(&channel_id, "channel")?,
                    access: access.to_proto(),
                })
                .await?;
            merge_channel(shared, result.channel);
        }
        Command::SearchMonid { query } => {
            let query = query.trim().to_owned();
            let serial = match shared.cache.lock() {
                Ok(mut cache) => {
                    cache.extensions.monid_search_serial += 1;
                    let search = &mut cache.extensions.monid.search;
                    search.query = query.clone();
                    search.load = if query.is_empty() {
                        LoadState::NotLoaded
                    } else {
                        LoadState::Loading
                    };
                    if query.is_empty() {
                        search.hits.clear();
                    }
                    cache.extensions.monid_search_serial
                }
                Err(_) => return Ok(CommandOutput::Done),
            };
            shared.publish();
            if query.is_empty() {
                return Ok(CommandOutput::Done);
            }
            let result = shared.call(MonidCatalogSearch { query }).await;
            if let Ok(mut cache) = shared.cache.lock()
                && cache.extensions.monid_search_serial == serial
            {
                let search = &mut cache.extensions.monid.search;
                match result {
                    Ok(found) => {
                        search.hits = found.tools.iter().map(MonidSearchHit::from_proto).collect();
                        search.load = LoadState::Loaded;
                    }
                    Err(err) => {
                        search.hits.clear();
                        search.load = failed(err);
                    }
                }
            }
            shared.publish();
        }
        Command::LoadMonidCalls { channel_id } => {
            let id = parse(&channel_id, "channel")?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache
                    .extensions
                    .monid
                    .calls
                    .entry(channel_id.clone())
                    .or_default()
                    .load = LoadState::Loading;
            }
            shared.publish();
            let result = shared
                .call(MonidCallList {
                    channel_id: id,
                    limit: Some(RECENT_CALLS),
                })
                .await;
            if let Ok(mut cache) = shared.cache.lock() {
                let entry = match result {
                    Ok(list) => MonidCallsView {
                        calls: list.calls.iter().map(recent_call).collect(),
                        load: LoadState::Loaded,
                    },
                    Err(err) => MonidCallsView {
                        calls: Vec::new(),
                        load: failed(err),
                    },
                };
                cache.extensions.monid.calls.insert(channel_id, entry);
            }
            shared.publish();
        }
        Command::LoadMonidCall { call_id } => {
            let id = parse(&call_id, "call")?;
            if let Ok(mut cache) = shared.cache.lock() {
                let entry = cache
                    .extensions
                    .monid_calls
                    .entry(call_id.clone())
                    .or_default();
                if entry.load.is_loading() {
                    return Ok(CommandOutput::Done);
                }
                // The old text stays while the new text loads.
                entry.load = LoadState::Loading;
            }
            shared.publish();
            match shared.call(MonidCallGet { id }).await {
                Ok(result) => set_detail(shared, &result.call),
                Err(err) => {
                    if let Ok(mut cache) = shared.cache.lock() {
                        cache
                            .extensions
                            .monid_calls
                            .entry(call_id)
                            .or_default()
                            .load = failed(err);
                    }
                    shared.publish();
                }
            }
        }
        Command::StopMonidCall { call_id } => {
            let result = shared
                .call(MonidCallStop {
                    id: parse(&call_id, "call")?,
                })
                .await?;
            set_detail(shared, &result.call);
        }
        Command::EndAccess {
            channel_id,
            grant_id,
        } => {
            let result = shared
                .call(ChannelEndAccess {
                    channel_id: parse(&channel_id, "channel")?,
                    grant_id: parse(&grant_id, "grant")?,
                })
                .await?;
            merge_channel(shared, result.channel);
        }
        _ => {}
    }
    Ok(CommandOutput::Done)
}
