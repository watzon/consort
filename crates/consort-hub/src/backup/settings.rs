//! The backup settings file and the schedule rule.
//!
//! The settings live in `backup.json` in the data folder, not in `hub.db`,
//! so they need no schema migration. The hub writes the file through a
//! temp file and a rename, so a crash never leaves half a file.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use consort_proto::client_api::{
    BACKUP_KEEP_DEFAULT, BackupFrequency, BackupRecord, BackupSchedule, BackupSettings,
};
use consort_proto::time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::error::{HubError, Result};

/// The name of the settings file in the data folder.
pub(crate) const SETTINGS_FILE: &str = "backup.json";

/// The name of the default backup folder in the data folder.
pub(crate) const DEFAULT_FOLDER: &str = "backups";

/// A scheduled backup that is later than this does not run. The next slot
/// runs instead. The same rule as for routines.
pub(crate) const LATE_LIMIT_MS: i64 = 60 * 60 * 1000;

/// What `backup.json` holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BackupFile {
    pub settings: BackupSettings,
    /// When the schedule last changed. A slot before it does not run.
    #[serde(default)]
    pub schedule_changed_at: Option<Timestamp>,
    /// The last backup that finished or failed.
    #[serde(default)]
    pub last: Option<BackupRecord>,
    /// The schedule slot of the last scheduled backup that started.
    #[serde(default)]
    pub last_slot: Option<Timestamp>,
}

impl BackupFile {
    /// The settings of a new hub: no schedule, the `backups` folder in the
    /// data folder, and the last 7 backups.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            settings: BackupSettings {
                schedule: BackupSchedule::default(),
                folder: data_dir.join(DEFAULT_FOLDER).display().to_string(),
                keep: BACKUP_KEEP_DEFAULT,
            },
            schedule_changed_at: None,
            last: None,
            last_slot: None,
        }
    }

    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(SETTINGS_FILE)
    }

    /// Reads the file. A missing file gives the defaults. A damaged file
    /// gives the defaults too, with a warning, so it never stops the hub.
    pub fn load(data_dir: &Path) -> Self {
        let path = Self::path(data_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                tracing::warn!(path = %path.display(), error = %err, "the backup settings file is damaged, so the hub uses the default settings");
                Self::new(data_dir)
            }),
            Err(_) => Self::new(data_dir),
        }
    }

    /// Writes the file through a temp file and a rename. On Unix it has
    /// mode 0600.
    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        crate::util::write_private_file(&Self::path(data_dir), text.as_bytes())
    }
}

/// Reads an IANA time zone.
pub(crate) fn time_zone(name: &str) -> Result<chrono_tz::Tz> {
    name.trim().parse().map_err(|_| {
        HubError::invalid(format!(
            "The hub does not know the time zone \"{name}\". Choose an IANA time zone, for example America/Chicago."
        ))
    })
}

/// The newest time at or before `now` when the schedule wants a backup.
/// `None` when the schedule is off or cannot be read.
pub(crate) fn latest_slot(schedule: &BackupSchedule, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if schedule.frequency == BackupFrequency::Off {
        return None;
    }
    let (hour, minute) = schedule.hour_minute()?;
    let tz = time_zone(&schedule.time_zone).ok()?;
    let today = now.with_timezone(&tz).date_naive();
    // Go back day by day: at most a week and a day finds a weekly slot, also
    // across a day that has no such time (a DST gap).
    (0..9).find_map(|back| {
        let date: NaiveDate = today.checked_sub_signed(Duration::days(back))?;
        if schedule.frequency == BackupFrequency::Weekly
            && date.weekday().num_days_from_monday() != schedule.weekday.days_from_monday()
        {
            return None;
        }
        let naive = date.and_hms_opt(hour, minute, 0)?;
        let at = tz
            .from_local_datetime(&naive)
            .earliest()?
            .with_timezone(&Utc);
        (at <= now).then_some(at)
    })
}

