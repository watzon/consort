//! `consort hub restore`: puts a backup archive back into a data folder
//! while no hub uses that folder.

use std::io::Read;
use std::path::{Path, PathBuf};

use consort_proto::time::Timestamp;
use sha2::{Digest, Sha256};

use super::archive::{EXTRA_FILES, FORMAT, MANIFEST_KIND, MANIFEST_NAME, Manifest};
use super::storage::{DATABASE_FILES, is_blob_name};

/// Why a restore did not run. Each message says what to do next.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    /// A hub uses the data folder now.
    #[error(
        "A hub runs with the data folder {}. Stop the hub, then run the restore again.",
        .0.display()
    )]
    HubRunning(PathBuf),
    /// The file is not a Consort backup.
    #[error(
        "{} is not a Consort backup: {reason}. Choose a consort-<date>.tar file from the backup folder.",
        path.display()
    )]
    NotABackup {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// A newer hub made the backup.
    #[error(
        "A newer hub ({hub_version}) made this backup. Update Consort on this machine, then run the restore again."
    )]
    TooNew {
        /// The version of the hub that made it.
        hub_version: String,
    },
    /// The data folder holds a hub already.
    #[error(
        "The data folder {} is not empty. Add --force to move its database, blobs and keys to a before-restore folder and restore the backup.",
        .0.display()
    )]
    NotEmpty(PathBuf),
    /// A file could not be read or written.
    #[error(
        "The restore failed: {0}. Check the space on the disk and that you can write to the data folder, then try again."
    )]
    Io(String),
}

impl From<std::io::Error> for RestoreError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// What a restore did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    /// When the backup was made.
    pub created_at: Timestamp,
    /// The version of the hub that made it.
    pub hub_version: String,
    /// The number of blobs that it restored.
    pub blobs: u64,
    /// Where the files that were in the data folder went, with `--force`.
    pub moved_aside: Option<PathBuf>,
}

/// The name of the lock file in the data folder. A running hub holds a lock
/// on it.
pub const LOCK_FILE: &str = "hub.lock";

/// Takes the lock of a data folder. The hub holds it while it runs, and a
/// restore holds it while it works. `None` when another process holds it.
pub(crate) fn try_lock(data_dir: &Path) -> std::io::Result<Option<std::fs::File>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(err)) => Err(err),
    }
}

fn not_a_backup(path: &Path, reason: impl Into<String>) -> RestoreError {
    RestoreError::NotABackup {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

/// Reads the manifest, the first entry of the archive.
pub(crate) fn read_manifest(archive: &Path) -> Result<Manifest, RestoreError> {
    let file = std::fs::File::open(archive)
        .map_err(|err| not_a_backup(archive, format!("it cannot be opened ({err})")))?;
    let mut tar = tar::Archive::new(file);
    let mut entries = tar
        .entries()
        .map_err(|_| not_a_backup(archive, "it is not a tar file"))?;
    let mut first = match entries.next() {
        Some(Ok(entry)) => entry,
        _ => return Err(not_a_backup(archive, "it is not a tar file")),
    };
    let name = first
        .path()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name != MANIFEST_NAME {
        return Err(not_a_backup(archive, "it has no Consort manifest"));
    }
    let mut text = String::new();
    first
        .read_to_string(&mut text)
        .map_err(|_| not_a_backup(archive, "its manifest cannot be read"))?;
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|_| not_a_backup(archive, "its manifest cannot be read"))?;
    if manifest.kind != MANIFEST_KIND {
        return Err(not_a_backup(archive, "its manifest is not a hub backup"));
    }
    if manifest.format > FORMAT {
        return Err(RestoreError::TooNew {
            hub_version: manifest.hub_version,
        });
    }
    Ok(manifest)
}

/// Whether the data folder holds anything but the lock file.
fn has_content(data_dir: &Path) -> std::io::Result<bool> {
    Ok(std::fs::read_dir(data_dir)?
        .flatten()
        .any(|e| e.file_name() != LOCK_FILE))
}

/// Unpacks the archive into `staging` and checks every blob hash. Returns
/// the number of blobs.
fn unpack(archive: &Path, staging: &Path) -> Result<u64, RestoreError> {
    let blobs_dir = staging.join("blobs");
    crate::store::create_private_dir(&blobs_dir).map_err(|e| RestoreError::Io(e.to_string()))?;
    let mut tar = tar::Archive::new(std::fs::File::open(archive)?);
    let mut blobs = 0_u64;
    let mut has_db = false;
    for entry in tar
        .entries()
        .map_err(|_| not_a_backup(archive, "it is not a tar file"))?
    {
        let mut entry = entry.map_err(|_| not_a_backup(archive, "it is damaged"))?;
        let name = entry
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|_| not_a_backup(archive, "it has an entry with a bad name"))?;
        if name == MANIFEST_NAME {
            continue;
        }
        let (dest, blob) = if let Some(blob) = name.strip_prefix("blobs/") {
            if !is_blob_name(blob) {
                return Err(not_a_backup(
                    archive,
                    format!("it has an unknown entry {name}"),
                ));
            }
            (blobs_dir.join(blob), Some(blob.to_owned()))
        } else if name == "hub.db" || EXTRA_FILES.contains(&name.as_str()) {
            has_db |= name == "hub.db";
            (staging.join(&name), None)
        } else {
            return Err(not_a_backup(
                archive,
                format!("it has an unknown entry {name}"),
            ));
        };
        if !entry.header().entry_type().is_file() {
            return Err(not_a_backup(archive, format!("{name} is not a file")));
        }
        let mut out = crate::util::create_new_private_file(&dest)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0_u8; 64 * 1024];
        loop {
            let n = entry
                .read(&mut buf)
                .map_err(|_| not_a_backup(archive, "it is damaged"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            std::io::Write::write_all(&mut out, &buf[..n])?;
        }
        out.sync_all()?;
        if let Some(blob) = blob {
            let hash = crate::util::hex_encode(&hasher.finalize());
            if hash != blob {
                return Err(not_a_backup(
                    archive,
                    format!("the blob {blob} does not match its hash"),
                ));
            }
            blobs += 1;
        }
    }
    if !has_db {
        return Err(not_a_backup(archive, "it has no database"));
    }
    Ok(blobs)
}

/// Checks the restored database: SQLite's integrity check, and a schema
/// that this hub knows. Then gives it a new hub epoch.
fn check_database(archive: &Path, db: &Path, hub_version: &str) -> Result<(), RestoreError> {
    let conn = rusqlite::Connection::open(db)
        .map_err(|_| not_a_backup(archive, "its database cannot be opened"))?;
    let check: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| not_a_backup(archive, "its database is damaged"))?;
    if check != "ok" {
        return Err(not_a_backup(archive, "its database is damaged"));
    }
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| not_a_backup(archive, "its database cannot be read"))?;
    let known = i64::try_from(crate::store::migrations::MIGRATIONS.len()).unwrap_or(i64::MAX);
    if version > known {
        return Err(RestoreError::TooNew {
            hub_version: hub_version.to_owned(),
        });
    }
    // A restored hub has an older feed. A new epoch makes every client
    // drop its saved feed position and load a snapshot.
    crate::store::renew_hub_epoch(&conn)
        .map_err(|_| not_a_backup(archive, "its database cannot be written"))?;
    Ok(())
}

