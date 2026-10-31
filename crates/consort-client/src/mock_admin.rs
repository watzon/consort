//! The mock content of Settings > Hub, Settings > Clients, the pairing
//! dialog and the notifications, from the phase 3 designs, and what the
//! mock does for their commands.

use std::time::Duration;

use chrono::{Local, TimeZone};
use consort_proto::auth::PublicKey;
use consort_proto::client_api::{
    BackupFrequency, BackupInfo, BackupRecord, BackupSchedule, BackupSettings, BackupTrigger,
    HubInfo, HubStorage, Weekday,
};
use consort_proto::ids::ClientId;
use consort_proto::model::{Client, ClientKind, Os, PairedBy};
use consort_proto::time::Timestamp;

use crate::admin::{
    AdminCache, AdminView, ClientPairingView, LastBackupView, PAIRING_CODE_MS, clients_view,
    hub_view, keep_label, qr_text, time_label,
};
use crate::connectors::LoadState;
use crate::mock_backend_ext::Later;
use crate::notify::{AttentionView, WaitingItem};
use crate::view::ViewState;
use crate::{ClientError, Command, CommandOutput, format};

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;
/// The hub address of the designs.
pub const MOCK_HUB_URL: &str = "wss://hub.example.com:7443";
/// The pairing code of the designs.
pub const MOCK_PAIRING_CODE: &str = "KQ7M-4XPD";

/// Today at a local time, in Unix milliseconds.
fn today_at(hour: u32, minute: u32) -> i64 {
    let now = Local::now();
    now.date_naive()
        .and_hms_opt(hour, minute, 0)
        .and_then(|t| Local.from_local_datetime(&t).earliest())
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|| now.timestamp_millis())
}

/// A day of this year at noon, in Unix milliseconds.
fn this_year(month: u32, day: u32) -> i64 {
    use chrono::Datelike;
    let year = Local::now().year();
    Local
        .with_ymd_and_hms(year, month, day, 12, 0, 0)
        .earliest()
        .map(|t| t.timestamp_millis())
        .unwrap_or_default()
}

fn mock_client(
    id: ClientId,
    name: &str,
    os: Os,
    paired_by: PairedBy,
    paired_at: i64,
    seen: i64,
) -> Client {
    Client {
        id,
        name: name.to_owned(),
        kind: ClientKind::Desktop,
        public_key: PublicKey::from_bytes([3; 32]),
        push_provider: None,
        paired_at: Timestamp::from_millis(paired_at),
        last_seen_at: Some(Timestamp::from_millis(seen)),
        revoked_at: None,
        os: Some(os),
        paired_by: Some(paired_by),
    }
}

/// The hub status and the four paired apps of the designs.
pub fn cache() -> AdminCache {
    let now = format::now_ms();
    let hub = HubInfo {
        hub_url: MOCK_HUB_URL.to_owned(),
        hub_version: "0.6.0".to_owned(),
        protocol_versions: consort_proto::frame::SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
        data_dir: "/var/lib/consort".to_owned(),
        storage: HubStorage {
            database_bytes: 312 * MB,
            blobs_bytes: 3_891 * MB,
            skills_bytes: 96 * MB,
            free_bytes: Some(41 * GB),
        },
        backup: BackupInfo {
            settings: BackupSettings {
                schedule: BackupSchedule {
                    frequency: BackupFrequency::Daily,
                    time: "03:00".to_owned(),
                    weekday: Weekday::Sunday,
                    time_zone: "America/Chicago".to_owned(),
                },
                folder: "/srv/backups/consort".to_owned(),
                keep: 7,
            },
            last: Some(BackupRecord {
                file_name: format!("consort-{}-0300.tar", Local::now().format("%Y-%m-%d")),
                path: String::new(),
                size_bytes: 4_200 * MB,
                trigger: BackupTrigger::Scheduled,
                started_at: Timestamp::from_millis(today_at(3, 0)),
                finished_at: Timestamp::from_millis(today_at(3, 2)),
                error: None,
            }),
            running: None,
        },
    };
    let mac = ClientId::new();
    let clients = vec![
        mock_client(
            mac,
            "MacBook Pro",
            Os::Macos,
            PairedBy::HubMachine,
            this_year(10, 12),
            now,
        ),
        mock_client(
            ClientId::new(),
            "bigboi",
            Os::Windows,
            PairedBy::Client { client_id: mac },
            this_year(10, 20),
            now - 2 * 3_600_000,
        ),
        mock_client(
            ClientId::new(),
            "Mac mini",
            Os::Macos,
            PairedBy::HubMachine,
            this_year(8, 3),
            now - 41 * 86_400_000,
        ),
        mock_client(
            ClientId::new(),
            "ThinkPad",
            Os::Linux,
            PairedBy::Client { client_id: mac },
            this_year(10, 27),
            now,
        ),
    ];
    let online = [clients[0].id, clients[3].id]
        .iter()
        .map(ToString::to_string)
        .collect();
    AdminCache {
        hub: Some(hub),
        hub_load: LoadState::Loaded,
        clients: clients.into_iter().map(|c| (c.id.to_string(), c)).collect(),
        online,
        clients_load: LoadState::Loaded,
        self_id: Some(mac.to_string()),
        ..AdminCache::default()
    }
}

