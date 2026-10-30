//! Settings > Hub and Settings > Clients: the views, their labels, and the
//! cache of the hub status and the paired clients.
//!
//! The hub sends `client` feed entries when a client pairs, connects,
//! leaves or is removed, but no snapshot holds clients, so the client loads
//! them with `client.list` when the page opens. The hub status has no feed
//! entity: the client asks `hub.status` when the page opens, and asks again
//! every second while a backup runs.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, Local, TimeZone};
pub use consort_proto::client_api::{BackupFrequency, Weekday};
use consort_proto::client_api::{BackupRecord, HubInfo};
use consort_proto::model::{Client, ClientKind, Os, PairedBy};

use crate::connectors::LoadState;
use crate::format;

/// The command that restores a backup on the hub machine.
pub const RESTORE_COMMAND: &str = "consort hub restore <file>";

/// A change to the backup settings. A field that is `None` does not
/// change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackupChanges {
    pub frequency: Option<BackupFrequency>,
    /// `HH:MM`.
    pub time: Option<String>,
    pub weekday: Option<Weekday>,
    /// An absolute path on the hub machine.
    pub folder: Option<String>,
    pub keep: Option<u32>,
}

/// The IANA time zone of this computer, or UTC when it cannot be read.
pub fn local_time_zone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_owned())
}

/// What the cache keeps for these pages.
#[derive(Clone, Debug, Default)]
pub struct AdminCache {
    pub hub: Option<HubInfo>,
    pub hub_load: LoadState,
    /// Back up now waits for the hub's answer.
    pub backup_starting: bool,
    /// What went wrong with the last backup command or settings change.
    pub backup_error: Option<String>,
    pub clients: BTreeMap<String, Client>,
    pub online: BTreeSet<String>,
    pub clients_load: LoadState,
    pub clients_error: Option<String>,
    /// This app's client ID.
    pub self_id: Option<String>,
    pub pairing: Option<PairingCache>,
    /// A loop asks `hub.status` every second while a backup runs.
    pub polling: bool,
}

/// A pairing code that this app made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairingCache {
    pub code: String,
    pub hub_url: String,
    pub expires_at: i64,
}

/// Everything that the Hub and Clients pages and the notifications draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdminView {
    pub hub: HubStatusView,
    pub clients: ClientsView,
    /// The Pair a new app dialog.
    pub pairing: Option<ClientPairingView>,
    /// What needs the user, for notifications and the Dock badge.
    pub attention: crate::notify::AttentionView,
}

/// Settings > Hub.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HubStatusView {
    pub load: LoadState,
    /// The link to the hub is up.
    pub online: bool,
    /// "Connected. The hub runs on hub.example.com and keeps every channel,
    /// task and memory note."
    pub summary: String,
    pub address: String,
    pub hub_version: String,
    /// The newest protocol that the hub speaks, as text.
    pub protocol: String,
    /// "This app also speaks protocol 1."
    pub protocol_line: String,
    pub data_dir: String,
    pub storage: StorageView,
    pub backup: BackupView,
}

/// The storage bar and its legend.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StorageView {
    /// "4.2 GB used, 41 GB free on the hub disk".
    pub summary: String,
    /// Database, blobs and skills, in that order.
    pub parts: Vec<StoragePart>,
}

/// One kind in the storage bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoragePart {
    pub label: String,
    pub size: String,
    /// The part of the used space, from 0 to 1.
    pub share: f32,
}

/// The Backup section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackupView {
    pub frequency: BackupFrequency,
    /// `HH:MM`.
    pub time: String,
    /// "At 3:00 AM".
    pub time_label: String,
    pub weekday: Weekday,
    /// "On Sunday".
    pub weekday_label: String,
    pub time_zone: String,
    pub folder: String,
    /// "A folder on the hub machine, not on this Mac."
    pub folder_hint: String,
    pub keep: u32,
    /// "The last 7 backups".
    pub keep_label: String,
    pub last: LastBackupView,
    /// Back up now waits for the hub, or a backup runs.
    pub busy: bool,
    pub error: Option<String>,
    pub restore_command: String,
}

