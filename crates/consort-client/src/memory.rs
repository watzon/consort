//! Memory in the client: the views of memory notes, the tree by path that
//! the Memory tab and Settings > Memory show, the words about who saved a
//! version, and the changed lines of a version preview.

use std::collections::{BTreeMap, BTreeSet};

/// The scope text of global memory. A channel's memory uses the channel ID.
pub const GLOBAL: &str = "global";

/// A note that the assistant changed this recently shows "Just now" with an
/// accent dot, until the user opens it.
pub const JUST_NOW_MS: i64 = 10 * 60 * 1000;

/// One memory note, or one version of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryNoteView {
    /// `global` or a channel ID.
    pub scope: String,
    pub path: String,
    /// The text, once the client read it. A list gives only the summary.
    pub content: Option<String>,
    pub sha256: String,
    pub version: u64,
    /// The user wrote this version. Otherwise the assistant did.
    pub by_user: bool,
    pub updated_at: i64,
    /// The task or channel whose memory suggestion this version keeps: its
    /// ID and its title or name.
    pub suggested_by: Option<(String, String)>,
    /// Set while the note is in Recently deleted.
    pub deleted_at: Option<i64>,
}

impl MemoryNoteView {
    /// The feed entity ID: `<scope>:<path>`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.scope, self.path)
    }

    /// The last part of the path.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// Whether the assistant changed it in the last [`JUST_NOW_MS`].
    pub fn just_changed(&self, now_ms: i64) -> bool {
        !self.by_user && self.deleted_at.is_none() && now_ms - self.updated_at < JUST_NOW_MS
    }
}

/// The scope of a channel's memory in the client: `global` for Tutti, else
/// the channel ID.
pub fn scope_of(channel_id: &str, is_tutti: bool) -> String {
    if is_tutti {
        GLOBAL.to_owned()
    } else {
        channel_id.to_owned()
    }
}

/// One row of the memory tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryRow {
    Folder {
        /// The folder path, such as `people` or `places/berlin`.
        path: String,
        name: String,
        depth: usize,
        /// The notes in the folder and its folders.
        count: usize,
        open: bool,
    },
    Note {
        path: String,
        name: String,
        depth: usize,
        /// "Today", "Oct 27", "You, Oct 12", or "Just now".
        date: String,
        /// The assistant just changed it and the user has not opened it.
        just_now: bool,
    },
}

/// The tree of the notes of one scope that are not deleted: folders first,
/// then notes, each sorted by name. A folder in `closed` shows no rows
/// under it. `seen` holds the keys of the notes that the user opened, with
/// the version they saw: a note with a newer version is "Just now" again.
pub fn tree(
    notes: &[MemoryNoteView],
    scope: &str,
    closed: &BTreeSet<String>,
    seen: &BTreeMap<String, u64>,
    now_ms: i64,
) -> Vec<MemoryRow> {
    let mut root = Folder::default();
    for note in notes
        .iter()
        .filter(|n| n.scope == scope && n.deleted_at.is_none())
    {
        let mut folder = &mut root;
        let parts: Vec<&str> = note.path.split('/').collect();
        let Some((_, folders)) = parts.split_last() else {
            continue;
        };
        for part in folders {
            folder = folder.folders.entry((*part).to_owned()).or_default();
        }
        folder.notes.push(note);
    }
    let mut out = Vec::new();
    root.rows("", 0, closed, seen, now_ms, &mut out);
    out
}

#[derive(Default)]
struct Folder<'a> {
    folders: BTreeMap<String, Folder<'a>>,
    notes: Vec<&'a MemoryNoteView>,
}

impl Folder<'_> {
    fn count(&self) -> usize {
        self.notes.len() + self.folders.values().map(Folder::count).sum::<usize>()
    }

    fn rows(
        &self,
        prefix: &str,
        depth: usize,
        closed: &BTreeSet<String>,
        seen: &BTreeMap<String, u64>,
        now_ms: i64,
        out: &mut Vec<MemoryRow>,
    ) {
        for (name, folder) in &self.folders {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let open = !closed.contains(&path);
            out.push(MemoryRow::Folder {
                path: path.clone(),
                name: name.clone(),
                depth,
                count: folder.count(),
                open,
            });
            if open {
                folder.rows(&path, depth + 1, closed, seen, now_ms, out);
            }
        }
        let mut notes = self.notes.clone();
        notes.sort_by(|a, b| a.name().cmp(b.name()));
        for note in notes {
            let unseen = seen.get(&note.key()).is_none_or(|v| *v < note.version);
            let just_now = note.just_changed(now_ms) && unseen;
            out.push(MemoryRow::Note {
                path: note.path.clone(),
                name: note.name().to_owned(),
                depth,
                date: if just_now {
                    "Just now".to_owned()
                } else {
                    date_label(note, now_ms)
                },
                just_now,
            });
        }
    }
}