/// The slot that is due now: the latest slot, when it is after the slot of
/// the last scheduled backup and after the last schedule change, and less
/// than an hour late. A backup that the user started does not move the
/// schedule.
pub(crate) fn due_slot(file: &BackupFile, now: Timestamp) -> Option<Timestamp> {
    let now_utc = DateTime::<Utc>::from_timestamp_millis(now.as_millis())?;
    let slot = latest_slot(&file.settings.schedule, now_utc)?;
    let slot = Timestamp::from_millis(slot.timestamp_millis());
    let after = |t: Option<Timestamp>| t.is_none_or(|t| slot > t);
    let late = now.as_millis() - slot.as_millis();
    (after(file.last_slot) && after(file.schedule_changed_at) && late < LATE_LIMIT_MS)
        .then_some(slot)
}

#[cfg(test)]
mod tests {
    use consort_proto::client_api::Weekday;

    use super::*;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn schedule(frequency: BackupFrequency, weekday: Weekday, tz: &str) -> BackupSchedule {
        BackupSchedule {
            frequency,
            time: "03:00".into(),
            weekday,
            time_zone: tz.into(),
        }
    }

    #[test]
    fn a_daily_slot_is_today_after_the_time_and_yesterday_before_it() {
        let s = schedule(BackupFrequency::Daily, Weekday::Sunday, "America/Chicago");
        // 3:00 AM in Chicago is 08:00 UTC in October (CDT).
        assert_eq!(
            latest_slot(&s, utc("2026-10-29T09:00:00Z")),
            Some(utc("2026-10-29T08:00:00Z"))
        );
        assert_eq!(
            latest_slot(&s, utc("2026-10-29T07:59:00Z")),
            Some(utc("2026-10-28T08:00:00Z"))
        );
    }

    #[test]
    fn a_weekly_slot_falls_on_its_day() {
        // 2026-10-29 is a Thursday.
        let s = schedule(BackupFrequency::Weekly, Weekday::Monday, "UTC");
        assert_eq!(
            latest_slot(&s, utc("2026-10-29T12:00:00Z")),
            Some(utc("2026-10-26T03:00:00Z"))
        );
        let thursday = schedule(BackupFrequency::Weekly, Weekday::Thursday, "UTC");
        assert_eq!(
            latest_slot(&thursday, utc("2026-10-29T02:00:00Z")),
            Some(utc("2026-10-22T03:00:00Z"))
        );
    }

    #[test]
    fn off_and_an_unknown_zone_have_no_slot() {
        let off = schedule(BackupFrequency::Off, Weekday::Sunday, "UTC");
        assert_eq!(latest_slot(&off, utc("2026-10-29T12:00:00Z")), None);
        let bad = schedule(BackupFrequency::Daily, Weekday::Sunday, "Mars/Olympus");
        assert_eq!(latest_slot(&bad, utc("2026-10-29T12:00:00Z")), None);
    }

    #[test]
    fn a_slot_runs_once_and_not_when_it_is_an_hour_late() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = BackupFile::new(dir.path());
        file.settings.schedule = schedule(BackupFrequency::Daily, Weekday::Sunday, "UTC");
        let at = |text: &str| Timestamp::from_millis(utc(text).timestamp_millis());
        let slot = at("2026-10-29T03:00:00Z");
        assert_eq!(due_slot(&file, at("2026-10-29T03:00:30Z")), Some(slot));
        assert_eq!(due_slot(&file, at("2026-10-29T04:00:00Z")), None);
        file.last_slot = Some(slot);
        assert_eq!(due_slot(&file, at("2026-10-29T03:02:00Z")), None);
        // A schedule that changed after the slot waits for the next one.
        file.last_slot = None;
        file.schedule_changed_at = Some(at("2026-10-29T03:00:10Z"));
        assert_eq!(due_slot(&file, at("2026-10-29T03:00:30Z")), None);
    }

    #[test]
    fn a_missing_or_damaged_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = BackupFile::load(dir.path());
        assert_eq!(file.settings.keep, 7);
        assert_eq!(file.settings.schedule.frequency, BackupFrequency::Off);
        assert!(file.settings.folder.ends_with("backups"));
        std::fs::write(dir.path().join(SETTINGS_FILE), "{ not json").unwrap();
        assert_eq!(BackupFile::load(dir.path()), BackupFile::new(dir.path()));
        let mut changed = BackupFile::new(dir.path());
        changed.settings.keep = 3;
        changed.save(dir.path()).unwrap();
        assert_eq!(BackupFile::load(dir.path()).settings.keep, 3);
    }
}