/// The state of the Last backup row.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum LastBackupView {
    /// No backup yet.
    #[default]
    None,
    /// A backup runs now.
    Running {
        /// "Backing up. 1.2 GB of 4.2 GB."
        line: String,
        /// From 0 to 1.
        share: f32,
        /// "Started at 3:00 AM".
        detail: String,
    },
    /// The last backup worked.
    Done {
        /// "Finished today at 3:00 AM in 2 minutes".
        line: String,
        file_name: String,
        size: String,
    },
    /// The last backup failed.
    Failed { line: String, error: String },
}

/// Settings > Clients.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientsView {
    pub load: LoadState,
    /// This app first, then the others by name. Removed apps are not here.
    pub rows: Vec<ClientRowView>,
    /// What went wrong with the last revoke.
    pub error: Option<String>,
}

/// One paired app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientRowView {
    pub id: String,
    pub name: String,
    /// The app that you use. It has no Revoke.
    pub this_app: bool,
    /// "Desktop app on macOS. Paired with a code from the hub CLI on
    /// October 12."
    pub detail: String,
    pub online: bool,
    /// "Online now", "Seen 2 hours ago" or "Never connected".
    pub status: String,
    pub phone: bool,
}

/// The Pair a new app dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientPairingView {
    /// `KQ7M-4XPD`.
    pub code: String,
    pub hub_url: String,
    pub expires_at: i64,
    /// What the QR code holds: the hub address and the code as JSON.
    pub qr_text: String,
    /// The name of the app that paired with this code, once it did.
    pub paired: Option<String>,
}

/// How long a pairing code works.
pub const PAIRING_CODE_MS: i64 = consort_proto::auth::PAIRING_CODE_TTL_MS;

/// A size in a few characters, as Settings > Hub shows it: "312 MB",
/// "96 MB", "4.1 GB", "41 GB".
pub fn storage_size_label(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b < K {
        return format!("{bytes} B");
    }
    if b < K * K {
        return format!("{} KB", (b / K).round() as u64);
    }
    if b < K * K * K {
        return format!("{} MB", (b / (K * K)).round() as u64);
    }
    let units = ["GB", "TB", "PB"];
    let mut value = b / (K * K * K);
    let mut unit = 0;
    while value >= K && unit + 1 < units.len() {
        value /= K;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", units[unit])
    } else {
        format!("{} {}", value.round() as u64, units[unit])
    }
}

/// "At 3:00 AM" for `03:00`.
pub fn time_label(time: &str) -> String {
    match consort_proto::client_api::parse_time(time) {
        Some((h, m)) => {
            let (hour, half) = match h {
                0 => (12, "AM"),
                1..=11 => (h, "AM"),
                12 => (12, "PM"),
                _ => (h - 12, "PM"),
            };
            format!("At {hour}:{m:02} {half}")
        }
        None => format!("At {time}"),
    }
}

/// The times that the time select offers: every hour.
pub fn backup_times() -> Vec<(String, String)> {
    (0..24)
        .map(|h| {
            let time = format!("{h:02}:00");
            let label = time_label(&time);
            (time, label)
        })
        .collect()
}

/// The counts that the Keep select offers.
pub const KEEP_CHOICES: [u32; 7] = [1, 3, 5, 7, 14, 30, 60];

/// "The last 7 backups", or "The last backup" for 1.
pub fn keep_label(keep: u32) -> String {
    if keep == 1 {
        "The last backup".to_owned()
    } else {
        format!("The last {keep} backups")
    }
}

/// The name of this kind of computer, for "not on this Mac".
fn this_computer() -> &'static str {
    match std::env::consts::OS {
        "macos" => "this Mac",
        "windows" => "this PC",
        _ => "this computer",
    }
}

/// "today at 3:00 AM", "yesterday at 3:00 AM", "on October 12 at 3:00 AM".
fn when_label(ms: i64, now_ms: i64) -> String {
    let day = match format::day_label(ms, now_ms).as_str() {
        "Today" => "today".to_owned(),
        "Yesterday" => "yesterday".to_owned(),
        other => format!("on {other}"),
    };
    format!("{day} at {}", format::time_label(ms))
}

