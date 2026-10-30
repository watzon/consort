//! Reads one skill folder from an npm package tarball (`.tgz`) in memory.
//!
//! Nothing goes to the disk and nothing runs: the hub never runs npm, and
//! install scripts of the package are never read. The tarball is unpacked as
//! a stream with a cap on the unpacked size, so a small file that grows to a
//! huge stream (a tar bomb) is refused. Only the entries of the wanted
//! folder are kept, and they pass the same checks as every other source
//! (`package::check_listing`).

use std::collections::BTreeMap;
use std::io::Read;

use consort_proto::model::{SKILL_MAX_FILE_BYTES, SKILL_MAX_FILES, SKILL_MAX_TOTAL_BYTES};
use flate2::read::GzDecoder;
use tar::EntryType;

use super::NewSkillFile;
use super::package::{Entry, EntryKind, LISTING_MAX_ENTRIES, check_listing, skill_invalid};
use crate::error::Result;

/// The most bytes that the unpacked stream of a tarball may have. A package
/// holds many files besides the skill, so this is above the skill limit.
pub(crate) const UNPACKED_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// The longest link target that the hub reads.
const LINK_TARGET_MAX_BYTES: u64 = 4_096;

/// A reader that stops at a cap and remembers that it did.
struct Capped<R> {
    inner: R,
    left: u64,
    hit: std::rc::Rc<std::cell::Cell<bool>>,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            self.hit.set(true);
            return Ok(0);
        }
        let max = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let n = self.inner.read(&mut buf[..max])?;
        self.left -= n as u64;
        Ok(n)
    }
}

fn too_big() -> crate::error::HubError {
    skill_invalid("The npm package unpacks to more than the hub accepts.")
}

fn damaged() -> crate::error::HubError {
    skill_invalid("The npm package is not a tarball that the hub can read.")
}

/// The files of `folder` in the tarball, after every check. The package
/// root is the one top folder of the tarball (`package/` for npm). With no
/// `folder`, the package root is the skill.
pub(crate) fn untar_folder(bytes: &[u8], folder: Option<&str>) -> Result<Vec<NewSkillFile>> {
    let hit = std::rc::Rc::new(std::cell::Cell::new(false));
    let stream = Capped {
        inner: GzDecoder::new(bytes),
        left: UNPACKED_MAX_BYTES,
        hit: hit.clone(),
    };
    let mut archive = tar::Archive::new(stream);
    let broken = |hit: &std::cell::Cell<bool>| if hit.get() { too_big() } else { damaged() };
    let mut root: Option<String> = None;
    let wanted = folder.map(|f| format!("{}/", f.trim_matches('/')));
    let mut entries: Vec<Entry> = Vec::new();
    let mut contents: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut seen = 0_usize;
    let mut total = 0_u64;
    let mut found_folder = wanted.is_none();
    let list = archive.entries().map_err(|_| broken(&hit))?;
    for entry in list {
        let mut entry = entry.map_err(|_| broken(&hit))?;
        seen += 1;
        if seen > LISTING_MAX_ENTRIES * 20 {
            return Err(skill_invalid("The npm package has too many entries."));
        }
        let path = std::str::from_utf8(&entry.path_bytes())
            .map_err(|_| skill_invalid("A path in the npm package is not UTF-8 text."))?
            .to_owned();
        let path = path.trim_start_matches("./");
        // The one top folder of the package.
        let Some((top, inside)) = path.split_once('/') else {
            // A file at the top, such as pax headers: not a package file.
            continue;
        };
        match &root {
            None => root = Some(top.to_owned()),
            Some(root) if root == top => {}
            Some(_) => {
                return Err(skill_invalid(
                    "The npm package has more than one top folder, so the hub cannot find its files.",
                ));
            }
        }
        let relative = match &wanted {
            Some(prefix) => match inside.strip_prefix(prefix.as_str()) {
                Some(rest) => rest,
                None => continue,
            },
            None => inside,
        };
        if relative.is_empty() {
            continue;
        }
        found_folder = true;
        let kind = match entry.header().entry_type() {
            EntryType::Directory => continue,
            EntryType::Regular | EntryType::Continuous => {
                let size = entry.header().size().map_err(|_| damaged())?;
                if size > SKILL_MAX_FILE_BYTES {
                    return Err(skill_invalid(format!(
                        "{relative} is larger than the limit for one file of a skill."
                    )));
                }
                total = total.saturating_add(size);
                if total > SKILL_MAX_TOTAL_BYTES || entries.len() > SKILL_MAX_FILES * 4 {
                    return Err(skill_invalid(
                        "The skill folder is larger than the skill limits.",
                    ));
                }
                let mode = entry.header().mode().unwrap_or(0o644);
                let mut content = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
                (&mut entry)
                    .take(size.saturating_add(1))
                    .read_to_end(&mut content)
                    .map_err(|_| damaged())?;
                contents.insert(relative.to_owned(), content);
                EntryKind::File {
                    size,
                    executable: mode & 0o111 != 0,
                }
            }
            EntryType::Symlink => {
                let target = entry
                    .link_name_bytes()
                    .map(|t| String::from_utf8_lossy(&t).into_owned())
                    .unwrap_or_default();
                if target.len() as u64 > LINK_TARGET_MAX_BYTES {
                    return Err(skill_invalid(format!(
                        "{relative} is a link with a target that is too long."
                    )));
                }
                EntryKind::Link { target }
            }
            // Hard links, devices, pipes and the like.
            _ => EntryKind::Special,
        };
        entries.push(Entry {
            path: relative.to_owned(),
            kind,
        });
    }
    if hit.get() {
        return Err(too_big());
    }
    if !found_folder {
        return Err(skill_invalid(format!(
            "The npm package has no folder {}.",
            folder.unwrap_or(".")
        )));
    }
    let plan = check_listing(&entries)?;
    plan.assemble(&contents)
}

