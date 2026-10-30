//! Reads an uploaded skill zip in memory.
//!
//! Nothing is extracted to the disk. The hub reads the central directory
//! first and checks every entry with [`check_listing`]: paths, links,
//! devices, the file count, and the declared sizes against the limits.
//! Only then does it decompress, and it reads at most one byte more than
//! the declared size of each entry, so a zip bomb or an entry that lies
//! about its size is refused without filling the memory.

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read};

use consort_proto::model::{SKILL_MANIFEST_FILE, SKILL_UPLOAD_MAX_BYTES};

use super::NewSkillFile;
use super::package::{Entry, EntryKind, LISTING_MAX_ENTRIES, check_listing, skill_invalid};
use crate::error::Result;

/// The file type bits of a Unix mode.
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;
/// The longest link target that the hub reads.
const LINK_TARGET_MAX_BYTES: u64 = 4_096;

/// Files that archive tools add and that are not part of a skill.
fn is_junk(path: &str) -> bool {
    path.starts_with("__MACOSX/") || path == ".DS_Store" || path.ends_with("/.DS_Store")
}

/// Reads `limit + 1` bytes at most.
fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| skill_invalid("A file in the zip is damaged."))?;
    Ok(bytes)
}

/// The files of a skill zip, after every check. When the zip holds one
/// folder with the skill in it, that folder is the skill folder.
pub(crate) fn unpack_zip(bytes: &[u8]) -> Result<Vec<NewSkillFile>> {
    unpack_zip_folder(bytes, None)
}

