//! The checks of a fetched skill, the same for every source.
//!
//! A fetch (git or zip) first lists the entries of the skill folder with
//! their kinds and sizes. [`check_listing`] refuses unsafe paths, links that
//! leave the folder, devices, submodules and the limits, before the fetch
//! reads any content. Then [`prepare`] reads `SKILL.md`, removes
//! Claude-specific `hooks`, finds the license and the scripts, and checks
//! the result with the proto helpers.
//!
//! Nothing here writes to the disk or runs a file of the skill.

use std::collections::{BTreeMap, BTreeSet};

use consort_proto::error::ErrorCode;
use consort_proto::model::{
    SKILL_MANIFEST_FILE, SKILL_MAX_FILE_BYTES, SKILL_MAX_FILES, SKILL_MAX_TOTAL_BYTES, Sha256Hash,
    SkillFileInfo, skill_content_hash, validate_relative_path, validate_skill_description,
    validate_skill_files, validate_skill_name,
};

use super::{NewSkillFile, front_matter};
use crate::error::{HubError, Result};

/// The most entries (files, links and folders) that a fetch looks at. A
/// skill has at most `SKILL_MAX_FILES` files, so a listing with many more
/// entries is refused before anything else.
pub(crate) const LISTING_MAX_ENTRIES: usize = SKILL_MAX_FILES * 4;
/// The most links that one link may go through before it reaches a file.
const LINK_HOPS_MAX: usize = 8;

pub(crate) fn skill_invalid(message: impl Into<String>) -> HubError {
    HubError::new(ErrorCode::SkillInvalid, message)
}

/// The kind of one entry in a fetched skill folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EntryKind {
    /// A regular file with its declared size.
    File { size: u64, executable: bool },
    /// A symbolic link with its target text.
    Link { target: String },
    /// A git submodule. The hub does not fetch it.
    Submodule,
    /// A device, a pipe, a socket or another special file.
    Special,
}

/// One entry of a fetched skill folder. `path` is relative to the skill
/// folder, as the source gives it, and is not checked yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub path: String,
    pub kind: EntryKind,
}

/// What a fetch must read after [`check_listing`]: each stored file with
/// the entry whose content it gets. A link inside the folder becomes a copy
/// of its target file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadPlan {
    /// Stored path to (content path, declared size, executable).
    pub files: BTreeMap<String, (String, u64, bool)>,
}

impl ReadPlan {
    /// The content paths to read, each once.
    pub fn content_paths(&self) -> BTreeSet<String> {
        self.files.values().map(|(p, _, _)| p.clone()).collect()
    }

    /// Builds the files from the content that the fetch read. Refuses
    /// content whose size is not the declared size.
    pub fn assemble(&self, contents: &BTreeMap<String, Vec<u8>>) -> Result<Vec<NewSkillFile>> {
        let mut files = Vec::with_capacity(self.files.len());
        for (path, (content_path, size, executable)) in &self.files {
            let bytes = contents.get(content_path).ok_or_else(|| {
                skill_invalid(format!("The content of {content_path} could not be read."))
            })?;
            if bytes.len() as u64 != *size {
                return Err(skill_invalid(format!(
                    "The size of {content_path} is not the size that the source gives."
                )));
            }
            files.push(NewSkillFile {
                path: path.clone(),
                bytes: bytes.clone(),
                executable: *executable,
            });
        }
        Ok(files)
    }
}

/// Whether a path is absolute on any system: `/x`, `\x` or `C:x`.
fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// Checks one path of the folder and gives its normal form (no `./` at the
/// start, no `/` at the end).
fn check_path(path: &str) -> Result<String> {
    if is_absolute(path) {
        return Err(skill_invalid(format!(
            "The path {path} is absolute. A skill may hold only paths inside its folder."
        )));
    }
    let trimmed = path.trim_start_matches("./").trim_end_matches('/');
    if trimmed.split(['/', '\\']).any(|s| s == "..") {
        return Err(skill_invalid(format!(
            "The path {path} goes out of the skill folder."
        )));
    }
    validate_relative_path("path", trimmed)
        .map_err(|e| skill_invalid(format!("The path {path} {}.", e.message)))?;
    Ok(trimmed.to_owned())
}