/// "in 2 minutes", "in less than a minute", "in 1 hour 5 minutes".
fn duration_label(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    let plural = |n: i64, word: &str| {
        if n == 1 {
            format!("1 {word}")
        } else {
            format!("{n} {word}s")
        }
    };
    match minutes {
        0 => "in less than a minute".to_owned(),
        1..=59 => format!("in {}", plural(minutes, "minute")),
        _ if minutes % 60 == 0 => format!("in {}", plural(minutes / 60, "hour")),
        _ => format!(
            "in {} {}",
            plural(minutes / 60, "hour"),
            plural(minutes % 60, "minute")
        ),
    }
}

fn last_backup(info: &HubInfo, now: i64) -> LastBackupView {
    if let Some(run) = &info.backup.running {
        let share = if run.bytes_total == 0 {
            0.0
        } else {
            (run.bytes_done as f64 / run.bytes_total as f64).clamp(0.0, 1.0) as f32
        };
        return LastBackupView::Running {
            line: format!(
                "Backing up. {} of {}.",
                storage_size_label(run.bytes_done),
                storage_size_label(run.bytes_total)
            ),
            share,
            detail: format!("Started {}", when_label(run.started_at.as_millis(), now)),
        };
    }
    match &info.backup.last {
        None => LastBackupView::None,
        Some(BackupRecord {
            error: Some(error),
            finished_at,
            ..
        }) => LastBackupView::Failed {
            line: format!("Failed {}", when_label(finished_at.as_millis(), now)),
            error: error.clone(),
        },
        Some(record) => LastBackupView::Done {
            line: format!(
                "Finished {} {}",
                when_label(record.started_at.as_millis(), now),
                duration_label(record.finished_at.as_millis() - record.started_at.as_millis())
            ),
            file_name: record.file_name.clone(),
            size: storage_size_label(record.size_bytes),
        },
    }
}

/// The host of a hub URL, for the summary line.
fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}

/// Builds Settings > Hub.
pub fn hub_view(cache: &AdminCache, online: bool, now: i64) -> HubStatusView {
    let Some(info) = &cache.hub else {
        return HubStatusView {
            load: cache.hub_load.clone(),
            online,
            ..HubStatusView::default()
        };
    };
    let protocol = info.protocol_versions.iter().max().copied().unwrap_or(0);
    let ours = consort_proto::frame::SUPPORTED_PROTOCOL_VERSIONS;
    let protocol_line = if ours.contains(&protocol) {
        format!("This app also speaks protocol {protocol}.")
    } else {
        let mine = ours.iter().max().copied().unwrap_or(0);
        format!(
            "This app speaks protocol {mine}. Update the {} so both speak the same protocol.",
            if mine < protocol { "app" } else { "hub" }
        )
    };
    let host = host_of(&info.hub_url);
    let summary = if online {
        format!("Connected. The hub runs on {host} and keeps every channel, task and memory note.")
    } else {
        format!(
            "Not connected. Consort tries to reach the hub on {host} again. The values below may be old."
        )
    };
    let s = info.storage;
    let used = s.used_bytes();
    let share = |bytes: u64| {
        if used == 0 {
            0.0
        } else {
            (bytes as f64 / used as f64) as f32
        }
    };
    let free = s
        .free_bytes
        .map(|f| format!(", {} free on the hub disk", storage_size_label(f)))
        .unwrap_or_default();
    let schedule = &info.backup.settings.schedule;
    let mut last = last_backup(info, now);
    if cache.backup_starting
        && matches!(
            last,
            LastBackupView::None | LastBackupView::Done { .. } | LastBackupView::Failed { .. }
        )
    {
        last = LastBackupView::Running {
            line: "Starting the backup.".to_owned(),
            share: 0.0,
            detail: String::new(),
        };
    }
    HubStatusView {
        load: cache.hub_load.clone(),
        online,
        summary,
        address: info.hub_url.clone(),
        hub_version: info.hub_version.clone(),
        protocol: protocol.to_string(),
        protocol_line,
        data_dir: info.data_dir.clone(),
        storage: StorageView {
            summary: format!("{} used{free}", storage_size_label(used)),
            parts: vec![
                StoragePart {
                    label: "Database".to_owned(),
                    size: storage_size_label(s.database_bytes),
                    share: share(s.database_bytes),
                },
                StoragePart {
                    label: "Blobs".to_owned(),
                    size: storage_size_label(s.blobs_bytes),
                    share: share(s.blobs_bytes),
                },
                StoragePart {
                    label: "Skills".to_owned(),
                    size: storage_size_label(s.skills_bytes),
                    share: share(s.skills_bytes),
                },
            ],
        },
        backup: BackupView {
            frequency: schedule.frequency,
            time: schedule.time.clone(),
            time_label: time_label(&schedule.time),
            weekday: schedule.weekday,
            weekday_label: format!("On {}", schedule.weekday.name()),
            time_zone: schedule.time_zone.clone(),
            folder: info.backup.settings.folder.clone(),
            folder_hint: format!("A folder on the hub machine, not on {}.", this_computer()),
            keep: info.backup.settings.keep,
            keep_label: keep_label(info.backup.settings.keep),
            busy: matches!(last, LastBackupView::Running { .. }),
            last,
            error: cache.backup_error.clone(),
            restore_command: RESTORE_COMMAND.to_owned(),
        },
    }
}

