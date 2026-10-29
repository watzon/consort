//! Safe archive extraction.
//!
//! Every entry must stay inside the target folder:
//!
//! - An entry path with `..`, a root or a drive prefix is refused.
//! - The parent folder of each written entry is resolved on disk, and must
//!   be inside the target folder. This stops a write through a symbolic link.
//! - A symbolic link must be relative, and its target must stay inside the
//!   target folder when it is resolved from the link's own folder.
//!
//! Archives come from checksummed sources, so these checks are a second line
//! of defense.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use crate::ToolError;

/// The archive formats that the tool sources use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveKind {
    TarGz,
    TarBz2,
    Zip,
    /// Not an archive: the download is the program.
    Raw,
}

impl ArchiveKind {
    /// The kind of a download, from its URL or file name.
    pub(crate) fn from_name(name: &str) -> ArchiveKind {
        let path = name.split(['?', '#']).next().unwrap_or(name);
        let file = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        if file.ends_with(".tar.gz") || file.ends_with(".tgz") {
            ArchiveKind::TarGz
        } else if file.ends_with(".tar.bz2") || file.ends_with(".tbz2") {
            ArchiveKind::TarBz2
        } else if file.ends_with(".zip") {
            ArchiveKind::Zip
        } else {
            ArchiveKind::Raw
        }
    }
}

/// Unpacks `archive` into the existing folder `dest`.
///
/// `strip` removes that many leading folders from each entry path, like
/// `tar --strip-components`. Entries that have no path left are skipped.
pub(crate) fn extract(
    archive: &Path,
    kind: ArchiveKind,
    dest: &Path,
    strip: usize,
) -> Result<(), ToolError> {
    let root = dest
        .canonicalize()
        .map_err(ToolError::io(format!("cannot open {}", dest.display())))?;
    let file =
        File::open(archive).map_err(ToolError::io(format!("cannot open {}", archive.display())))?;
    match kind {
        ArchiveKind::TarGz => extract_tar(flate2::read::GzDecoder::new(file), &root, strip),
        ArchiveKind::TarBz2 => extract_tar(bzip2::read::BzDecoder::new(file), &root, strip),
        ArchiveKind::Zip => extract_zip(file, &root, strip),
        ArchiveKind::Raw => Err(ToolError::Archive {
            message: "a raw binary is not an archive".to_owned(),
        }),
    }
}

fn archive_error(err: impl std::fmt::Display) -> ToolError {
    ToolError::Archive {
        message: err.to_string(),
    }
}

fn extract_tar(reader: impl Read, root: &Path, strip: usize) -> Result<(), ToolError> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(archive_error)? {
        let mut entry = entry.map_err(archive_error)?;
        let raw_path = entry.path().map_err(archive_error)?.into_owned();
        let Some(rel) = safe_relative(&raw_path, strip)? else {
            continue;
        };
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            create_dir(root, &rel)?;
        } else if kind.is_symlink() {
            let target = entry
                .link_name()
                .map_err(archive_error)?
                .ok_or_else(|| archive_error("a symbolic link has no target"))?
                .into_owned();
            create_symlink(root, &rel, &target)?;
        } else if kind.is_hard_link() {
            let source = entry
                .link_name()
                .map_err(archive_error)?
                .ok_or_else(|| archive_error("a hard link has no target"))?
                .into_owned();
            let source = safe_relative(&source, strip)?.ok_or_else(|| ToolError::UnsafePath {
                path: source.display().to_string(),
            })?;
            let out = prepare_file(root, &rel)?;
            let from = root.join(&source);
            fs::copy(&from, &out).map_err(ToolError::io(format!(
                "cannot copy {} to {}",
                from.display(),
                out.display()
            )))?;
        } else if kind.is_file() || kind == tar::EntryType::Continuous {
            let mode = entry.header().mode().unwrap_or(0o644);
            write_file(root, &rel, &mut entry, mode)?;
        }
        // Devices, FIFOs and other entry types are skipped.
    }
    Ok(())
}