/// The data folder entries that a restore replaces.
fn replaced_entries() -> impl Iterator<Item = &'static str> {
    DATABASE_FILES
        .into_iter()
        .chain(EXTRA_FILES)
        .chain(std::iter::once("blobs"))
}

/// Restores `archive` into `data_dir`. The hub must not run with that
/// folder. A folder with content needs `force`: its database, blobs, keys
/// and backup settings move to `before-restore-<time>/` in the folder first.
/// Other entries, such as the `backups` folder, stay.
pub fn restore(
    archive: &Path,
    data_dir: &Path,
    force: bool,
) -> Result<RestoreReport, RestoreError> {
    if !archive.is_file() {
        return Err(not_a_backup(archive, "it is not a file"));
    }
    let manifest = read_manifest(archive)?;
    crate::store::create_private_dir(data_dir).map_err(|e| RestoreError::Io(e.to_string()))?;
    let Some(_lock) = try_lock(data_dir)? else {
        return Err(RestoreError::HubRunning(data_dir.to_path_buf()));
    };
    let occupied = has_content(data_dir)?;
    if occupied && !force {
        return Err(RestoreError::NotEmpty(data_dir.to_path_buf()));
    }
    let staging = data_dir.join(format!(".restore-{}", &crate::util::random_token()[..12]));
    let unpacked = unpack(archive, &staging).and_then(|blobs| {
        check_database(archive, &staging.join("hub.db"), &manifest.hub_version).map(|()| blobs)
    });
    let blobs = match unpacked {
        Ok(blobs) => blobs,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
    };
    let mut moved_aside = None;
    if occupied {
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let aside = data_dir.join(format!("before-restore-{stamp}"));
        crate::store::create_private_dir(&aside).map_err(|e| RestoreError::Io(e.to_string()))?;
        for name in replaced_entries() {
            let from = data_dir.join(name);
            if from.exists() {
                std::fs::rename(&from, aside.join(name))?;
            }
        }
        moved_aside = Some(aside);
    }
    for name in replaced_entries() {
        let from = staging.join(name);
        if from.exists() {
            std::fs::rename(&from, data_dir.join(name))?;
        }
    }
    let _ = std::fs::remove_dir_all(&staging);
    Ok(RestoreReport {
        created_at: manifest.created_at,
        hub_version: manifest.hub_version,
        blobs,
        moved_aside,
    })
}
