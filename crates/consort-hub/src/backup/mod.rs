//! Settings > Hub: the status of the hub, and its backups.
//!
//! A backup is one tar archive in the backup folder on the hub machine (see
//! [`archive`]). The settings and the last result live in `backup.json` in
//! the data folder (see [`settings`]), so they need no schema migration.
//! The hub runs scheduled backups itself, and deletes the oldest archives so
//! that the chosen number stays. `consort hub restore` puts an archive back
//! (see [`restore`]).

mod archive;
mod restore;
mod settings;
mod storage;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use consort_proto::client_api::{
    BackupInfo, BackupProgress, BackupRecord, BackupTrigger, HubBackup, HubBackupResult,
    HubBackupSettings, HubBackupSettingsResult, HubInfo, HubStatus, HubStatusResult,
};
use consort_proto::frame::SUPPORTED_PROTOCOL_VERSIONS;
use consort_proto::time::Timestamp;
use tokio::task::JoinHandle;

pub(crate) use restore::try_lock;
pub use restore::{LOCK_FILE, RestoreError, RestoreReport, restore};

use crate::error::{HubError, Result};
use crate::hub::{Caller, HUB_VERSION, Hub};
use settings::BackupFile;

/// How often the hub looks whether a scheduled backup is due.
const SCHEDULE_TICK: Duration = Duration::from_secs(30);

/// A backup that runs now.
struct Running {
    trigger: BackupTrigger,
    started_at: Timestamp,
    done: Arc<AtomicU64>,
    total: u64,
}

impl Running {
    fn progress(&self) -> BackupProgress {
        let done = self.done.load(Ordering::Relaxed);
        BackupProgress {
            trigger: self.trigger,
            started_at: self.started_at,
            // The tar headers make the archive a little larger than the
            // files. The share never goes past the whole.
            bytes_done: done.min(self.total),
            bytes_total: self.total,
        }
    }
}

/// The backup state of a running hub.
pub(crate) struct Backups {
    data_dir: PathBuf,
    file: Mutex<BackupFile>,
    running: Mutex<Option<Running>>,
}

impl Backups {
    pub(crate) fn open(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            file: Mutex::new(BackupFile::load(data_dir)),
            running: Mutex::new(None),
        }
    }

    fn file(&self) -> Result<BackupFile> {
        self.file
            .lock()
            .map(|f| f.clone())
            .map_err(|_| HubError::internal("the backup settings lock is poisoned"))
    }

    fn progress(&self) -> Option<BackupProgress> {
        self.running
            .lock()
            .ok()
            .and_then(|r| r.as_ref().map(Running::progress))
    }

    /// Changes the file and writes it.
    fn update(&self, change: impl FnOnce(&mut BackupFile)) -> Result<BackupFile> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| HubError::internal("the backup settings lock is poisoned"))?;
        let mut next = file.clone();
        change(&mut next);
        next.save(&self.data_dir)?;
        *file = next.clone();
        Ok(next)
    }
}

/// Writes a backup of a data folder into `folder` now, and keeps the newest
/// `keep` archives there. It uses SQLite's online backup, so it works while a
/// hub runs and needs no running hub. A local update uses it.
pub fn back_up_now(data_dir: &Path, folder: &Path, keep: u32) -> Result<PathBuf> {
    let stem = archive_stem(Timestamp::now());
    let done = AtomicU64::new(0);
    let written = archive::write(data_dir, folder, &stem, &done)?;
    archive::prune(folder, keep);
    Ok(written.path)
}

/// The archive name for a backup that starts at `at`: the time in UTC, for
/// example `consort-2026-10-29-0800`. A name in local time can go back
/// (a change of the time zone, or the hour that daylight saving time
/// repeats), so a newer archive could get an older name.
fn archive_stem(at: Timestamp) -> String {
    let utc =
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at.as_millis()).unwrap_or_default();
    format!("{}{}", archive::NAME_PREFIX, utc.format("%Y-%m-%d-%H%M"))
}