fn os_name(os: Os) -> &'static str {
    match os {
        Os::Macos => "macOS",
        Os::Windows => "Windows",
        Os::Linux => "Linux",
    }
}

/// "October 12", with the year when it is not this year.
fn date_label(ms: i64, now: i64) -> String {
    let (Some(day), Some(today)) = (
        Local.timestamp_millis_opt(ms).single(),
        Local.timestamp_millis_opt(now).single(),
    ) else {
        return String::new();
    };
    if day.year() == today.year() {
        day.format("%B %-d").to_string()
    } else {
        day.format("%B %-d, %Y").to_string()
    }
}

/// "Seen 2 hours ago".
fn seen_label(ms: i64, now: i64) -> String {
    let minutes = (now - ms).max(0) / 60_000;
    let ago = |n: i64, word: &str| {
        if n == 1 {
            format!("Seen 1 {word} ago")
        } else {
            format!("Seen {n} {word}s ago")
        }
    };
    match minutes {
        0 => "Seen just now".to_owned(),
        1..=59 => ago(minutes, "minute"),
        60..=1439 => ago(minutes / 60, "hour"),
        _ => ago(minutes / 1440, "day"),
    }
}

/// The detail line of a client row.
pub fn client_detail(client: &Client, names: &BTreeMap<String, String>, now: i64) -> String {
    let kind = match client.kind {
        ClientKind::Desktop => "Desktop app",
        ClientKind::Ios => "iPhone app",
        ClientKind::Android => "Android app",
    };
    let on = client
        .os
        .map(|os| format!(" on {}", os_name(os)))
        .unwrap_or_default();
    let by = match client.paired_by {
        Some(PairedBy::HubMachine) => "Paired with a code from the hub CLI".to_owned(),
        Some(PairedBy::Client { client_id }) => match names.get(&client_id.to_string()) {
            Some(name) => format!("Paired from {name}"),
            None => "Paired from an app that was removed".to_owned(),
        },
        None => "Paired".to_owned(),
    };
    format!(
        "{kind}{on}. {by} on {}.",
        date_label(client.paired_at.as_millis(), now)
    )
}