fn extract_zip(file: File, root: &Path, strip: usize) -> Result<(), ToolError> {
    let mut archive = zip::ZipArchive::new(file).map_err(archive_error)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(archive_error)?;
        // Zip files made on Windows can use `\` as the separator.
        let name = entry.name().map_err(archive_error)?.replace('\\', "/");
        let Some(rel) = safe_relative(Path::new(&name), strip)? else {
            continue;
        };
        if entry.is_dir() {
            create_dir(root, &rel)?;
        } else if entry.is_symlink() {
            let mut target = String::new();
            entry.read_to_string(&mut target).map_err(archive_error)?;
            create_symlink(root, &rel, Path::new(&target))?;
        } else {
            let mode = entry.unix_mode().unwrap_or(0o644);
            write_file(root, &rel, &mut entry, mode)?;
        }
    }
    Ok(())
}

/// Turns an entry path into a safe relative path, after removing `strip`
/// leading folders. Returns `None` when nothing is left.
pub(crate) fn safe_relative(path: &Path, strip: usize) -> Result<Option<PathBuf>, ToolError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ToolError::UnsafePath {
                    path: path.display().to_string(),
                });
            }
        }
    }
    if parts.len() <= strip {
        return Ok(None);
    }
    Ok(Some(parts[strip..].iter().collect()))
}

/// Creates the parent folders of `rel` and checks that the parent is inside
/// `root` on disk. Returns the full output path. An old file or link at the
/// output path is removed, so the write never follows a link.
fn prepare_file(root: &Path, rel: &Path) -> Result<PathBuf, ToolError> {
    let out = root.join(rel);
    let parent = out.parent().unwrap_or(root);
    fs::create_dir_all(parent)
        .map_err(ToolError::io(format!("cannot create {}", parent.display())))?;
    let real_parent = parent
        .canonicalize()
        .map_err(ToolError::io(format!("cannot open {}", parent.display())))?;
    if !real_parent.starts_with(root) {
        return Err(ToolError::UnsafePath {
            path: rel.display().to_string(),
        });
    }
    if let Ok(meta) = fs::symlink_metadata(&out) {
        let removed = if meta.is_dir() {
            fs::remove_dir_all(&out)
        } else {
            fs::remove_file(&out)
        };
        removed.map_err(ToolError::io(format!("cannot replace {}", out.display())))?;
    }
    Ok(out)
}

fn create_dir(root: &Path, rel: &Path) -> Result<(), ToolError> {
    let out = root.join(rel);
    fs::create_dir_all(&out).map_err(ToolError::io(format!("cannot create {}", out.display())))?;
    let real = out
        .canonicalize()
        .map_err(ToolError::io(format!("cannot open {}", out.display())))?;
    if !real.starts_with(root) {
        return Err(ToolError::UnsafePath {
            path: rel.display().to_string(),
        });
    }
    Ok(())
}

fn write_file(root: &Path, rel: &Path, reader: &mut impl Read, mode: u32) -> Result<(), ToolError> {
    let out = prepare_file(root, rel)?;
    let mut file =
        File::create(&out).map_err(ToolError::io(format!("cannot create {}", out.display())))?;
    io::copy(reader, &mut file)
        .map_err(ToolError::io(format!("cannot write {}", out.display())))?;
    drop(file);
    set_mode(&out, mode)
}

/// Makes a file executable for everyone when any execute bit is set, else
/// readable for everyone. Other mode bits from the archive are ignored.
#[cfg(unix)]
pub(crate) fn set_mode(path: &Path, mode: u32) -> Result<(), ToolError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(ToolError::io(format!(
        "cannot set mode of {}",
        path.display()
    )))
}

#[cfg(not(unix))]
pub(crate) fn set_mode(_path: &Path, _mode: u32) -> Result<(), ToolError> {
    Ok(())
}