/// What needs the user in the mock: the count of the Dock badge preview
/// and the approval of the example banner.
fn attention() -> AttentionView {
    AttentionView {
        loaded: true,
        count: 3,
        waiting: vec![WaitingItem {
            id: "apr_mock_sayso".to_owned(),
            decision: false,
            channel_id: "chn_0192f0c4-sayso".to_owned(),
            channel_name: "sayso".to_owned(),
            task_id: Some("tsk_0192f0c4-sayso-issues".to_owned()),
            task_title: Some("Go through open Sayso issues".to_owned()),
            text: "Run gh issue list --repo watzon/sayso --state open".to_owned(),
        }],
        tasks: Vec::new(),
        messages: Vec::new(),
        quiet: false,
        expired_accounts: Vec::new(),
    }
}

/// The admin part of the mock view state.
pub fn admin() -> AdminView {
    let cache = cache();
    let now = format::now_ms();
    AdminView {
        hub: hub_view(&cache, true, now),
        clients: clients_view(&cache, now),
        pairing: None,
        attention: attention(),
    }
}

/// The pairing dialog of the designs: the code with 9:42 left.
pub fn pairing() -> ClientPairingView {
    ClientPairingView {
        code: MOCK_PAIRING_CODE.to_owned(),
        hub_url: MOCK_HUB_URL.to_owned(),
        expires_at: format::now_ms() + 582_000,
        qr_text: qr_text(MOCK_HUB_URL, MOCK_PAIRING_CODE),
        paired: None,
    }
}

/// How long the mock backup takes.
const MOCK_BACKUP: Duration = Duration::from_millis(2_000);

