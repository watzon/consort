//! Writes one backup archive: a tar file with a manifest, a consistent copy
//! of the database, the keys and the settings, and every blob.
//!
//! The copy of the database comes from SQLite's online backup API, so the
//! hub keeps running and writing while the backup runs. Blobs are never
//! changed after they are written, so a plain copy of each is consistent.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use consort_proto::time::Timestamp;
use serde::{Deserialize, Serialize};

use super::storage::{DATABASE_FILES, is_blob_name};
use crate::error::{HubError, Result};

/// The first entry of every archive.
pub(crate) const MANIFEST_NAME: &str = "consort-backup.json";
/// The `kind` in the manifest.
pub(crate) const MANIFEST_KIND: &str = "consort-hub-backup";
/// The archive format. A restore refuses a format that it does not know.
pub(crate) const FORMAT: u32 = 1;
/// The files of the data folder, next to `hub.db` and `blobs/`, that a
/// backup holds when they exist.
pub(crate) const EXTRA_FILES: [&str; 3] = ["hub-secret.key", "composio.key", "backup.json"];
/// The start of every archive name.
pub(crate) const NAME_PREFIX: &str = "consort-";

/// What the first entry of an archive says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub kind: String,
    pub format: u32,
    pub hub_version: String,
    /// The `user_version` of `hub.db`.
    pub schema_version: i64,
    pub created_at: Timestamp,
    /// The number of blobs.
    pub blobs: u64,
}

/// An archive that the hub wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Written {
    pub path: PathBuf,
    pub file_name: String,
    pub size: u64,
}

/// The bytes that a backup of `data_dir` will copy, for the progress.
pub(crate) fn total_bytes(data_dir: &Path) -> u64 {
    let size = |path: PathBuf| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut total: u64 = DATABASE_FILES.iter().map(|n| size(data_dir.join(n))).sum();
    total += EXTRA_FILES
        .iter()
        .map(|n| size(data_dir.join(n)))
        .sum::<u64>();
    total += blob_files(data_dir)
        .iter()
        .map(|(_, path)| size(path.clone()))
        .sum::<u64>();
    total
}

