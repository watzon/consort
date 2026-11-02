//! The Settings > Hub and Settings > Clients commands: what each asks of
//! the hub, and where the answer goes in the cache.

use std::sync::Arc;
use std::time::Duration;

use consort_proto::client_api::{
    BackupSchedule, ClientList, ClientPairingCode, ClientRevoke, HubBackup, HubBackupSettings,
    HubReachSet, HubReachStatus, HubStatus,
};

use super::Shared;
use crate::{
    ClientError, Command, CommandOutput,
    admin::{PairingCache, local_time_zone},
    connectors::LoadState,
};

/// How often the client asks for the status while a backup runs.
const POLL: Duration = Duration::from_secs(1);

fn parse<T: std::str::FromStr>(id: &str, what: &str) -> Result<T, ClientError> {
    id.parse().map_err(|_| {
        ClientError::Other(format!(
            "Consort cannot use this {what}, because its ID is not valid. Restart Consort, then try again"
        ))
    })
}

/// An `unsupported` answer in words that say what the hub cannot do yet.
fn older_hub(err: ClientError) -> ClientError {
    match err {
        ClientError::Hub { code, .. } if code == "unsupported" => ClientError::Hub {
            code,
            message: "This hub is older than the app and has no Settings > Hub yet. Update the hub, then try again.".to_owned(),
        },
        other => other,
    }
}

fn set_self_id(shared: &Shared) {
    let me = shared.identity().map(|i| i.config.client_id.to_string());
    if let Ok(mut cache) = shared.cache.lock() {
        cache.admin.self_id = me;
    }
}

/// Asks `hub.status` once and puts the answer in the cache. Returns
/// whether a backup runs.
async fn load_status(shared: &Arc<Shared>) -> Result<bool, ClientError> {
    match shared.call(HubStatus {}).await {
        Ok(result) => {
            let running = result.status.backup.running.is_some();
            if let Ok(mut cache) = shared.cache.lock() {
                cache.admin.hub = Some(result.status);
                cache.admin.hub_load = LoadState::Loaded;
            }
            shared.publish();
            Ok(running)
        }
        Err(err) => {
            let err = older_hub(err);
            if let Ok(mut cache) = shared.cache.lock() {
                cache.admin.hub_load = LoadState::Failed(err.to_string());
            }
            shared.publish();
            Err(err)
        }
    }
}

/// Asks for the status every second until the backup ends. Only one loop
/// runs at a time.
fn poll_while_running(shared: &Arc<Shared>) {
    let start = shared.cache.lock().is_ok_and(|mut cache| {
        let start = !cache.admin.polling;
        cache.admin.polling = true;
        start
    });
    if !start {
        return;
    }
    let shared = shared.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(POLL).await;
            match load_status(&shared).await {
                Ok(true) => continue,
                _ => break,
            }
        }
        if let Ok(mut cache) = shared.cache.lock() {
            cache.admin.polling = false;
        }
    });
}