/// Builds Settings > Clients.
pub fn clients_view(cache: &AdminCache, now: i64) -> ClientsView {
    let names: BTreeMap<String, String> = cache
        .clients
        .iter()
        .map(|(id, c)| (id.clone(), c.name.clone()))
        .collect();
    let mut clients: Vec<&Client> = cache
        .clients
        .values()
        .filter(|c| c.revoked_at.is_none())
        .collect();
    clients.sort_by_key(|c| (c.paired_at, c.id.to_string()));
    let mut rows: Vec<ClientRowView> = clients
        .into_iter()
        .map(|c| {
            let id = c.id.to_string();
            let this_app = cache.self_id.as_deref() == Some(id.as_str());
            let online = this_app || cache.online.contains(&id);
            ClientRowView {
                detail: client_detail(c, &names, now),
                status: if online {
                    "Online now".to_owned()
                } else {
                    match c.last_seen_at {
                        Some(at) => seen_label(at.as_millis(), now),
                        None => "Never connected".to_owned(),
                    }
                },
                id,
                name: c.name.clone(),
                this_app,
                online,
                phone: c.kind != ClientKind::Desktop,
            }
        })
        .collect();
    rows.sort_by_key(|r| (!r.this_app, r.name.to_lowercase()));
    ClientsView {
        load: cache.clients_load.clone(),
        rows,
        error: cache.clients_error.clone(),
    }
}

/// The QR code text: the hub address and the code, as `PairingInvite`.
pub fn qr_text(hub_url: &str, code: &str) -> String {
    // Built by hand, so the field order is the same in every build.
    let quote = |s: &str| serde_json::Value::from(s).to_string();
    format!(
        "{{\"hub_url\":{},\"code\":{}}}",
        quote(hub_url),
        quote(code)
    )
}

/// Builds the pairing dialog, and finds the app that paired with its code.
pub fn pairing_view(cache: &AdminCache) -> Option<ClientPairingView> {
    let pairing = cache.pairing.as_ref()?;
    let made_at = pairing.expires_at - PAIRING_CODE_MS;
    let paired = cache.self_id.as_ref().and_then(|me| {
        cache
            .clients
            .values()
            .filter(|c| {
                c.revoked_at.is_none()
                    && c.paired_at.as_millis() >= made_at
                    && matches!(c.paired_by, Some(PairedBy::Client { client_id }) if client_id.to_string() == *me)
            })
            .max_by_key(|c| c.paired_at)
            .map(|c| c.name.clone())
    });
    Some(ClientPairingView {
        code: pairing.code.clone(),
        hub_url: pairing.hub_url.clone(),
        expires_at: pairing.expires_at,
        qr_text: qr_text(&pairing.hub_url, &pairing.code),
        paired,
    })
}

/// Builds the whole view from the cache.
pub(crate) fn admin_view(cache: &crate::cache::Cache, online: bool, now: i64) -> AdminView {
    AdminView {
        hub: hub_view(&cache.admin, online, now),
        clients: clients_view(&cache.admin, now),
        pairing: pairing_view(&cache.admin),
        attention: crate::notify::attention(cache),
    }
}

/// The countdown of a pairing code: "9:42" and the share of the time that
/// is left, from 0 to 1.
pub fn countdown(expires_at: i64, now: i64) -> (String, f32) {
    let left = (expires_at - now).max(0);
    let seconds = left / 1000;
    let label = format!("{}:{:02}", seconds / 60, seconds % 60);
    let share = (left as f64 / PAIRING_CODE_MS as f64).clamp(0.0, 1.0) as f32;
    (label, share)
}

#[cfg(test)]
mod tests {
    use consort_proto::client_api::{
        BackupInfo, BackupProgress, BackupSchedule, BackupSettings, BackupTrigger, HubStorage,
    };
    use consort_proto::ids::ClientId;
    use consort_proto::time::Timestamp;

    use super::*;

    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;