/// "Today", "Oct 27", or "You, Oct 12" for a version that the user wrote.
pub fn date_label(note: &MemoryNoteView, now_ms: i64) -> String {
    let date = crate::format::file_date_label(note.updated_at, now_ms);
    if note.by_user {
        format!("You, {date}")
    } else {
        date
    }
}

/// Who writes the notes of a scope for the assistant: "Tutti" for global
/// memory, else "The assistant".
fn writer(scope: &str) -> &'static str {
    if scope == GLOBAL {
        "Tutti"
    } else {
        "The assistant"
    }
}

/// The line under a note's path: "Version 6. Tutti saved it on Oct 27, from
/// a suggestion by sayso."
pub fn saved_line(note: &MemoryNoteView, now_ms: i64) -> String {
    let who = if note.by_user {
        "You"
    } else {
        writer(&note.scope)
    };
    let when = crate::format::file_date_label(note.updated_at, now_ms);
    let when = if when == "Today" {
        format!("today at {}", clock(note.updated_at))
    } else {
        format!("on {when}")
    };
    let from = match &note.suggested_by {
        Some((_, name)) if !name.is_empty() => format!(", from a suggestion by {name}"),
        _ => String::new(),
    };
    format!("Version {}. {who} saved it {when}{from}.", note.version)
}

/// A version in the version list: "Oct 27, 20:14. Tutti, from sayso".
pub fn version_line(note: &MemoryNoteView, first: bool) -> String {
    let when = crate::format::local_date_time(note.updated_at);
    let who = if note.by_user {
        "You"
    } else {
        writer(&note.scope)
    };
    let mut line = format!("{when}. {who}");
    if first && !note.by_user {
        line.push_str(" created it");
    }
    if let Some((_, name)) = &note.suggested_by
        && !name.is_empty()
    {
        line.push_str(&format!(", from {name}"));
    }
    line
}

/// "20:41", the local time on a 24-hour clock.
pub fn clock(ms: i64) -> String {
    crate::format::clock_24(ms)
}

/// Which lines of `old` are not in `new`, by a longest common subsequence
/// of the lines. The version preview marks them with a bar.
pub fn changed_lines(old: &str, new: &str) -> Vec<bool> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // Notes are small (at most 256 KiB), but a table of every pair of lines
    // can still be large. Past this size, compare line by line.
    if a.len().saturating_mul(b.len()) > 4_000_000 {
        return a
            .iter()
            .enumerate()
            .map(|(i, line)| b.get(i) != Some(line))
            .collect();
    }
    let mut table = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut changed = vec![true; a.len()];
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            changed[i] = false;
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    changed
}

/// When a note in Recently deleted goes for good: "today", "in 1 day",
/// "in 27 days".
pub fn purge_label(deleted_at: i64, now_ms: i64) -> String {
    const DAY: i64 = 86_400_000;
    let keep = consort_proto::model::MEMORY_DELETED_KEEP_DAYS;
    let left = keep - (now_ms - deleted_at).max(0) / DAY;
    match left {
        i64::MIN..=0 => "today".to_owned(),
        1 => "in 1 day".to_owned(),
        n => format!("in {n} days"),
    }
}