impl Hub {
    /// `hub.status`.
    pub(crate) async fn hub_status(
        &self,
        _caller: &Caller,
        _request: HubStatus,
    ) -> Result<HubStatusResult> {
        let skill_blobs = self.store.read(storage::skill_blob_names).await?;
        let data_dir = self.backups.data_dir.clone();
        let storage =
            tokio::task::spawn_blocking(move || storage::measure(&data_dir, &skill_blobs)).await?;
        let file = self.backups.file()?;
        Ok(HubStatusResult {
            status: HubInfo {
                hub_url: self.settings.public_url.clone(),
                hub_version: HUB_VERSION.to_owned(),
                protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
                data_dir: self.backups.data_dir.display().to_string(),
                storage,
                backup: BackupInfo {
                    settings: file.settings,
                    last: file.last,
                    running: self.backups.progress(),
                },
                install: Some(self.settings.install_kind),
            },
        })
    }

    /// `hub.backup`: **Back up now**.
    pub(crate) async fn hub_backup(
        &self,
        _caller: &Caller,
        _request: HubBackup,
    ) -> Result<HubBackupResult> {
        let progress = self.start_backup(BackupTrigger::Manual)?;
        Ok(HubBackupResult { progress })
    }

    /// `hub.backup_settings`: reads the settings, or changes the fields that
    /// are set.
    pub(crate) async fn hub_backup_settings(
        &self,
        _caller: &Caller,
        request: HubBackupSettings,
    ) -> Result<HubBackupSettingsResult> {
        request.validate()?;
        if let Some(schedule) = &request.schedule {
            settings::time_zone(&schedule.time_zone)?;
        }
        let folder = match &request.folder {
            Some(folder) => Some(check_folder(folder.trim()).await?),
            None => None,
        };
        let no_change = request.schedule.is_none() && folder.is_none() && request.keep.is_none();
        if no_change {
            return Ok(HubBackupSettingsResult {
                settings: self.backups.file()?.settings,
            });
        }
        let now = Timestamp::now();
        let file = self.backups.update(|file| {
            if let Some(schedule) = request.schedule {
                if schedule != file.settings.schedule {
                    file.schedule_changed_at = Some(now);
                }
                file.settings.schedule = schedule;
            }
            if let Some(folder) = folder {
                file.settings.folder = folder;
            }
            if let Some(keep) = request.keep {
                file.settings.keep = keep;
            }
        })?;
        tracing::info!(
            frequency = ?file.settings.schedule.frequency,
            folder = %file.settings.folder,
            keep = file.settings.keep,
            "changed the backup settings"
        );
        Ok(HubBackupSettingsResult {
            settings: file.settings,
        })
    }

    /// Starts a backup in the background. Fails with `conflict` while one
    /// runs.
    pub(crate) fn start_backup(&self, trigger: BackupTrigger) -> Result<BackupProgress> {
        let file = self.backups.file()?;
        let data_dir = self.backups.data_dir.clone();
        let started_at = Timestamp::now();
        let done = Arc::new(AtomicU64::new(0));
        let progress = {
            let mut running = self
                .backups
                .running
                .lock()
                .map_err(|_| HubError::internal("the backup lock is poisoned"))?;
            if running.is_some() {
                return Err(HubError::conflict(
                    "A backup runs already. Wait until it finishes, then try again.",
                ));
            }
            let next = Running {
                trigger,
                started_at,
                done: done.clone(),
                total: archive::total_bytes(&data_dir),
            };
            let progress = next.progress();
            *running = Some(next);
            progress
        };
        let hub = self.clone();
        let folder = PathBuf::from(&file.settings.folder);
        let stem = archive_stem(started_at);
        let keep = file.settings.keep;
        tokio::spawn(async move {
            let job_folder = folder.clone();
            let written = tokio::task::spawn_blocking(move || {
                archive::write(&data_dir, &job_folder, &stem, &done)
            })
            .await
            .map_err(HubError::from)
            .and_then(|r| r);
            let finished_at = Timestamp::now();
            let record = match written {
                Ok(written) => {
                    let deleted = archive::prune(&folder, keep);
                    tracing::info!(
                        file = %written.path.display(),
                        size = written.size,
                        deleted = deleted.len(),
                        "a backup finished"
                    );
                    BackupRecord {
                        file_name: written.file_name,
                        path: written.path.display().to_string(),
                        size_bytes: written.size,
                        trigger,
                        started_at,
                        finished_at,
                        error: None,
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "a backup failed");
                    BackupRecord {
                        file_name: String::new(),
                        path: String::new(),
                        size_bytes: 0,
                        trigger,
                        started_at,
                        finished_at,
                        error: Some(backup_error_text(&err)),
                    }
                }
            };
            if let Err(err) = hub.backups.update(|file| file.last = Some(record)) {
                tracing::warn!(error = %err, "could not record the backup result");
            }
            if let Ok(mut running) = hub.backups.running.lock() {
                *running = None;
            }
        });
        Ok(progress)
    }