    fn info(last: Option<BackupRecord>, running: Option<BackupProgress>) -> HubInfo {
        HubInfo {
            hub_url: "https://hub.example.com:7443".into(),
            hub_version: "0.6.0".into(),
            protocol_versions: vec![1],
            data_dir: "/var/lib/consort".into(),
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
                        time: "03:00".into(),
                        weekday: Weekday::Sunday,
                        time_zone: "America/Chicago".into(),
                    },
                    folder: "/srv/backups/consort".into(),
                    keep: 7,
                },
                last,
                running,
            },
        }
    }

    fn local_ms(h: u32, m: u32) -> i64 {
        Local
            .with_ymd_and_hms(2026, 10, 29, h, m, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn sizes_read_like_the_design() {
        assert_eq!(storage_size_label(312 * MB), "312 MB");
        assert_eq!(storage_size_label(96 * MB), "96 MB");
        assert_eq!(storage_size_label(4_300 * MB), "4.2 GB");
        assert_eq!(storage_size_label(41 * GB), "41 GB");
        assert_eq!(storage_size_label(512), "512 B");
    }

    #[test]
    fn the_hub_page_has_the_design_lines() {
        let now = local_ms(9, 41);
        let record = BackupRecord {
            file_name: "consort-2026-10-29-0300.tar".into(),
            path: "/srv/backups/consort/consort-2026-10-29-0300.tar".into(),
            size_bytes: 4_200 * MB,
            trigger: BackupTrigger::Scheduled,
            started_at: Timestamp::from_millis(local_ms(3, 0)),
            finished_at: Timestamp::from_millis(local_ms(3, 2)),
            error: None,
        };
        let cache = AdminCache {
            hub: Some(info(Some(record), None)),
            hub_load: LoadState::Loaded,
            ..AdminCache::default()
        };
        let view = hub_view(&cache, true, now);
        assert_eq!(
            view.summary,
            "Connected. The hub runs on hub.example.com and keeps every channel, task and memory note."
        );
        assert_eq!(view.protocol, "1");
        assert_eq!(view.protocol_line, "This app also speaks protocol 1.");
        assert_eq!(
            view.storage.summary,
            "4.2 GB used, 41 GB free on the hub disk"
        );
        let sizes: Vec<&str> = view.storage.parts.iter().map(|p| p.size.as_str()).collect();
        assert_eq!(sizes, vec!["312 MB", "3.8 GB", "96 MB"]);
        let shares: f32 = view.storage.parts.iter().map(|p| p.share).sum();
        assert!((shares - 1.0).abs() < 0.001);
        assert_eq!(view.backup.time_label, "At 3:00 AM");
        assert_eq!(view.backup.keep_label, "The last 7 backups");
        assert_eq!(
            view.backup.last,
            LastBackupView::Done {
                line: "Finished today at 3:00 AM in 2 minutes".into(),
                file_name: "consort-2026-10-29-0300.tar".into(),
                size: "4.1 GB".into(),
            }
        );
        assert!(!view.backup.busy);
    }

    #[test]
    fn a_running_or_failed_backup_shows_in_the_last_backup_row() {
        let now = local_ms(9, 41);
        let running = BackupProgress {
            trigger: BackupTrigger::Manual,
            started_at: Timestamp::from_millis(local_ms(9, 40)),
            bytes_done: 1_229 * MB,
            bytes_total: 4_300 * MB,
        };
        let cache = AdminCache {
            hub: Some(info(None, Some(running))),
            ..AdminCache::default()
        };
        let view = hub_view(&cache, true, now);
        let LastBackupView::Running {
            line,
            share,
            detail,
        } = &view.backup.last
        else {
            panic!("expected a running backup");
        };
        assert_eq!(line, "Backing up. 1.2 GB of 4.2 GB.");
        assert!((share - 0.2858).abs() < 0.01);
        assert_eq!(detail, "Started today at 9:40 AM");
        assert!(view.backup.busy);

        let failed = BackupRecord {
            file_name: String::new(),
            path: String::new(),
            size_bytes: 0,
            trigger: BackupTrigger::Manual,
            started_at: Timestamp::from_millis(local_ms(3, 0)),
            finished_at: Timestamp::from_millis(local_ms(3, 1)),
            error: Some("The disk is full.".into()),
        };
        let cache = AdminCache {
            hub: Some(info(Some(failed), None)),
            ..AdminCache::default()
        };
        assert_eq!(
            hub_view(&cache, true, now).backup.last,
            LastBackupView::Failed {
                line: "Failed today at 3:01 AM".into(),
                error: "The disk is full.".into()
            }
        );
    }

    #[test]
    fn labels_for_times_durations_and_keep() {
        assert_eq!(time_label("00:00"), "At 12:00 AM");
        assert_eq!(time_label("12:30"), "At 12:30 PM");
        assert_eq!(time_label("23:00"), "At 11:00 PM");
        assert_eq!(backup_times().len(), 24);
        assert_eq!(duration_label(30_000), "in less than a minute");
        assert_eq!(duration_label(60_000), "in 1 minute");
        assert_eq!(duration_label(3_600_000), "in 1 hour");
        assert_eq!(duration_label(3_900_000), "in 1 hour 5 minutes");
        assert_eq!(keep_label(1), "The last backup");
    }

    fn client(name: &str, paired_by: Option<PairedBy>, at: i64, seen: Option<i64>) -> Client {
        Client {
            id: ClientId::new(),
            name: name.into(),
            kind: ClientKind::Desktop,
            public_key: consort_proto::auth::PublicKey::from_bytes([1; 32]),
            push_provider: None,
            paired_at: Timestamp::from_millis(at),
            last_seen_at: seen.map(Timestamp::from_millis),
            revoked_at: None,
            os: Some(Os::Windows),
            paired_by,
        }
    }

    #[test]
    fn client_rows_say_who_paired_them_and_when_they_were_seen() {
        let now = local_ms(12, 0);
        let mac = client(
            "MacBook Pro",
            Some(PairedBy::HubMachine),
            local_ms(1, 0),
            None,
        );
        let pc = client(
            "bigboi",
            Some(PairedBy::Client { client_id: mac.id }),
            local_ms(2, 0),
            Some(now - 2 * 3_600_000),
        );
        let mut gone = client("Old", None, local_ms(3, 0), None);
        gone.revoked_at = Some(Timestamp::from_millis(now));
        let mut cache = AdminCache {
            self_id: Some(mac.id.to_string()),
            ..AdminCache::default()
        };
        for c in [pc.clone(), mac.clone(), gone] {
            cache.clients.insert(c.id.to_string(), c);
        }
        let view = clients_view(&cache, now);
        assert_eq!(view.rows.len(), 2);
        assert!(view.rows[0].this_app);
        assert_eq!(view.rows[0].status, "Online now");
        assert_eq!(
            view.rows[0].detail,
            "Desktop app on Windows. Paired with a code from the hub CLI on October 29."
        );
        assert_eq!(
            view.rows[1].detail,
            "Desktop app on Windows. Paired from MacBook Pro on October 29."
        );
        assert_eq!(view.rows[1].status, "Seen 2 hours ago");
        cache.online.insert(pc.id.to_string());
        assert_eq!(clients_view(&cache, now).rows[1].status, "Online now");
        assert_eq!(seen_label(now - 41 * 86_400_000, now), "Seen 41 days ago");
    }

    #[test]
    fn the_pairing_dialog_sees_the_app_that_used_its_code() {
        let now = local_ms(12, 0);
        let me = client("MacBook Pro", None, local_ms(1, 0), None);
        let mut cache = AdminCache {
            self_id: Some(me.id.to_string()),
            pairing: Some(PairingCache {
                code: "KQ7M-4XPD".into(),
                hub_url: "https://hub.example.com".into(),
                expires_at: now + PAIRING_CODE_MS,
            }),
            ..AdminCache::default()
        };
        let view = pairing_view(&cache).unwrap();
        assert_eq!(view.paired, None);
        assert_eq!(
            view.qr_text,
            r#"{"hub_url":"https://hub.example.com","code":"KQ7M-4XPD"}"#
        );
        // An app that another client paired does not count.
        let other = client("Other", Some(PairedBy::HubMachine), now + 1_000, None);
        cache.clients.insert(other.id.to_string(), other);
        assert_eq!(pairing_view(&cache).unwrap().paired, None);
        let new = client(
            "ThinkPad",
            Some(PairedBy::Client { client_id: me.id }),
            now + 2_000,
            None,
        );
        cache.clients.insert(new.id.to_string(), new);
        assert_eq!(
            pairing_view(&cache).unwrap().paired.as_deref(),
            Some("ThinkPad")
        );
    }

    #[test]
    fn the_countdown_runs_from_ten_minutes() {
        let (label, share) = countdown(1_000_000 + 582_000, 1_000_000);
        assert_eq!(label, "9:42");
        assert!((share - 0.97).abs() < 0.001);
        assert_eq!(countdown(0, 5).0, "0:00");
    }
}