/// Checks that a link target stays inside the root.
///
/// `base` is the real folder of the link, relative to the root. The target
/// must be relative, and `..` may only come at its start. So the target never
/// goes up through a folder that is itself a link.
fn check_link_target(base: &Path, rel: &Path, target: &Path) -> Result<(), ToolError> {
    let unsafe_link = || ToolError::UnsafePath {
        path: format!("{} -> {}", rel.display(), target.display()),
    };
    let mut depth = base
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count();
    let mut descended = false;
    for component in target.components() {
        match component {
            Component::Normal(_) => {
                depth += 1;
                descended = true;
            }
            Component::CurDir => {}
            Component::ParentDir if !descended && depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(unsafe_link());
            }
        }
    }
    Ok(())
}

/// Checks the target of a link that will be created at `rel`, from the real
/// folder of the link on disk.
fn check_link(root: &Path, rel: &Path, target: &Path) -> Result<PathBuf, ToolError> {
    // Check the text first, so an unsafe link makes no folders.
    check_link_target(rel.parent().unwrap_or(Path::new("")), rel, target)?;
    let out = prepare_file(root, rel)?;
    let parent = out.parent().unwrap_or(root);
    let real_parent = parent
        .canonicalize()
        .map_err(ToolError::io(format!("cannot open {}", parent.display())))?;
    let base = real_parent.strip_prefix(root).unwrap_or(Path::new(""));
    check_link_target(base, rel, target)?;
    Ok(out)
}

#[cfg(unix)]
fn create_symlink(root: &Path, rel: &Path, target: &Path) -> Result<(), ToolError> {
    let out = check_link(root, rel, target)?;
    std::os::unix::fs::symlink(target, &out).map_err(ToolError::io(format!(
        "cannot create link {}",
        out.display()
    )))
}

