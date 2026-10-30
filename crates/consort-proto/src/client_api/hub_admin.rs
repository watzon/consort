//! Client requests about the hub itself: its address, versions, data
//! folder and storage (`hub.status`), and its backups (`hub.backup` and
//! `hub.backup_settings`).

use serde::{Deserialize, Serialize};

use crate::model::ValidationError;
use crate::time::Timestamp;

/// The least number of backups that the hub keeps.
pub const BACKUP_KEEP_MIN: u32 = 1;
/// The most backups that the hub keeps.
pub const BACKUP_KEEP_MAX: u32 = 100;
/// How many backups the hub keeps when the user did not choose.
pub const BACKUP_KEEP_DEFAULT: u32 = 7;
/// The time of a scheduled backup when the user did not choose.
pub const BACKUP_TIME_DEFAULT: &str = "03:00";

/// `hub.status`: the address, versions, data folder, storage and backup
/// state of the hub.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubStatus {}

/// The response to `hub.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubStatusResult {
    /// The state of the hub.
    pub status: HubInfo,
}

/// What a client shows in Settings > Hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubInfo {
    /// The URL that clients and workers use to reach the hub.
    pub hub_url: String,
    /// The hub's software version.
    pub hub_version: String,
    /// The protocol versions that the hub speaks, newest last.
    pub protocol_versions: Vec<u32>,
    /// The data folder on the hub machine.
    pub data_dir: String,
    /// What Consort keeps on the hub disk.
    pub storage: HubStorage,
    /// The backup settings, the last backup and the backup that runs now.
    pub backup: BackupInfo,
}

/// The space that Consort uses on the hub disk, by kind, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubStorage {
    /// `hub.db` with its write-ahead log.
    pub database_bytes: u64,
    /// The blobs that no skill uses: attachments, channel files and
    /// artifacts.
    pub blobs_bytes: u64,
    /// The blobs of installed skills.
    pub skills_bytes: u64,
    /// The free space on the disk of the data folder. Empty when the hub
    /// cannot read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_bytes: Option<u64>,
}

impl HubStorage {
    /// The space that Consort uses, all kinds together.
    pub fn used_bytes(&self) -> u64 {
        self.database_bytes
            .saturating_add(self.blobs_bytes)
            .saturating_add(self.skills_bytes)
    }
}

/// How often the hub makes a backup on its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupFrequency {
    /// Only when the user chooses Back up now.
    #[default]
    Off,
    /// Every day at the time.
    Daily,
    /// Every week on the weekday at the time.
    Weekly,
}

/// A day of the week.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    #[default]
    Sunday,
}

impl Weekday {
    /// Every day, Monday first.
    pub const ALL: [Weekday; 7] = [
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
        Weekday::Sunday,
    ];

    /// Days since Monday: 0 for Monday, 6 for Sunday.
    pub fn days_from_monday(self) -> u32 {
        match self {
            Weekday::Monday => 0,
            Weekday::Tuesday => 1,
            Weekday::Wednesday => 2,
            Weekday::Thursday => 3,
            Weekday::Friday => 4,
            Weekday::Saturday => 5,
            Weekday::Sunday => 6,
        }
    }

    /// The English name, for example "Sunday".
    pub fn name(self) -> &'static str {
        match self {
            Weekday::Monday => "Monday",
            Weekday::Tuesday => "Tuesday",
            Weekday::Wednesday => "Wednesday",
            Weekday::Thursday => "Thursday",
            Weekday::Friday => "Friday",
            Weekday::Saturday => "Saturday",
            Weekday::Sunday => "Sunday",
        }
    }
}

/// When the hub makes a backup on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSchedule {
    /// Off, daily or weekly.
    pub frequency: BackupFrequency,
    /// The time of day, `HH:MM` in 24 hours, in `time_zone`.
    pub time: String,
    /// The day of a weekly backup.
    #[serde(default)]
    pub weekday: Weekday,
    /// An IANA time zone, for example `America/Chicago`. The client that
    /// sets the schedule sends its own. The hub checks it.
    pub time_zone: String,
}

impl Default for BackupSchedule {
    fn default() -> Self {
        Self {
            frequency: BackupFrequency::Off,
            time: BACKUP_TIME_DEFAULT.to_owned(),
            weekday: Weekday::Sunday,
            time_zone: "UTC".to_owned(),
        }
    }
}

impl BackupSchedule {
    /// The hour and the minute of `time`, or `None` when it is not
    /// `HH:MM`.
    pub fn hour_minute(&self) -> Option<(u32, u32)> {
        parse_time(&self.time)
    }

    /// Checks the time and that the time zone is not empty. The hub checks
    /// that it knows the time zone.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.hour_minute().is_none() {
            return Err(ValidationError::new(
                "schedule.time",
                "must be HH:MM from 00:00 to 23:59",
            ));
        }
        if self.time_zone.trim().is_empty() {
            return Err(ValidationError::new("schedule.time_zone", "is empty"));
        }
        Ok(())
    }
}

/// Reads `HH:MM` (24 hours).
pub fn parse_time(text: &str) -> Option<(u32, u32)> {
    let (h, m) = text.trim().split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let hour: u32 = h.parse().ok()?;
    let minute: u32 = m.parse().ok()?;
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// The backup settings of the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSettings {
    /// When the hub makes a backup on its own.
    pub schedule: BackupSchedule,
    /// The absolute path of the backup folder on the hub machine.
    pub folder: String,
    /// How many backups the hub keeps. It deletes the oldest backups after
    /// a new one finishes.
    pub keep: u32,
}