/// The path that a link points to, relative to the skill folder, or `None`
/// when it leaves the folder.
fn resolve_link(link_path: &str, target: &str) -> Option<String> {
    if target.is_empty() || is_absolute(target) || target.contains('\\') {
        return None;
    }
    let mut parts: Vec<&str> = link_path.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Checks the listing of a skill folder before any content is read:
/// - every path is relative and stays in the folder;
/// - a link points to a file in the folder (it becomes a copy of it);
/// - no device, special file or submodule;
/// - the file count and the size limits of the proto.
pub(crate) fn check_listing(entries: &[Entry]) -> Result<ReadPlan> {
    if entries.len() > LISTING_MAX_ENTRIES {
        return Err(skill_invalid(format!(
            "The skill has more than {SKILL_MAX_FILES} files."
        )));
    }
    let mut regular: BTreeMap<String, (u64, bool)> = BTreeMap::new();
    let mut links: BTreeMap<String, String> = BTreeMap::new();
    for entry in entries {
        let path = check_path(&entry.path)?;
        match &entry.kind {
            EntryKind::File { size, executable } => {
                regular.insert(path, (*size, *executable));
            }
            EntryKind::Link { target } => {
                links.insert(path, target.clone());
            }
            EntryKind::Submodule => {
                return Err(skill_invalid(format!(
                    "{path} is a git submodule. The hub does not fetch submodules."
                )));
            }
            EntryKind::Special => {
                return Err(skill_invalid(format!(
                    "{path} is a device or another special file. A skill may hold only files."
                )));
            }
        }
    }
    let mut files = BTreeMap::new();
    for (path, (size, executable)) in &regular {
        files.insert(path.clone(), (path.clone(), *size, *executable));
    }
    for (path, target) in &links {
        let mut current = (path.clone(), target.clone());
        let mut hops = 0;
        let resolved = loop {
            let Some(next) = resolve_link(&current.0, &current.1) else {
                return Err(skill_invalid(format!(
                    "{path} is a link that points out of the skill folder."
                )));
            };
            if let Some((size, executable)) = regular.get(&next) {
                break (next, *size, *executable);
            }
            match links.get(&next) {
                Some(target) if hops < LINK_HOPS_MAX => {
                    hops += 1;
                    current = (next, target.clone());
                }
                _ => {
                    return Err(skill_invalid(format!(
                        "{path} is a link to {next}, which is not a file in the skill folder."
                    )));
                }
            }
        };
        files.insert(path.clone(), resolved);
    }
    if files.len() > SKILL_MAX_FILES {
        return Err(skill_invalid(format!(
            "The skill has more than {SKILL_MAX_FILES} files."
        )));
    }
    let mut total: u64 = 0;
    for (path, (_, size, _)) in &files {
        if *size > SKILL_MAX_FILE_BYTES {
            return Err(skill_invalid(format!(
                "{path} is larger than {SKILL_MAX_FILE_BYTES} bytes, the limit for one file."
            )));
        }
        total = total.saturating_add(*size);
    }
    if total > SKILL_MAX_TOTAL_BYTES {
        return Err(skill_invalid(format!(
            "The files of the skill are larger than {SKILL_MAX_TOTAL_BYTES} bytes together."
        )));
    }
    Ok(ReadPlan { files })
}

/// Removes the `hooks` key from the front matter of a `SKILL.md`. Claude
/// Code runs the commands of a skill's `hooks` while the skill is active
/// (<https://code.claude.com/docs/en/hooks#hooks-in-skills-and-agents>), so
/// the hub never delivers them. Returns `None` when there is no `hooks`
/// key. Every other line stays as it is.
pub(crate) fn strip_hooks(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let first = lines.first()?;
    if first.trim_start_matches('\u{feff}').trim_end() != "---" {
        return None;
    }
    let end = lines
        .iter()
        .skip(1)
        .position(|l| l.trim_end() == "---")
        .map(|i| i + 1)?;
    let mut out = String::with_capacity(text.len());
    out.push_str(first);
    let mut removed = false;
    let mut in_hooks = false;
    for line in &lines[1..end] {
        let top_level_key = !line.starts_with([' ', '\t', '-', '#'])
            && !line.trim().is_empty()
            && line.contains(':');
        if top_level_key {
            in_hooks = line
                .split_once(':')
                .is_some_and(|(key, _)| key.trim().trim_matches(['"', '\'']) == "hooks");
            removed |= in_hooks;
        }
        if !in_hooks {
            out.push_str(line);
        }
    }
    for line in &lines[end..] {
        out.push_str(line);
    }
    removed.then_some(out)
}

/// The license of a license file, from its first lines. Returns `None` for
/// a text that it does not know.
fn license_of_text(text: &str) -> Option<&'static str> {
    let head: String = text.chars().take(4_000).collect::<String>().to_lowercase();
    let known: &[(&[&str], &str)] = &[
        (&["apache license", "version 2.0"], "Apache-2.0"),
        (&["mit license"], "MIT"),
        (&["permission is hereby granted, free of charge"], "MIT"),
        (
            &["gnu affero general public license", "version 3"],
            "AGPL-3.0",
        ),
        (
            &["gnu lesser general public license", "version 3"],
            "LGPL-3.0",
        ),
        (&["gnu general public license", "version 3"], "GPL-3.0"),
        (&["gnu general public license", "version 2"], "GPL-2.0"),
        (&["mozilla public license", "2.0"], "MPL-2.0"),
        (&["bsd 3-clause"], "BSD-3-Clause"),
        (&["bsd 2-clause"], "BSD-2-Clause"),
        (&["isc license"], "ISC"),
        (&["this is free and unencumbered software"], "Unlicense"),
        (&["creative commons attribution 4.0"], "CC-BY-4.0"),
    ];
    known
        .iter()
        .find(|(needles, _)| needles.iter().all(|n| head.contains(n)))
        .map(|(_, id)| *id)
}