#[cfg(test)]
pub(crate) mod tests {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;
    use crate::skills::upload::tests::MANIFEST;

    /// A package tarball. Each entry is `(path, content, mode)`. A path
    /// that ends in `/` is a folder. A content of `link:<target>` is a
    /// symbolic link.
    pub(crate) fn make_tgz(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, content, mode) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(*mode);
            header.set_mtime(0);
            if path.ends_with('/') {
                header.set_entry_type(EntryType::Directory);
                header.set_size(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, path, std::io::empty())
                    .unwrap();
            } else if let Some(target) = content.strip_prefix(b"link:") {
                header.set_entry_type(EntryType::Symlink);
                header.set_size(0);
                header
                    .set_link_name(std::str::from_utf8(target).unwrap())
                    .unwrap();
                header.set_cksum();
                builder
                    .append_data(&mut header, path, std::io::empty())
                    .unwrap();
            } else {
                header.set_entry_type(EntryType::Regular);
                header.set_size(content.len() as u64);
                header.set_cksum();
                builder.append_data(&mut header, path, *content).unwrap();
            }
        }
        let tar = builder.into_inner().unwrap();
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn one_folder_of_a_package_unpacks() {
        let tgz = make_tgz(&[
            ("package/package.json", b"{}", 0o644),
            ("package/skills/", b"", 0o755),
            ("package/skills/pdf/SKILL.md", MANIFEST.as_bytes(), 0o644),
            ("package/skills/pdf/scripts/run.sh", b"#!/bin/sh\n", 0o755),
            ("package/skills/other/SKILL.md", b"other", 0o644),
        ]);
        let files = untar_folder(&tgz, Some("skills/pdf")).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["SKILL.md", "scripts/run.sh"]);
        assert!(files[1].executable && !files[0].executable);
        // The package root is the skill when no folder is named.
        let root = make_tgz(&[("package/SKILL.md", MANIFEST.as_bytes(), 0o644)]);
        assert_eq!(untar_folder(&root, None).unwrap().len(), 1);
    }

    #[test]
    fn unsafe_packages_are_refused() {
        use consort_proto::error::ErrorCode;
        let code =
            |tgz: Vec<u8>, folder: Option<&str>| untar_folder(&tgz, folder).unwrap_err().code();
        // A link out of the folder.
        let evil = make_tgz(&[
            ("package/s/SKILL.md", MANIFEST.as_bytes(), 0o644),
            ("package/s/key", b"link:../../../../etc/passwd", 0o777),
        ]);
        assert_eq!(code(evil, Some("s")), ErrorCode::SkillInvalid);
        // A path that climbs out. The builder refuses to write one, so the
        // header is written by hand.
        let climb = {
            let mut header = tar::Header::new_gnu();
            let name = b"package/s/../../x";
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_size(1);
            header.set_mode(0o644);
            header.set_cksum();
            let mut builder = tar::Builder::new(Vec::new());
            builder.append(&header, &b"x"[..]).unwrap();
            let tar = builder.into_inner().unwrap();
            let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
            std::io::Write::write_all(&mut gz, &tar).unwrap();
            gz.finish().unwrap()
        };
        assert!(untar_folder(&climb, Some("s")).is_err());
        // A folder that is not there.
        let none = make_tgz(&[("package/a/SKILL.md", MANIFEST.as_bytes(), 0o644)]);
        assert_eq!(code(none, Some("missing")), ErrorCode::SkillInvalid);
        // Not a tarball.
        assert_eq!(code(b"hello".to_vec(), None), ErrorCode::SkillInvalid);
        // Two top folders.
        let two = make_tgz(&[("a/x", b"1", 0o644), ("b/y", b"2", 0o644)]);
        assert_eq!(code(two, None), ErrorCode::SkillInvalid);
        // A big file.
        let big = make_tgz(&[(
            "package/s/big.bin",
            &vec![0_u8; usize::try_from(SKILL_MAX_FILE_BYTES).unwrap() + 1],
            0o644,
        )]);
        assert_eq!(code(big, Some("s")), ErrorCode::SkillInvalid);
    }
}
