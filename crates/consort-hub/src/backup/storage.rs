//! How much space Consort uses on the hub disk, by kind.

use std::collections::HashSet;
use std::path::Path;

use consort_proto::client_api::HubStorage;
use rusqlite::Connection;

use crate::error::Result;

/// The names of the database files: the database, its write-ahead log and
/// its shared memory file.
pub(crate) const DATABASE_FILES: [&str; 3] = ["hub.db", "hub.db-wal", "hub.db-shm"];

/// The SHA-256 names of every blob that a stored skill version uses.
pub(crate) fn skill_blob_names(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT files FROM skill_versions")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut names = HashSet::new();
    for row in rows {
        let files: Vec<consort_proto::model::SkillFileInfo> = serde_json::from_str(&row?)?;
        names.extend(files.into_iter().map(|f| f.sha256.as_str().to_owned()));
    }
    Ok(names)
}

/// Whether a file name in `blobs/` is a blob: 64 lowercase hex characters.
pub(crate) fn is_blob_name(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Measures the database files and the blobs. `skill_blobs` are the blobs
/// that count as skills.
pub(crate) fn measure(data_dir: &Path, skill_blobs: &HashSet<String>) -> HubStorage {
    let database_bytes = DATABASE_FILES
        .iter()
        .filter_map(|name| std::fs::metadata(data_dir.join(name)).ok())
        .map(|m| m.len())
        .sum();
    let (mut blobs_bytes, mut skills_bytes) = (0_u64, 0_u64);
    if let Ok(entries) = std::fs::read_dir(data_dir.join("blobs")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() || !is_blob_name(&name) {
                continue;
            }
            if skill_blobs.contains(&name) {
                skills_bytes += meta.len();
            } else {
                blobs_bytes += meta.len();
            }
        }
    }
    HubStorage {
        database_bytes,
        blobs_bytes,
        skills_bytes,
        free_bytes: free_space(data_dir),
    }
}

/// The space that the user can still use on the disk of `path`.
#[cfg(unix)]
pub(crate) fn free_space(path: &Path) -> Option<u64> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

/// The hub cannot read the free space on this system without `unsafe` code
/// in Consort, so it leaves the number out.
#[cfg(not(unix))]
pub(crate) fn free_space(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_and_skill_blobs_count_apart() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hub.db"), vec![0; 100]).unwrap();
        std::fs::write(dir.path().join("hub.db-wal"), vec![0; 20]).unwrap();
        let blobs = dir.path().join("blobs");
        std::fs::create_dir(&blobs).unwrap();
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        std::fs::write(blobs.join(&a), vec![0; 7]).unwrap();
        std::fs::write(blobs.join(&b), vec![0; 5]).unwrap();
        std::fs::write(blobs.join("upload.tmp"), vec![0; 1000]).unwrap();
        let skills: HashSet<String> = [b.clone()].into_iter().collect();
        let storage = measure(dir.path(), &skills);
        assert_eq!(storage.database_bytes, 120);
        assert_eq!(storage.blobs_bytes, 7);
        assert_eq!(storage.skills_bytes, 5);
        #[cfg(unix)]
        assert!(storage.free_bytes.is_some_and(|f| f > 0));
    }

    #[test]
    fn only_hex_names_are_blobs() {
        assert!(is_blob_name(&"0f".repeat(32)));
        assert!(!is_blob_name(&"0F".repeat(32)));
        assert!(!is_blob_name("abc"));
        assert!(!is_blob_name(&format!("{}.tmp", "a".repeat(60))));
    }
}