/// The files of one skill folder in a zip of a whole plugin. The plugin root
/// is at the top of the zip, or in the one folder that holds everything.
/// With no `folder`, the zip is one skill, as for an upload. Only the files
/// of the folder are read, and every check of an upload applies to them.
pub(crate) fn unpack_zip_folder(bytes: &[u8], folder: Option<&str>) -> Result<Vec<NewSkillFile>> {
    if bytes.len() as u64 > SKILL_UPLOAD_MAX_BYTES {
        return Err(skill_invalid(format!(
            "The zip is larger than {SKILL_UPLOAD_MAX_BYTES} bytes."
        )));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| skill_invalid("The upload is not a zip file that the hub can read."))?;
    if archive.len() > LISTING_MAX_ENTRIES {
        return Err(skill_invalid("The zip has too many entries for one skill."));
    }
    // The listing, from the central directory. Only link targets are read
    // here, and they are small.
    let mut listed: Vec<(String, EntryKind, usize)> = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|_| skill_invalid("An entry of the zip is damaged or encrypted."))?;
        let name = std::str::from_utf8(file.name_raw())
            .map_err(|_| skill_invalid("A path in the zip is not UTF-8 text."))?
            .to_owned();
        if file.encrypted() {
            return Err(skill_invalid(format!(
                "{name} in the zip is encrypted. Upload a zip with no password."
            )));
        }
        if is_junk(&name) {
            continue;
        }
        let mode = file.unix_mode().unwrap_or(S_IFREG | 0o644);
        let kind = match mode & S_IFMT {
            S_IFDIR => continue,
            _ if file.is_dir() => continue,
            0 | S_IFREG => EntryKind::File {
                size: file.size(),
                executable: mode & 0o111 != 0,
            },
            S_IFLNK => {
                if file.size() > LINK_TARGET_MAX_BYTES {
                    return Err(skill_invalid(format!(
                        "{name} is a link with a target that is too long."
                    )));
                }
                let target = read_limited(&mut file, LINK_TARGET_MAX_BYTES)?;
                EntryKind::Link {
                    target: String::from_utf8_lossy(&target).into_owned(),
                }
            }
            _ => EntryKind::Special,
        };
        listed.push((name, kind, index));
    }
    // A folder of a plugin: keep only its entries.
    let folder_prefix = match folder {
        Some(folder) => {
            let top = listed
                .first()
                .and_then(|(n, _, _)| n.split_once('/').map(|(top, _)| format!("{top}/")))
                .filter(|top| listed.iter().all(|(n, _, _)| n.starts_with(top.as_str())));
            let wanted = format!("{}/", folder.trim_matches('/'));
            let root = [Some(String::new()), top]
                .into_iter()
                .flatten()
                .find(|root| {
                    listed
                        .iter()
                        .any(|(n, _, _)| n.starts_with(&format!("{root}{wanted}")))
                })
                .ok_or_else(|| {
                    skill_invalid(format!(
                        "The archive has no folder {}.",
                        wanted.trim_end_matches('/')
                    ))
                })?;
            Some(format!("{root}{wanted}"))
        }
        None => None,
    };
    if let Some(prefix) = &folder_prefix {
        listed.retain(|(n, _, _)| n.starts_with(prefix.as_str()));
    }
    // One folder around the skill: use it as the root.
    let has_root_manifest = folder_prefix.is_none()
        && listed
            .iter()
            .any(|(n, _, _)| n.trim_start_matches("./") == SKILL_MANIFEST_FILE);
    let prefix = if let Some(prefix) = folder_prefix {
        Some(prefix)
    } else if has_root_manifest {
        None
    } else {
        listed
            .first()
            .and_then(|(n, _, _)| n.split_once('/').map(|(top, _)| format!("{top}/")))
            .filter(|top| {
                top != "../"
                    && listed.iter().all(|(n, _, _)| n.starts_with(top.as_str()))
                    && listed
                        .iter()
                        .any(|(n, _, _)| *n == format!("{top}{SKILL_MANIFEST_FILE}"))
            })
    };
    let mut entries = Vec::with_capacity(listed.len());
    let mut index_of: HashMap<String, usize> = HashMap::new();
    for (name, kind, index) in listed {
        let path = match &prefix {
            Some(p) => name[p.len()..].to_owned(),
            None => name,
        };
        let key = path
            .trim_start_matches("./")
            .trim_end_matches('/')
            .to_owned();
        index_of.insert(key, index);
        entries.push(Entry { path, kind });
    }
    let plan = check_listing(&entries)?;
    let mut contents = BTreeMap::new();
    for path in plan.content_paths() {
        let Some(index) = index_of.get(&path).copied() else {
            continue;
        };
        let size = plan.files.get(&path).map_or(0, |(_, size, _)| *size);
        let file = archive
            .by_index(index)
            .map_err(|_| skill_invalid(format!("{path} in the zip is damaged.")))?;
        contents.insert(path, read_limited(file, size)?);
    }
    plan.assemble(&contents)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use consort_proto::error::ErrorCode;
    use zip::write::SimpleFileOptions;

    use super::*;

    pub(crate) const MANIFEST: &str =
        "---\nname: zipped\ndescription: Comes from a zip.\n---\nBody\n";

    /// A zip of `(path, bytes, unix mode)` entries. A mode with the link
    /// bits makes a link whose content is its target.
    pub(crate) fn make_zip(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (path, bytes, mode) in entries {
            if mode & S_IFMT == S_IFLNK {
                writer
                    .add_symlink(
                        *path,
                        std::str::from_utf8(bytes).unwrap(),
                        SimpleFileOptions::default(),
                    )
                    .unwrap();
                continue;
            }
            // Windows writes the DOS system by default, and a reader then
            // ignores the Unix mode. Real zips with a mode come from Unix.
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .system(zip::System::Unix)
                .external_attributes(mode << 16);
            writer.start_file(*path, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn code(bytes: &[u8]) -> ErrorCode {
        unpack_zip(bytes).unwrap_err().code()
    }

    #[test]
    fn a_plain_zip_and_a_zip_with_one_folder_unpack() {
        let zip = make_zip(&[
            ("SKILL.md", MANIFEST.as_bytes(), S_IFREG | 0o644),
            ("scripts/run.sh", b"#!/bin/sh\n", S_IFREG | 0o755),
        ]);
        let files = unpack_zip(&zip).unwrap();
        assert_eq!(files.len(), 2);
        let script = files.iter().find(|f| f.path == "scripts/run.sh").unwrap();
        assert!(script.executable);
        let wrapped = make_zip(&[
            ("zipped/SKILL.md", MANIFEST.as_bytes(), S_IFREG | 0o644),
            ("zipped/ref/a.md", b"a", S_IFREG | 0o644),
            ("__MACOSX/zipped/._SKILL.md", b"junk", S_IFREG | 0o644),
        ]);
        let files = unpack_zip(&wrapped).unwrap();
        let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(paths, vec!["SKILL.md", "ref/a.md"]);
    }

    #[test]
    fn traversal_and_absolute_paths_are_refused() {
        for bad in ["../evil.sh", "a/../../evil", "/etc/cron.d/evil"] {
            let zip = make_zip(&[
                ("SKILL.md", MANIFEST.as_bytes(), S_IFREG | 0o644),
                (bad, b"x", S_IFREG | 0o644),
            ]);
            assert_eq!(code(&zip), ErrorCode::SkillInvalid, "{bad}");
        }
    }

    #[test]
    fn a_link_out_is_refused_and_a_link_in_is_a_copy() {
        let out = make_zip(&[
            ("SKILL.md", MANIFEST.as_bytes(), S_IFREG | 0o644),
            ("secrets", b"../../../../etc/passwd", S_IFLNK | 0o777),
        ]);
        assert_eq!(code(&out), ErrorCode::SkillInvalid);
        let absolute = make_zip(&[
            ("SKILL.md", MANIFEST.as_bytes(), S_IFREG | 0o644),
            ("key", b"/home/user/.ssh/id_ed25519", S_IFLNK | 0o777),
        ]);
        assert_eq!(code(&absolute), ErrorCode::SkillInvalid);
        let inside = make_zip(&[
            ("SKILL.md", MANIFEST.as_bytes(), S_IFREG | 0o644),
            ("docs/a.md", b"hello", S_IFREG | 0o644),
            ("a.md", b"docs/a.md", S_IFLNK | 0o777),
        ]);
        let files = unpack_zip(&inside).unwrap();
        let copy = files.iter().find(|f| f.path == "a.md").unwrap();
        assert_eq!(copy.bytes, b"hello");
    }

    #[test]
    fn a_device_is_refused() {
        let zip = make_zip(&[
            ("SKILL.md", MANIFEST.as_bytes(), S_IFREG | 0o644),
            ("dev", b"", 0o020_644),
        ]);
        assert_eq!(code(&zip), ErrorCode::SkillInvalid);
    }

    #[test]
    fn a_zip_bomb_is_refused_before_it_is_extracted() {
        // 6 MiB of zeros compresses to a few KiB, and one file may hold
        // only 5 MiB.
        let zeros = vec![0u8; 6 * 1024 * 1024];
        let zip = make_zip(&[
            ("SKILL.md", MANIFEST.as_bytes(), S_IFREG | 0o644),
            ("bomb.bin", &zeros, S_IFREG | 0o644),
        ]);
        assert!(zip.len() < 64 * 1024, "the bomb is small: {}", zip.len());
        assert_eq!(code(&zip), ErrorCode::SkillInvalid);
        // Five files of 4.5 MiB each break the total limit of 20 MiB.
        let part = vec![0u8; 4 * 1024 * 1024 + 512 * 1024];
        let mut entries: Vec<(String, &[u8], u32)> =
            vec![("SKILL.md".to_owned(), MANIFEST.as_bytes(), S_IFREG | 0o644)];
        for i in 0..5 {
            entries.push((format!("part{i}.bin"), &part, S_IFREG | 0o644));
        }
        let refs: Vec<(&str, &[u8], u32)> = entries
            .iter()
            .map(|(p, b, m)| (p.as_str(), *b, *m))
            .collect();
        assert_eq!(code(&make_zip(&refs)), ErrorCode::SkillInvalid);
    }

    #[test]
    fn a_zip_with_too_many_files_is_refused() {
        let names: Vec<String> = (0..=consort_proto::model::SKILL_MAX_FILES)
            .map(|i| format!("f/{i}.md"))
            .collect();
        let mut entries: Vec<(&str, &[u8], u32)> =
            vec![("SKILL.md", MANIFEST.as_bytes(), S_IFREG | 0o644)];
        for name in &names {
            entries.push((name.as_str(), b"x", S_IFREG | 0o644));
        }
        assert_eq!(code(&make_zip(&entries)), ErrorCode::SkillInvalid);
    }

    #[test]
    fn not_a_zip_is_refused() {
        assert_eq!(code(b"PK not really"), ErrorCode::SkillInvalid);
    }
}