/// The blobs in name order, as `(name, path)`.
fn blob_files(data_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(data_dir.join("blobs"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                .filter(|(name, _)| is_blob_name(name))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// Counts the bytes that go into the archive.
struct Counting<'a, W> {
    inner: W,
    done: &'a AtomicU64,
}

impl<W: Write> Write for Counting<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.done.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Copies the database with SQLite's online backup API, in one step, so the
/// copy is one consistent state while the hub writes.
pub(crate) fn copy_database(db_path: &Path, dest: &Path) -> Result<i64> {
    use rusqlite::backup::{Backup, StepResult};
    let src = crate::store::open_connection(db_path)?;
    let schema_version: i64 = src.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    // The copy is the whole database, and the backup folder may be shared:
    // the file is private (mode 0600 on Unix) from the start. SQLite gives
    // its journal the same mode.
    drop(crate::util::create_new_private_file(dest)?);
    let mut dst = rusqlite::Connection::open(dest)?;
    {
        let backup = Backup::new(&src, &mut dst)?;
        let mut waits = 0;
        loop {
            match backup.step(-1)? {
                StepResult::Done => break,
                StepResult::More => {}
                StepResult::Busy | StepResult::Locked => {
                    waits += 1;
                    if waits > 600 {
                        return Err(HubError::internal(
                            "the database stayed locked for a minute, so the backup stopped",
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                // A new variant: treat it as one more step.
                _ => {}
            }
        }
    }
    drop(dst);
    Ok(schema_version)
}

/// A free archive name in `folder`: `<stem>.tar`, else `<stem>-2.tar` and
/// so on.
fn free_name(folder: &Path, stem: &str) -> String {
    let mut n = 1;
    loop {
        let name = if n == 1 {
            format!("{stem}.tar")
        } else {
            format!("{stem}-{n}.tar")
        };
        if !folder.join(&name).exists() {
            return name;
        }
        n += 1;
    }
}

/// Makes the backup folder when it is missing, with mode 0700 on Unix. A
/// folder that exists keeps its mode.
pub(crate) fn make_folder(folder: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(folder)
    }
}

fn append_bytes<W: Write>(builder: &mut tar::Builder<W>, name: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(u64::try_from(Timestamp::now().as_millis() / 1000).unwrap_or(0));
    header.set_cksum();
    builder.append_data(&mut header, name, bytes)?;
    Ok(())
}

/// Writes a backup of `data_dir` to `<folder>/<stem>.tar`. `done` counts the
/// bytes written. A failure removes the partial files.
pub(crate) fn write(
    data_dir: &Path,
    folder: &Path,
    stem: &str,
    done: &AtomicU64,
) -> Result<Written> {
    make_folder(folder).map_err(|err| folder_error(folder, &err))?;
    let token = &crate::util::random_token()[..12];
    let temp_db = folder.join(format!(".consort-backup-{token}.db"));
    let partial = folder.join(format!(".consort-backup-{token}.tar.partial"));
    let result = write_inner(data_dir, folder, stem, &temp_db, &partial, done);
    let _ = std::fs::remove_file(&temp_db);
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

fn write_inner(
    data_dir: &Path,
    folder: &Path,
    stem: &str,
    temp_db: &Path,
    partial: &Path,
    done: &AtomicU64,
) -> Result<Written> {
    let schema_version = copy_database(&data_dir.join("hub.db"), temp_db)?;
    let blobs = blob_files(data_dir);
    let manifest = Manifest {
        kind: MANIFEST_KIND.to_owned(),
        format: FORMAT,
        hub_version: crate::hub::HUB_VERSION.to_owned(),
        schema_version,
        created_at: Timestamp::now(),
        blobs: blobs.len() as u64,
    };
    let file =
        crate::util::create_new_private_file(partial).map_err(|err| folder_error(folder, &err))?;
    let mut builder = tar::Builder::new(Counting {
        inner: BufWriter::new(file),
        done,
    });
    append_bytes(
        &mut builder,
        MANIFEST_NAME,
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    builder.append_path_with_name(temp_db, "hub.db")?;
    for name in EXTRA_FILES {
        let path = data_dir.join(name);
        if path.is_file() {
            builder.append_path_with_name(&path, name)?;
        }
    }
    for (name, path) in &blobs {
        // A blob that the hub deleted since the list was made is skipped.
        match builder.append_path_with_name(path, format!("blobs/{name}")) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    let counting = builder.into_inner()?;
    let file = counting
        .inner
        .into_inner()
        .map_err(|e| HubError::Io(e.into_error()))?;
    file.sync_all()?;
    let size = file.metadata()?.len();
    drop(file);
    let file_name = free_name(folder, stem);
    let path = folder.join(&file_name);
    std::fs::rename(partial, &path)?;
    Ok(Written {
        path,
        file_name,
        size,
    })
}

fn folder_error(folder: &Path, err: &std::io::Error) -> HubError {
    HubError::invalid(format!(
        "The hub cannot write to the backup folder {}: {err}. Choose another folder, or let the hub user write to it.",
        folder.display()
    ))
}

/// Reads `consort-YYYY-MM-DD-HHMM[-N].tar` as its time stamp and number.
pub(crate) fn archive_order(name: &str) -> Option<(String, u32)> {
    let stem = name.strip_prefix(NAME_PREFIX)?.strip_suffix(".tar")?;
    let (stamp, n) = match stem.get(15..) {
        Some(rest) if !rest.is_empty() => (&stem[..15], rest.strip_prefix('-')?.parse().ok()?),
        _ => (stem, 1),
    };
    let ok = stamp.len() == 15
        && stamp.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 | 10 => b == b'-',
            _ => b.is_ascii_digit(),
        });
    ok.then(|| (stamp.to_owned(), n))
}

/// The time of an archive for pruning: the time in its manifest, or else
/// the time in its name read as UTC. Names in local time from older hubs
/// do not decide the order when the manifest can be read.
fn archive_time(path: &Path, stamp: &str) -> i64 {
    if let Ok(manifest) = super::restore::read_manifest(path) {
        return manifest.created_at.as_millis();
    }
    chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d-%H%M")
        .map(|t| t.and_utc().timestamp_millis())
        .unwrap_or(0)
}

/// Deletes the oldest archives in `folder` so that `keep` stay. Only files
/// with the archive name count. The order is the time in each manifest
/// (see [`archive_time`]), then the name. Returns the names it deleted.
pub(crate) fn prune(folder: &Path, keep: u32) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut archives: Vec<((i64, String, u32), String)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let (stamp, n) = archive_order(&name)?;
            let time = archive_time(&e.path(), &stamp);
            Some(((time, stamp, n), name))
        })
        .collect();
    archives.sort();
    let extra = archives.len().saturating_sub(keep as usize);
    let mut deleted = Vec::new();
    for (_, name) in archives.into_iter().take(extra) {
        match std::fs::remove_file(folder.join(&name)) {
            Ok(()) => deleted.push(name),
            Err(err) => {
                tracing::warn!(file = %name, error = %err, "could not delete an old backup")
            }
        }
    }
    deleted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_sort_by_time_then_number() {
        assert_eq!(
            archive_order("consort-2026-10-29-0300.tar"),
            Some(("2026-10-29-0300".into(), 1))
        );
        assert_eq!(
            archive_order("consort-2026-10-29-0300-2.tar"),
            Some(("2026-10-29-0300".into(), 2))
        );
        assert_eq!(archive_order("consort-notes.tar"), None);
        assert_eq!(archive_order("consort-2026-10-29-0300.tar.partial"), None);
        assert_eq!(archive_order("other-2026-10-29-0300.tar"), None);
    }

    /// An archive file with only a manifest that says `created_at`.
    fn archive_made_at(path: &Path, created_at: Timestamp) {
        let manifest = Manifest {
            kind: MANIFEST_KIND.to_owned(),
            format: FORMAT,
            hub_version: "0.1.0".to_owned(),
            schema_version: 9,
            created_at,
            blobs: 0,
        };
        let mut builder = tar::Builder::new(std::fs::File::create(path).unwrap());
        append_bytes(
            &mut builder,
            MANIFEST_NAME,
            &serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        builder.finish().unwrap();
    }

    #[test]
    fn prune_keeps_the_newest_archive_by_its_manifest_not_its_name() {
        // The old hub named the first archive in UTC (20:00), and the next
        // one five minutes later in Chicago time (15:05). The newer one has
        // the older name.
        let dir = tempfile::tempdir().unwrap();
        let older = dir.path().join("consort-2026-10-29-2000.tar");
        let newer = dir.path().join("consort-2026-10-29-1505.tar");
        archive_made_at(
            &older,
            Timestamp::parse_rfc3339("2026-10-29T20:00:00Z").unwrap(),
        );
        archive_made_at(
            &newer,
            Timestamp::parse_rfc3339("2026-10-29T20:05:00Z").unwrap(),
        );
        let deleted = prune(dir.path(), 1);
        assert_eq!(deleted, vec!["consort-2026-10-29-2000.tar".to_owned()]);
        assert!(newer.exists());
    }

    #[test]
    fn the_database_copy_is_private_from_the_start() {
        let data = tempfile::tempdir().unwrap();
        let conn = crate::store::open_and_migrate(data.path()).unwrap();
        drop(conn);
        let folder = tempfile::tempdir().unwrap();
        let dest = folder.path().join(".consort-backup-test.db");
        copy_database(&data.path().join("hub.db"), &dest).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // A file that is there already is never overwritten.
        assert!(copy_database(&data.path().join("hub.db"), &dest).is_err());
    }

    #[test]
    fn prune_keeps_the_newest_archives_and_other_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "consort-2026-10-27-0300.tar",
            "consort-2026-10-28-0300.tar",
            "consort-2026-10-28-0300-2.tar",
            "consort-2026-10-29-0300.tar",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let deleted = prune(dir.path(), 2);
        assert_eq!(
            deleted,
            vec![
                "consort-2026-10-27-0300.tar".to_owned(),
                "consort-2026-10-28-0300.tar".to_owned()
            ]
        );
        assert!(dir.path().join("consort-2026-10-28-0300-2.tar").exists());
        assert!(dir.path().join("consort-2026-10-29-0300.tar").exists());
        assert!(dir.path().join("notes.txt").exists());
    }
}