/// Checks a note path that the user typed for a new note or a rename: a
/// relative path that ends in `.md`. A path with no `.md` gets it.
pub fn note_path(text: &str) -> Result<String, String> {
    let mut path = text.trim().trim_matches('/').to_owned();
    if path.is_empty() {
        return Err("Enter a name for the note, such as people/kevin.md.".to_owned());
    }
    if !path.ends_with(".md") {
        path.push_str(".md");
    }
    consort_proto::model::validate_memory_path(&path)
        .map_err(|_| "Use letters, digits and folders, such as people/kevin.md.".to_owned())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(scope: &str, path: &str, by_user: bool, updated_at: i64) -> MemoryNoteView {
        MemoryNoteView {
            scope: scope.into(),
            path: path.into(),
            content: None,
            sha256: "a".repeat(64),
            version: 1,
            by_user,
            updated_at,
            suggested_by: None,
            deleted_at: None,
        }
    }

    #[test]
    fn the_tree_lists_folders_first_and_hides_closed_folders() {
        let now = crate::format::now_ms();
        let old = now - 3 * 86_400_000;
        let notes = vec![
            note("chn_1", "house.md", false, old),
            note("chn_1", "decisions/roofline.md", false, now - 60_000),
            note("chn_1", "decisions/porch-switches.md", false, old),
            note("chn_1", "vendors/lumen-co.md", true, old),
            note("global", "about-me.md", false, old),
        ];
        let rows = tree(&notes, "chn_1", &BTreeSet::new(), &BTreeMap::new(), now);
        let names: Vec<String> = rows
            .iter()
            .map(|r| match r {
                MemoryRow::Folder { name, count, .. } => format!("{name}/{count}"),
                MemoryRow::Note { name, depth, .. } => format!("{depth}:{name}"),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "decisions/2",
                "1:porch-switches.md",
                "1:roofline.md",
                "vendors/1",
                "1:lumen-co.md",
                "0:house.md"
            ]
        );
        let MemoryRow::Note { date, just_now, .. } = &rows[2] else {
            panic!("not a note");
        };
        assert_eq!((date.as_str(), *just_now), ("Just now", true));
        let MemoryRow::Note { date, .. } = &rows[4] else {
            panic!("not a note");
        };
        assert!(date.starts_with("You, "), "{date}");

        // A closed folder hides its notes. An opened note is not new any more.
        let closed: BTreeSet<String> = ["decisions".to_owned()].into();
        let seen: BTreeMap<String, u64> = [("chn_1:decisions/roofline.md".to_owned(), 1)].into();
        assert_eq!(tree(&notes, "chn_1", &closed, &seen, now).len(), 4);
        let rows = tree(&notes, "chn_1", &BTreeSet::new(), &seen, now);
        assert!(matches!(
            &rows[2],
            MemoryRow::Note {
                just_now: false,
                ..
            }
        ));
    }

    #[test]
    fn a_deleted_note_is_not_in_the_tree() {
        let mut gone = note("global", "a.md", false, 0);
        gone.deleted_at = Some(5);
        assert!(tree(&[gone], "global", &BTreeSet::new(), &BTreeMap::new(), 10).is_empty());
    }

    #[test]
    fn changed_lines_mark_what_the_newer_text_does_not_have() {
        let old = "## Kevin\n\nLeipzig.\n- Prefers email.\n- Windows.";
        let new = "## Kevin\n\nLeipzig.\n- Prefers Signal.\n- Windows.";
        assert_eq!(
            changed_lines(old, new),
            vec![false, false, false, true, false]
        );
        assert_eq!(changed_lines("a\nb", "a\nb"), vec![false, false]);
    }

    #[test]
    fn the_saved_line_names_the_writer_and_the_suggestion() {
        let now = crate::format::now_ms();
        let mut kevin = note("global", "people/kevin.md", false, now - 3 * 86_400_000);
        kevin.version = 6;
        kevin.suggested_by = Some(("chn_1".into(), "sayso".into()));
        let line = saved_line(&kevin, now);
        assert!(line.starts_with("Version 6. Tutti saved it on "), "{line}");
        assert!(line.ends_with(", from a suggestion by sayso."), "{line}");
        let mine = note("chn_1", "a.md", true, now);
        assert!(saved_line(&mine, now).starts_with("Version 1. You saved it today at "));
    }

    #[test]
    fn a_deleted_note_says_when_it_goes_for_good() {
        let day = 86_400_000;
        assert_eq!(purge_label(0, 3 * day), "in 27 days");
        assert_eq!(purge_label(0, 29 * day), "in 1 day");
        assert_eq!(purge_label(0, 31 * day), "today");
    }

    #[test]
    fn a_note_path_gets_md_and_refuses_unsafe_paths() {
        assert_eq!(note_path("people/kevin"), Ok("people/kevin.md".into()));
        assert_eq!(note_path(" /a.md/ "), Ok("a.md".into()));
        assert!(note_path("../a.md").is_err());
        assert!(note_path("  ").is_err());
    }
}