    /// Starts the scheduled backup when one is due at `now`. Returns true
    /// when it started one.
    pub(crate) fn backup_if_due(&self, now: Timestamp) -> Result<bool> {
        let file = self.backups.file()?;
        let Some(slot) = settings::due_slot(&file, now) else {
            return Ok(false);
        };
        if self.backups.progress().is_some() {
            return Ok(false);
        }
        self.start_backup(BackupTrigger::Scheduled)?;
        self.backups.update(|file| file.last_slot = Some(slot))?;
        Ok(true)
    }

    /// Whether a backup runs now.
    pub(crate) fn backup_running(&self) -> bool {
        self.backups.progress().is_some()
    }
}

/// The text of a failed backup for the client: an API error says it in
/// words already, and other errors get the cause.
fn backup_error_text(err: &HubError) -> String {
    match err {
        HubError::Api(body) => body.message.clone(),
        other => format!("{other}. Check the space on the disk, then try again."),
    }
}

/// Checks a new backup folder on the hub machine: an absolute path that the
/// hub can make and write to. Returns the path as the hub stores it.
async fn check_folder(folder: &str) -> Result<String> {
    let path = PathBuf::from(folder);
    if !path.is_absolute() {
        return Err(HubError::invalid(format!(
            "The backup folder must be a full path on the hub machine, such as /srv/backups/consort. \"{folder}\" is not."
        )));
    }
    let probe_path = path.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        archive::make_folder(&probe_path).map_err(|err| cannot_write(&probe_path, &err))?;
        let probe = probe_path.join(format!(
            ".consort-write-check-{}",
            &crate::util::random_token()[..8]
        ));
        crate::util::create_new_private_file(&probe)
            .map_err(|err| cannot_write(&probe_path, &err))?;
        let _ = std::fs::remove_file(&probe);
        Ok(())
    })
    .await??;
    Ok(path.display().to_string())
}

fn cannot_write(folder: &Path, err: &std::io::Error) -> HubError {
    HubError::invalid(format!(
        "The hub cannot write to {}: {err}. Choose another folder, or let the hub user write to it.",
        folder.display()
    ))
}

/// Looks every 30 seconds whether a scheduled backup is due.
pub(crate) fn spawn_scheduler(hub: &Hub) -> JoinHandle<()> {
    let hub = hub.clone();
    tokio::spawn(async move {
        loop {
            if let Err(err) = hub.backup_if_due(Timestamp::now()) {
                tracing::warn!(error = %err, "could not start the scheduled backup");
            }
            tokio::select! {
                () = hub.shutdown.cancelled() => return,
                () = tokio::time::sleep(SCHEDULE_TICK) => {}
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_is_named_after_its_start_in_utc() {
        // 2026-10-29T08:00:00Z is 3:00 AM in Chicago. The name is in UTC
        // whatever the zone of the schedule.
        let at = Timestamp::from_millis(1_793_260_800_000);
        assert_eq!(archive_stem(at), "consort-2026-10-29-0800");
    }
}