/// What started a backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupTrigger {
    /// Back up now.
    Manual,
    /// The schedule.
    Scheduled,
}

/// One backup that finished or failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupRecord {
    /// The archive file name, for example `consort-2026-10-29-0300.tar`.
    /// Empty when the backup failed before it named a file.
    pub file_name: String,
    /// The absolute path of the archive on the hub machine.
    pub path: String,
    /// The size of the archive.
    pub size_bytes: u64,
    /// What started it.
    pub trigger: BackupTrigger,
    /// When it started.
    pub started_at: Timestamp,
    /// When it finished or failed.
    pub finished_at: Timestamp,
    /// Why it failed. Empty when it worked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A backup that runs now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupProgress {
    /// What started it.
    pub trigger: BackupTrigger,
    /// When it started.
    pub started_at: Timestamp,
    /// The bytes that are in the archive so far.
    pub bytes_done: u64,
    /// The bytes that the archive will hold, as counted at the start.
    pub bytes_total: u64,
}

/// The backup part of `hub.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupInfo {
    /// The settings.
    pub settings: BackupSettings,
    /// The last backup that finished or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<BackupRecord>,
    /// The backup that runs now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<BackupProgress>,
}

/// `hub.backup`: **Back up now**. The hub starts the backup and answers at
/// once. `hub.status` shows its progress and then its result. A backup that
/// runs already makes the hub answer `conflict`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubBackup {}

/// The response to `hub.backup`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubBackupResult {
    /// The backup that started.
    pub progress: BackupProgress,
}

/// `hub.backup_settings`: read the backup settings, or change some of them.
/// A field that is left out does not change, so an empty body only reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubBackupSettings {
    /// A new schedule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<BackupSchedule>,
    /// A new folder: an absolute path on the hub machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// How many backups to keep, from [`BACKUP_KEEP_MIN`] to
    /// [`BACKUP_KEEP_MAX`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<u32>,
}

impl HubBackupSettings {
    /// Checks the fields that are set. The hub checks the folder and the
    /// time zone on its own machine.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(schedule) = &self.schedule {
            schedule.validate()?;
        }
        if let Some(folder) = &self.folder
            && folder.trim().is_empty()
        {
            return Err(ValidationError::new("folder", "is empty"));
        }
        if let Some(keep) = self.keep
            && !(BACKUP_KEEP_MIN..=BACKUP_KEEP_MAX).contains(&keep)
        {
            return Err(ValidationError::new(
                "keep",
                format!("must be from {BACKUP_KEEP_MIN} to {BACKUP_KEEP_MAX}"),
            ));
        }
        Ok(())
    }
}

/// The response to `hub.backup_settings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubBackupSettingsResult {
    /// The settings after the change.
    pub settings: BackupSettings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_must_be_hours_and_minutes() {
        assert_eq!(parse_time("03:00"), Some((3, 0)));
        assert_eq!(parse_time("23:59"), Some((23, 59)));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("3:00"), None);
        assert_eq!(parse_time("03:60"), None);
        assert_eq!(parse_time("noon"), None);
    }

    #[test]
    fn settings_check_the_keep_count_and_the_schedule() {
        let ok = HubBackupSettings {
            keep: Some(7),
            ..HubBackupSettings::default()
        };
        assert!(ok.validate().is_ok());
        for keep in [0, 101] {
            let bad = HubBackupSettings {
                keep: Some(keep),
                ..HubBackupSettings::default()
            };
            assert_eq!(bad.validate().unwrap_err().field, "keep");
        }
        let bad_time = HubBackupSettings {
            schedule: Some(BackupSchedule {
                time: "7am".into(),
                ..BackupSchedule::default()
            }),
            ..HubBackupSettings::default()
        };
        assert_eq!(bad_time.validate().unwrap_err().field, "schedule.time");
        let empty_folder = HubBackupSettings {
            folder: Some("  ".into()),
            ..HubBackupSettings::default()
        };
        assert_eq!(empty_folder.validate().unwrap_err().field, "folder");
    }

    #[test]
    fn a_weekly_schedule_has_its_day_on_the_wire() {
        let schedule = BackupSchedule {
            frequency: BackupFrequency::Weekly,
            time: "03:00".into(),
            weekday: Weekday::Friday,
            time_zone: "Europe/Berlin".into(),
        };
        let json = serde_json::to_value(&schedule).unwrap();
        assert_eq!(json["frequency"], "weekly");
        assert_eq!(json["weekday"], "friday");
        let back: BackupSchedule = serde_json::from_value(json).unwrap();
        assert_eq!(back, schedule);
        // An old body without a day reads as Sunday.
        let old: BackupSchedule = serde_json::from_value(serde_json::json!({
            "frequency": "daily", "time": "03:00", "time_zone": "UTC"
        }))
        .unwrap();
        assert_eq!(old.weekday, Weekday::Sunday);
    }

    #[test]
    fn used_storage_adds_the_kinds() {
        let storage = HubStorage {
            database_bytes: 1,
            blobs_bytes: 2,
            skills_bytes: 3,
            free_bytes: None,
        };
        assert_eq!(storage.used_bytes(), 6);
        let json = serde_json::to_value(storage).unwrap();
        assert!(json.get("free_bytes").is_none());
    }
}