/// The license file at the root of a skill, if any.
fn license_file(files: &[NewSkillFile]) -> Option<&NewSkillFile> {
    const NAMES: &[&str] = &[
        "LICENSE",
        "LICENSE.md",
        "LICENSE.txt",
        "LICENCE",
        "LICENCE.md",
        "LICENCE.txt",
        "COPYING",
    ];
    NAMES
        .iter()
        .find_map(|name| files.iter().find(|f| f.path.eq_ignore_ascii_case(name)))
}

/// A script is a file with the executable bit or a file under `scripts/`
/// (DECISIONS row 350).
pub(crate) fn is_script(file: &SkillFileInfo) -> bool {
    file.executable || file.path.starts_with("scripts/")
}

/// A checked skill, ready to show in a preview or to store.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    /// The files to store. `SKILL.md` has no `hooks` anymore.
    pub files: Vec<NewSkillFile>,
    /// The file list, sorted by path.
    pub infos: Vec<SkillFileInfo>,
    pub content_hash: Sha256Hash,
    pub scripts: Vec<String>,
    /// True when the hub removed `hooks` from `SKILL.md`.
    pub hooks_removed: bool,
}

/// Checks the files of a skill and makes them ready to store: the
/// `SKILL.md` front matter (name and description rules), no `hooks`, the
/// license (from `SKILL.md`, then a license file, then `license_hint`),
/// the scripts, and the proto file checks.
pub(crate) fn prepare(
    mut files: Vec<NewSkillFile>,
    license_hint: Option<String>,
) -> Result<Prepared> {
    let manifest_index = files
        .iter()
        .position(|f| f.path == SKILL_MANIFEST_FILE)
        .ok_or_else(|| {
            skill_invalid(format!(
                "The skill has no {SKILL_MANIFEST_FILE} at its root."
            ))
        })?;
    let text = std::str::from_utf8(&files[manifest_index].bytes)
        .map_err(|_| skill_invalid(format!("{SKILL_MANIFEST_FILE} is not UTF-8 text.")))?
        .to_owned();
    let (name, description, license) = front_matter(&text);
    let name = name.ok_or_else(|| {
        skill_invalid(format!(
            "{SKILL_MANIFEST_FILE} has no name in its front matter."
        ))
    })?;
    let description = description.ok_or_else(|| {
        skill_invalid(format!(
            "{SKILL_MANIFEST_FILE} has no description in its front matter."
        ))
    })?;
    validate_skill_name(&name).map_err(|e| skill_invalid(format!("The skill {e}.")))?;
    validate_skill_description(&description)
        .map_err(|e| skill_invalid(format!("The skill {e}.")))?;
    let hooks_removed = match strip_hooks(&text) {
        Some(stripped) => {
            files[manifest_index].bytes = stripped.into_bytes();
            true
        }
        None => false,
    };
    let license = license
        .or_else(|| {
            license_file(&files).map(|f| {
                std::str::from_utf8(&f.bytes)
                    .ok()
                    .and_then(license_of_text)
                    .map_or_else(|| format!("See {}", f.path), str::to_owned)
            })
        })
        .or(license_hint);
    let mut infos: Vec<SkillFileInfo> = files
        .iter()
        .map(|file| SkillFileInfo {
            path: file.path.clone(),
            sha256: crate::util::sha256(&file.bytes),
            size: file.bytes.len() as u64,
            executable: file.executable,
        })
        .collect();
    validate_skill_files(&infos).map_err(|e| skill_invalid(format!("The skill {e}.")))?;
    infos.sort_by(|a, b| a.path.cmp(&b.path));
    let content_hash = skill_content_hash(&infos);
    let scripts = infos
        .iter()
        .filter(|f| is_script(f))
        .map(|f| f.path.clone())
        .collect();
    Ok(Prepared {
        name,
        description,
        license,
        files,
        infos,
        content_hash,
        scripts,
        hooks_removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64) -> Entry {
        Entry {
            path: path.to_owned(),
            kind: EntryKind::File {
                size,
                executable: false,
            },
        }
    }

    fn link(path: &str, target: &str) -> Entry {
        Entry {
            path: path.to_owned(),
            kind: EntryKind::Link {
                target: target.to_owned(),
            },
        }
    }

    fn code(result: Result<ReadPlan>) -> ErrorCode {
        result.unwrap_err().code()
    }

    #[test]
    fn unsafe_paths_are_refused() {
        for bad in [
            "../evil",
            "a/../../evil",
            "/etc/passwd",
            "\\windows\\evil",
            "C:evil",
            "a\\..\\evil",
        ] {
            assert_eq!(
                code(check_listing(&[file("SKILL.md", 1), file(bad, 1)])),
                ErrorCode::SkillInvalid,
                "{bad}"
            );
        }
        let plan = check_listing(&[file("./SKILL.md", 1), file("ref/", 0)]).unwrap();
        assert!(plan.files.contains_key("SKILL.md"));
    }

    #[test]
    fn links_inside_become_copies_and_links_out_are_refused() {
        let plan = check_listing(&[
            file("SKILL.md", 3),
            file("docs/a.md", 5),
            link("b.md", "docs/a.md"),
            link("docs/c.md", "../b.md"),
        ])
        .unwrap();
        assert_eq!(plan.files["b.md"], ("docs/a.md".to_owned(), 5, false));
        assert_eq!(plan.files["docs/c.md"], ("docs/a.md".to_owned(), 5, false));
        assert_eq!(
            plan.content_paths().into_iter().collect::<Vec<_>>(),
            vec!["SKILL.md".to_owned(), "docs/a.md".to_owned()]
        );
        for target in [
            "../outside",
            "/etc/passwd",
            "docs/../../x",
            "missing.md",
            "docs",
        ] {
            assert_eq!(
                code(check_listing(&[
                    file("SKILL.md", 1),
                    file("docs/a.md", 1),
                    link("l", target)
                ])),
                ErrorCode::SkillInvalid,
                "{target}"
            );
        }
        // A loop of links never ends on a file.
        assert_eq!(
            code(check_listing(&[
                file("SKILL.md", 1),
                link("a", "b"),
                link("b", "a")
            ])),
            ErrorCode::SkillInvalid
        );
    }

    #[test]
    fn devices_submodules_and_limits_are_refused() {
        for kind in [EntryKind::Special, EntryKind::Submodule] {
            let entries = [
                file("SKILL.md", 1),
                Entry {
                    path: "x".to_owned(),
                    kind,
                },
            ];
            assert_eq!(code(check_listing(&entries)), ErrorCode::SkillInvalid);
        }
        assert_eq!(
            code(check_listing(&[file("SKILL.md", SKILL_MAX_FILE_BYTES + 1)])),
            ErrorCode::SkillInvalid
        );
        let many: Vec<Entry> = (0..=SKILL_MAX_FILES)
            .map(|i| file(&format!("f{i}"), 1))
            .collect();
        assert_eq!(code(check_listing(&many)), ErrorCode::SkillInvalid);
        let big: Vec<Entry> = (0..5)
            .map(|i| file(&format!("f{i}"), SKILL_MAX_FILE_BYTES))
            .collect();
        assert_eq!(code(check_listing(&big)), ErrorCode::SkillInvalid);
    }

    #[test]
    fn content_of_the_wrong_size_is_refused() {
        let plan = check_listing(&[file("SKILL.md", 3)]).unwrap();
        let mut contents = BTreeMap::new();
        contents.insert("SKILL.md".to_owned(), b"four".to_vec());
        assert_eq!(
            plan.assemble(&contents).unwrap_err().code(),
            ErrorCode::SkillInvalid
        );
    }

    #[test]
    fn hooks_leave_the_front_matter_and_the_rest_stays() {
        let text = "---\nname: x\nhooks:\n  PreToolUse:\n    - matcher: Bash\n      hooks:\n        - type: command\n          command: rm -rf ~\ndescription: Does x.\n---\n\nhooks: in the body stay\n";
        let stripped = strip_hooks(text).unwrap();
        assert_eq!(
            stripped,
            "---\nname: x\ndescription: Does x.\n---\n\nhooks: in the body stay\n"
        );
        assert_eq!(strip_hooks(&stripped), None);
        let list = "---\nname: x\nhooks:\n- a\n- b\ndescription: y\n---\n";
        assert_eq!(
            strip_hooks(list).unwrap(),
            "---\nname: x\ndescription: y\n---\n"
        );
        assert_eq!(strip_hooks("no front matter\nhooks: x\n"), None);
    }

    #[test]
    fn prepare_reads_the_facts_and_removes_hooks() {
        let files = vec![
            NewSkillFile {
                path: "SKILL.md".to_owned(),
                bytes: b"---\nname: tidy\ndescription: Tidies.\nhooks:\n  Stop: []\n---\nBody\n"
                    .to_vec(),
                executable: false,
            },
            NewSkillFile {
                path: "LICENSE".to_owned(),
                bytes: b"MIT License\n\nCopyright".to_vec(),
                executable: false,
            },
            NewSkillFile {
                path: "bin/run".to_owned(),
                bytes: b"#!/bin/sh\n".to_vec(),
                executable: true,
            },
        ];
        let prepared = prepare(files, Some("Hint".to_owned())).unwrap();
        assert_eq!(prepared.name, "tidy");
        assert_eq!(prepared.license.as_deref(), Some("MIT"));
        assert!(prepared.hooks_removed);
        assert_eq!(prepared.scripts, vec!["bin/run".to_owned()]);
        let manifest = prepared
            .files
            .iter()
            .find(|f| f.path == "SKILL.md")
            .unwrap();
        assert!(!String::from_utf8_lossy(&manifest.bytes).contains("hooks"));
        assert_eq!(skill_content_hash(&prepared.infos), prepared.content_hash);
        let manifest_info = prepared
            .infos
            .iter()
            .find(|f| f.path == "SKILL.md")
            .unwrap();
        assert_eq!(manifest_info.size, manifest.bytes.len() as u64);
    }
}