/// Runs one Settings > Hub or Settings > Clients command.
pub(super) async fn run(
    shared: &Arc<Shared>,
    command: Command,
) -> Result<CommandOutput, ClientError> {
    match command {
        Command::LoadHubStatus => {
            if let Ok(mut cache) = shared.cache.lock()
                && cache.admin.hub.is_none()
            {
                cache.admin.hub_load = LoadState::Loading;
            }
            shared.publish();
            if load_status(shared).await? {
                poll_while_running(shared);
            }
        }
        Command::BackUpNow => {
            if let Ok(mut cache) = shared.cache.lock() {
                cache.admin.backup_starting = true;
                cache.admin.backup_error = None;
            }
            shared.publish();
            let started = shared.call(HubBackup {}).await;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.admin.backup_starting = false;
                match &started {
                    Ok(result) => {
                        if let Some(hub) = &mut cache.admin.hub {
                            hub.backup.running = Some(result.progress);
                        }
                    }
                    Err(err) => cache.admin.backup_error = Some(err.to_string()),
                }
            }
            shared.publish();
            started.map_err(older_hub)?;
            poll_while_running(shared);
        }
        Command::UpdateBackupSettings(changes) => {
            let current = shared
                .cache
                .lock()
                .ok()
                .and_then(|c| {
                    c.admin
                        .hub
                        .as_ref()
                        .map(|h| h.backup.settings.schedule.clone())
                })
                .unwrap_or_default();
            let schedule_changed =
                changes.frequency.is_some() || changes.time.is_some() || changes.weekday.is_some();
            let schedule = schedule_changed.then(|| BackupSchedule {
                frequency: changes.frequency.unwrap_or(current.frequency),
                time: changes.time.clone().unwrap_or(current.time.clone()),
                weekday: changes.weekday.unwrap_or(current.weekday),
                time_zone: local_time_zone(),
            });
            let result = shared
                .call(HubBackupSettings {
                    schedule,
                    folder: changes.folder.clone(),
                    keep: changes.keep,
                })
                .await;
            if let Ok(mut cache) = shared.cache.lock() {
                match &result {
                    Ok(answer) => {
                        cache.admin.backup_error = None;
                        if let Some(hub) = &mut cache.admin.hub {
                            hub.backup.settings = answer.settings.clone();
                        }
                    }
                    Err(err) => cache.admin.backup_error = Some(err.to_string()),
                }
            }
            shared.publish();
            result.map_err(older_hub)?;
        }
        Command::LoadClients => {
            set_self_id(shared);
            if let Ok(mut cache) = shared.cache.lock()
                && cache.admin.clients.is_empty()
            {
                cache.admin.clients_load = LoadState::Loading;
            }
            shared.publish();
            match shared.call(ClientList {}).await {
                Ok(list) => {
                    if let Ok(mut cache) = shared.cache.lock() {
                        cache.admin.clients = list
                            .clients
                            .into_iter()
                            .map(|c| (c.id.to_string(), c))
                            .collect();
                        cache.admin.online = list.online.iter().map(ToString::to_string).collect();
                        cache.admin.clients_load = LoadState::Loaded;
                    }
                    shared.publish();
                }
                Err(err) => {
                    if let Ok(mut cache) = shared.cache.lock() {
                        cache.admin.clients_load = LoadState::Failed(err.to_string());
                    }
                    shared.publish();
                    return Err(err);
                }
            }
        }
        Command::RevokeClient { client_id } => {
            let revoked = shared
                .call(ClientRevoke {
                    client_id: parse(&client_id, "app")?,
                })
                .await;
            if let Ok(mut cache) = shared.cache.lock() {
                match &revoked {
                    Ok(_) => {
                        cache.admin.clients_error = None;
                        cache.admin.online.remove(&client_id);
                        // The feed brings the same change. The row goes at
                        // once, also when the feed is slow.
                        if let Some(client) = cache.admin.clients.get_mut(&client_id) {
                            client.revoked_at = Some(consort_proto::time::Timestamp::now());
                        }
                    }
                    Err(err) => cache.admin.clients_error = Some(err.to_string()),
                }
            }
            shared.publish();
            revoked?;
        }
        Command::ClientPairingCode => {
            set_self_id(shared);
            let result = shared.call(ClientPairingCode {}).await?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.admin.pairing = Some(PairingCache {
                    code: result.code.to_string(),
                    hub_url: result.hub_url,
                    tls_sha256: result.tls_sha256,
                    expires_at: result.expires_at.as_millis(),
                });
            }
            shared.publish();
        }
        Command::CancelClientPairing => {
            if let Ok(mut cache) = shared.cache.lock() {
                cache.admin.pairing = None;
            }
            shared.publish();
        }
        Command::LoadReach => {
            let own_url = shared
                .identity()
                .map(|i| i.config.hub_url)
                .unwrap_or_default();
            if let Ok(mut cache) = shared.cache.lock() {
                cache.admin.own_url = own_url;
                if cache.admin.reach.info.is_none() {
                    cache.admin.reach.load = LoadState::Loading;
                }
            }
            shared.publish();
            let result = shared.call(HubReachStatus {}).await.map_err(older_hub);
            if let Ok(mut cache) = shared.cache.lock() {
                let reach = &mut cache.admin.reach;
                match &result {
                    Ok(answer) => {
                        reach.info = Some(answer.reach.clone());
                        reach.load = LoadState::Loaded;
                    }
                    // A later check repeats this, so the old state stays while
                    // there is one.
                    Err(err) if reach.info.is_none() => {
                        reach.load = LoadState::Failed(err.to_string());
                    }
                    Err(_) => {}
                }
            }
            shared.publish();
            result?;
        }
        Command::SetReach { method, on } => {
            if let Ok(mut cache) = shared.cache.lock() {
                cache.admin.reach.busy = Some(method);
                cache.admin.reach.error = None;
            }
            shared.publish();
            let result = shared
                .call(HubReachSet { method, on })
                .await
                .map_err(older_hub);
            if let Ok(mut cache) = shared.cache.lock() {
                let reach = &mut cache.admin.reach;
                reach.busy = None;
                match &result {
                    Ok(answer) => {
                        reach.info = Some(answer.reach.clone());
                        reach.load = LoadState::Loaded;
                    }
                    Err(err) => reach.error = Some(err.to_string()),
                }
            }
            shared.publish();
            result?;
        }
        other => {
            return Err(ClientError::Other(format!(
                "Consort cannot run {other:?} here. Restart Consort, then try again"
            )));
        }
    }
    Ok(CommandOutput::Done)
}