/// Applies a Settings > Hub or Settings > Clients command to the mock.
pub(crate) fn apply(
    state: &mut ViewState,
    command: Command,
    later: &mut Vec<Later>,
) -> Result<CommandOutput, ClientError> {
    let backup = &mut state.admin.hub.backup;
    match command {
        Command::LoadHubStatus | Command::LoadClients => {}
        Command::BackUpNow => {
            if backup.busy {
                return Err(ClientError::Hub {
                    code: "conflict".into(),
                    message: "A backup runs already. Wait until it finishes, then try again."
                        .into(),
                });
            }
            let started = format::now_ms();
            backup.busy = true;
            backup.last = LastBackupView::Running {
                line: "Backing up. 0 B of 4.2 GB.".into(),
                share: 0.0,
                detail: format!("Started today at {}", format::time_label(started)),
            };
            later.push((
                MOCK_BACKUP,
                Box::new(move |s: &mut ViewState| {
                    let backup = &mut s.admin.hub.backup;
                    backup.busy = false;
                    backup.last = LastBackupView::Done {
                        line: format!(
                            "Finished today at {} in less than a minute",
                            format::time_label(started)
                        ),
                        file_name: format!("consort-{}.tar", Local::now().format("%Y-%m-%d-%H%M")),
                        size: "4.1 GB".into(),
                    };
                }),
            ));
        }
        Command::UpdateBackupSettings(changes) => {
            if let Some(folder) = changes.folder {
                if !folder.starts_with('/') {
                    let message = format!(
                        "The backup folder must be a full path on the hub machine, such as /srv/backups/consort. \"{folder}\" is not."
                    );
                    backup.error = Some(message.clone());
                    return Err(ClientError::Hub {
                        code: "invalid".into(),
                        message,
                    });
                }
                backup.folder = folder;
            }
            if let Some(frequency) = changes.frequency {
                backup.frequency = frequency;
            }
            if let Some(time) = changes.time {
                backup.time_label = time_label(&time);
                backup.time = time;
            }
            if let Some(weekday) = changes.weekday {
                backup.weekday = weekday;
                backup.weekday_label = format!("On {}", weekday.name());
            }
            if let Some(keep) = changes.keep {
                backup.keep = keep;
                backup.keep_label = keep_label(keep);
            }
            backup.error = None;
        }
        Command::RevokeClient { client_id } => {
            let clients = &mut state.admin.clients;
            if clients.rows.iter().any(|r| r.id == client_id && r.this_app) {
                let message =
                    "You cannot revoke the client that you are using. Use another client to remove this one.".to_owned();
                clients.error = Some(message.clone());
                return Err(ClientError::Hub {
                    code: "conflict".into(),
                    message,
                });
            }
            clients.rows.retain(|r| r.id != client_id);
            clients.error = None;
        }
        Command::ClientPairingCode => {
            state.admin.pairing = Some(ClientPairingView {
                expires_at: format::now_ms() + PAIRING_CODE_MS,
                ..pairing()
            });
        }
        Command::CancelClientPairing => state.admin.pairing = None,
        other => {
            return Err(ClientError::Other(format!(
                "The mock cannot run {other:?} here"
            )));
        }
    }
    Ok(CommandOutput::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_has_the_design_content() {
        let admin = admin();
        assert_eq!(admin.hub.address, MOCK_HUB_URL);
        assert_eq!(
            admin.hub.storage.summary,
            "4.2 GB used, 41 GB free on the hub disk"
        );
        assert_eq!(admin.hub.backup.time_label, "At 3:00 AM");
        let names: Vec<&str> = admin.clients.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["MacBook Pro", "bigboi", "Mac mini", "ThinkPad"]);
        assert!(admin.clients.rows[0].this_app);
        assert_eq!(admin.clients.rows[1].status, "Seen 2 hours ago");
        assert_eq!(admin.attention.count, 3);
    }

    #[test]
    fn mock_commands_change_the_view() {
        let mut state = ViewState {
            admin: admin(),
            ..ViewState::default()
        };
        let mut later = Vec::new();
        apply(&mut state, Command::BackUpNow, &mut later).unwrap();
        assert!(state.admin.hub.backup.busy);
        assert!(apply(&mut state, Command::BackUpNow, &mut later).is_err());
        let (_, finish) = later.pop().unwrap();
        finish(&mut state);
        assert!(matches!(
            state.admin.hub.backup.last,
            LastBackupView::Done { .. }
        ));
        apply(
            &mut state,
            Command::UpdateBackupSettings(crate::admin::BackupChanges {
                time: Some("05:00".into()),
                keep: Some(3),
                ..Default::default()
            }),
            &mut later,
        )
        .unwrap();
        assert_eq!(state.admin.hub.backup.time_label, "At 5:00 AM");
        assert_eq!(state.admin.hub.backup.keep_label, "The last 3 backups");
        let me = state.admin.clients.rows[0].id.clone();
        assert!(
            apply(
                &mut state,
                Command::RevokeClient { client_id: me },
                &mut later
            )
            .is_err()
        );
        let other = state.admin.clients.rows[1].id.clone();
        apply(
            &mut state,
            Command::RevokeClient { client_id: other },
            &mut later,
        )
        .unwrap();
        assert_eq!(state.admin.clients.rows.len(), 3);
        apply(&mut state, Command::ClientPairingCode, &mut later).unwrap();
        assert_eq!(
            state.admin.pairing.as_ref().unwrap().code,
            MOCK_PAIRING_CODE
        );
        apply(&mut state, Command::CancelClientPairing, &mut later).unwrap();
        assert!(state.admin.pairing.is_none());
    }
}