/// Windows needs extra rights for symbolic links. The tool archives for
/// Windows do not use them, so a link is checked and then skipped.
#[cfg(not(unix))]
fn create_symlink(root: &Path, rel: &Path, target: &Path) -> Result<(), ToolError> {
    check_link(root, rel, target)?;
    tracing::warn!(path = %rel.display(), "skipped a symbolic link in an archive");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tar_gz(entries: &[(&str, &[u8], Option<&str>)]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for (path, data, link) in entries {
            let mut header = tar::Header::new_gnu();
            // Write the name bytes directly, so tests can build bad paths.
            let name = &mut header.as_old_mut().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
            match link {
                Some(target) => {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_link_name(target).unwrap();
                    header.set_size(0);
                }
                None => {
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_size(data.len() as u64);
                }
            }
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn unpack(
        bytes: &[u8],
        kind: ArchiveKind,
        strip: usize,
    ) -> (tempfile::TempDir, Result<(), ToolError>) {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("archive");
        fs::write(&archive, bytes).unwrap();
        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let result = extract(&archive, kind, &dest, strip);
        (temp, result)
    }

    #[test]
    fn kinds_from_names() {
        assert_eq!(
            ArchiveKind::from_name("https://x/a.tar.gz?x=1"),
            ArchiveKind::TarGz
        );
        assert_eq!(ArchiveKind::from_name("a.TGZ"), ArchiveKind::TarGz);
        assert_eq!(ArchiveKind::from_name("a.tar.bz2"), ArchiveKind::TarBz2);
        assert_eq!(ArchiveKind::from_name("a.tbz2"), ArchiveKind::TarBz2);
        assert_eq!(ArchiveKind::from_name("https://x/a.zip"), ArchiveKind::Zip);
        assert_eq!(
            ArchiveKind::from_name("https://x/agent.exe"),
            ArchiveKind::Raw
        );
        assert_eq!(
            ArchiveKind::from_name("https://x/amp-acp"),
            ArchiveKind::Raw
        );
    }

    #[test]
    fn unpacks_tar_gz_with_strip_and_links() {
        let bytes = tar_gz(&[
            ("node-v1/bin/node", b"#!/bin/sh\n", None),
            ("node-v1/lib/npm-cli.js", b"x", None),
            ("node-v1/bin/npm", b"", Some("../lib/npm-cli.js")),
        ]);
        let (temp, result) = unpack(&bytes, ArchiveKind::TarGz, 1);
        result.unwrap();
        let dest = temp.path().join("dest");
        assert_eq!(fs::read(dest.join("bin/node")).unwrap(), b"#!/bin/sh\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dest.join("bin/node"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
            assert_eq!(fs::read(dest.join("bin/npm")).unwrap(), b"x");
        }
    }

    #[test]
    fn refuses_parent_paths_in_tar() {
        let bytes = tar_gz(&[("ok.txt", b"ok", None), ("../evil.txt", b"evil", None)]);
        let (temp, result) = unpack(&bytes, ArchiveKind::TarGz, 0);
        assert!(
            matches!(result, Err(ToolError::UnsafePath { .. })),
            "{result:?}"
        );
        assert!(!temp.path().join("evil.txt").exists());
    }

    #[test]
    fn refuses_absolute_paths_in_tar() {
        let bytes = tar_gz(&[("/tmp/consort-evil.txt", b"evil", None)]);
        let (_temp, result) = unpack(&bytes, ArchiveKind::TarGz, 0);
        assert!(
            matches!(result, Err(ToolError::UnsafePath { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn refuses_links_that_leave_the_folder() {
        for target in ["../outside", "/etc/passwd", "a/../../outside", "a/../b"] {
            let bytes = tar_gz(&[("link", b"", Some(target))]);
            let (_temp, result) = unpack(&bytes, ArchiveKind::TarGz, 0);
            assert!(
                matches!(result, Err(ToolError::UnsafePath { .. })),
                "{target}: {result:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_link_chain_that_leaves_the_folder() {
        // `a/up` points at the root, which is allowed. A link under it that
        // goes up again would leave the folder, because its real folder is
        // the root.
        let bytes = tar_gz(&[
            ("a/file", b"x", None),
            ("a/up", b"", Some("..")),
            ("a/up/escape", b"", Some("../outside")),
        ]);
        let (temp, result) = unpack(&bytes, ArchiveKind::TarGz, 0);
        assert!(
            matches!(result, Err(ToolError::UnsafePath { .. })),
            "{result:?}"
        );
        assert!(fs::symlink_metadata(temp.path().join("dest/escape")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn follows_a_link_inside_the_folder() {
        // `self` points at the folder itself. A file written through it
        // stays inside, so it is allowed.
        let bytes = tar_gz(&[("self", b"", Some(".")), ("self/inner.txt", b"ok", None)]);
        let (temp, result) = unpack(&bytes, ArchiveKind::TarGz, 0);
        result.unwrap();
        assert_eq!(fs::read(temp.path().join("dest/inner.txt")).unwrap(), b"ok");
    }

    #[test]
    fn unpacks_zip_and_refuses_traversal() {
        let mut good = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        good.start_file("opencode", options).unwrap();
        good.write_all(b"bin").unwrap();
        let good = good.finish().unwrap().into_inner();
        let (temp, result) = unpack(&good, ArchiveKind::Zip, 0);
        result.unwrap();
        assert_eq!(fs::read(temp.path().join("dest/opencode")).unwrap(), b"bin");

        let mut bad = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        bad.start_file("../evil.txt", options).unwrap();
        bad.write_all(b"evil").unwrap();
        let bad = bad.finish().unwrap().into_inner();
        let (temp, result) = unpack(&bad, ArchiveKind::Zip, 0);
        assert!(
            matches!(result, Err(ToolError::UnsafePath { .. })),
            "{result:?}"
        );
        assert!(!temp.path().join("evil.txt").exists());
    }

    #[test]
    fn unpacks_tar_bz2() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "goose", &b"goos"[..])
            .unwrap();
        let tar = builder.into_inner().unwrap();
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        encoder.write_all(&tar).unwrap();
        let bytes = encoder.finish().unwrap();
        let (temp, result) = unpack(&bytes, ArchiveKind::TarBz2, 0);
        result.unwrap();
        assert_eq!(fs::read(temp.path().join("dest/goose")).unwrap(), b"goos");
    }
}
