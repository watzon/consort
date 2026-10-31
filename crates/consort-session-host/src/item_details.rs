//! The full output of command items and the diffs of file changes, for
//! `item.detail`.
//!
//! The hub keeps only the tail of a command's output (see
//! `COMMAND_OUTPUT_TAIL_BYTES`) and no diff. The worker keeps the whole text
//! here, one file per item, in `<consort home>/cache/items/<session>/`:
//! `<item>.out` for command output and `<item>.diff` for a file change. The
//! app asks for it when the user opens an item in the task thread. The hub
//! then sends `item.detail`, and the worker uploads the file as a blob.
//!
//! A file stops growing at [`MAX_BYTES`]; the end of the file then says so.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use consort_proto::ids::{ItemId, SessionId};
use consort_proto::model::{ItemPayload, ItemStatus};
use consort_proto::session_host_link::SessionEvent;

/// The most bytes that the worker keeps for one item.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// The line at the end of a file that reached [`MAX_BYTES`].
pub const CUT_NOTE: &str =
    "\n[Consort kept the first 16 MB of this output. The rest is not stored.]\n";

/// The media type of command output.
pub const OUTPUT_MIME: &str = "text/plain";

/// The media type of a diff.
pub const DIFF_MIME: &str = "text/x-diff";

/// The item files of one worker.
#[derive(Debug, Clone)]
pub struct ItemDetails {
    root: PathBuf,
}

impl ItemDetails {
    /// The item files under `<consort home>/cache/items`.
    pub fn new(consort_home: &Path) -> Self {
        Self {
            root: consort_home.join("cache").join("items"),
        }
    }

    fn file(&self, session: SessionId, item: ItemId, ext: &str) -> PathBuf {
        self.root
            .join(session.to_string())
            .join(format!("{item}.{ext}"))
    }

    /// Keeps what an event from the harness says about an item's output or
    /// diff. It takes the diffs out of a file change item, because the hub
    /// does not store them. A failure to write is logged: the turn goes on.
    pub fn observe(&self, session: SessionId, event: &mut SessionEvent) {
        let result = match event {
            SessionEvent::OutputDelta { item_id, text } => {
                self.append(&self.file(session, *item_id, "out"), text)
            }
            SessionEvent::Item(item) => {
                let id = item.id;
                let finished = matches!(item.status, ItemStatus::Completed | ItemStatus::Failed);
                match &mut item.payload {
                    // An adapter that gives no deltas sends the output in the
                    // item. Without deltas the file is shorter than it.
                    ItemPayload::Command(command) if finished && !command.output.is_empty() => {
                        let path = self.file(session, id, "out");
                        let have = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        if have < command.output.len() as u64 {
                            self.replace(&path, &command.output)
                        } else {
                            Ok(())
                        }
                    }
                    ItemPayload::FileChange(change) => {
                        let mut text = String::new();
                        for file in &mut change.changes {
                            let Some(diff) = file.diff.take() else {
                                continue;
                            };
                            if !diff.starts_with("---") && !diff.starts_with("diff ") {
                                let from = file.from_path.as_deref().unwrap_or(&file.path);
                                text.push_str(&format!("--- a/{from}\n+++ b/{}\n", file.path));
                            }
                            text.push_str(&diff);
                            if !diff.ends_with('\n') {
                                text.push('\n');
                            }
                        }
                        if text.is_empty() {
                            Ok(())
                        } else {
                            self.replace(&self.file(session, id, "diff"), &text)
                        }
                    }
                    _ => Ok(()),
                }
            }
            _ => Ok(()),
        };
        if let Err(err) = result {
            tracing::warn!(session = %session, error = %err, "could not keep the full output of an item");
        }
    }

    /// The kept text of an item and its media type. A diff wins over output.
    pub fn read(&self, session: SessionId, item: ItemId) -> Option<(Vec<u8>, &'static str)> {
        for (ext, mime) in [("diff", DIFF_MIME), ("out", OUTPUT_MIME)] {
            if let Ok(bytes) = std::fs::read(self.file(session, item, ext)) {
                return Some((bytes, mime));
            }
        }
        None
    }

    /// Removes the files of a session.
    pub fn forget(&self, session: SessionId) {
        let _ = std::fs::remove_dir_all(self.root.join(session.to_string()));
    }

    fn append(&self, path: &Path, text: &str) -> std::io::Result<()> {
        ensure_parent(path)?;
        let have = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if have >= MAX_BYTES {
            return Ok(());
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let room = usize::try_from(MAX_BYTES - have).unwrap_or(usize::MAX);
        // Output that reaches the limit gets the note at once, so a later
        // delta finds a full file and writes nothing.
        if text.len() < room {
            file.write_all(text.as_bytes())
        } else {
            let cut = floor_char_boundary(text, room);
            file.write_all(&text.as_bytes()[..cut])?;
            file.write_all(CUT_NOTE.as_bytes())
        }
    }

    fn replace(&self, path: &Path, text: &str) -> std::io::Result<()> {
        ensure_parent(path)?;
        let limit = usize::try_from(MAX_BYTES).unwrap_or(usize::MAX);
        if text.len() <= limit {
            std::fs::write(path, text)
        } else {
            let cut = floor_char_boundary(text, limit);
            let mut out = text[..cut].to_owned();
            out.push_str(CUT_NOTE);
            std::fs::write(path, out)
        }
    }
}

fn ensure_parent(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) => std::fs::create_dir_all(parent),
        None => Ok(()),
    }
}

/// The largest index at or below `at` that starts a character.
fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use consort_proto::ids::TurnId;
    use consort_proto::model::{
        CommandPayload, FileChange, FileChangeKind, FileChangePayload, SessionItem,
    };
    use consort_proto::time::Timestamp;

    fn item(id: ItemId, payload: ItemPayload, status: ItemStatus) -> SessionEvent {
        SessionEvent::Item(SessionItem {
            id,
            session_id: SessionId::new(),
            turn_id: TurnId::new("t1"),
            payload,
            status,
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(2),
        })
    }

    #[test]
    fn deltas_make_the_whole_output_and_the_final_item_does_not_cut_it() {
        let dir = tempfile::tempdir().unwrap();
        let details = ItemDetails::new(dir.path());
        let (session, id) = (SessionId::new(), ItemId::new());
        for text in ["line 1\n", "line 2\n", "line 3\n"] {
            details.observe(
                session,
                &mut SessionEvent::OutputDelta {
                    item_id: id,
                    text: text.to_owned(),
                },
            );
        }
        // The item holds only the tail. The file keeps the whole output.
        let tail = CommandPayload {
            command: "make".into(),
            output: "line 3\n".into(),
            output_truncated: true,
            ..CommandPayload::default()
        };
        details.observe(
            session,
            &mut item(id, ItemPayload::Command(tail), ItemStatus::Completed),
        );
        let (bytes, mime) = details.read(session, id).unwrap();
        assert_eq!(mime, OUTPUT_MIME);
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "line 1\nline 2\nline 3\n"
        );
    }

    #[test]
    fn output_with_no_deltas_comes_from_the_finished_item() {
        let dir = tempfile::tempdir().unwrap();
        let details = ItemDetails::new(dir.path());
        let (session, id) = (SessionId::new(), ItemId::new());
        let command = CommandPayload {
            command: "ls".into(),
            output: "a\nb\n".into(),
            ..CommandPayload::default()
        };
        details.observe(
            session,
            &mut item(
                id,
                ItemPayload::Command(command.clone()),
                ItemStatus::InProgress,
            ),
        );
        assert!(
            details.read(session, id).is_none(),
            "nothing before the end"
        );
        details.observe(
            session,
            &mut item(id, ItemPayload::Command(command), ItemStatus::Completed),
        );
        assert_eq!(details.read(session, id).unwrap().0, b"a\nb\n");
        details.forget(session);
        assert!(details.read(session, id).is_none());
    }

    #[test]
    fn a_file_change_keeps_its_diff_and_the_hub_gets_none() {
        let dir = tempfile::tempdir().unwrap();
        let details = ItemDetails::new(dir.path());
        let (session, id) = (SessionId::new(), ItemId::new());
        let change = |path: &str, diff: Option<&str>| FileChange {
            path: path.into(),
            change: FileChangeKind::Modify,
            from_path: None,
            lines_added: 1,
            lines_removed: 1,
            diff: diff.map(str::to_owned),
        };
        let mut event = item(
            id,
            ItemPayload::FileChange(FileChangePayload {
                changes: vec![
                    change("src/a.rs", Some("@@ -1 +1 @@\n-a\n+b")),
                    change(
                        "src/b.rs",
                        Some("--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-c\n+d\n"),
                    ),
                    change("src/c.rs", None),
                ],
            }),
            ItemStatus::Completed,
        );
        details.observe(session, &mut event);
        let SessionEvent::Item(sent) = &event else {
            unreachable!()
        };
        let ItemPayload::FileChange(sent) = &sent.payload else {
            unreachable!()
        };
        assert!(sent.changes.iter().all(|c| c.diff.is_none()));
        let (bytes, mime) = details.read(session, id).unwrap();
        assert_eq!(mime, DIFF_MIME);
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-a\n+b\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-c\n+d\n"
        );
    }

    #[test]
    fn a_long_output_stops_at_the_limit_with_a_note() {
        let dir = tempfile::tempdir().unwrap();
        let details = ItemDetails::new(dir.path());
        let (session, id) = (SessionId::new(), ItemId::new());
        let chunk = "x".repeat(1024 * 1024);
        for _ in 0..17 {
            details.observe(
                session,
                &mut SessionEvent::OutputDelta {
                    item_id: id,
                    text: chunk.clone(),
                },
            );
        }
        let bytes = details.read(session, id).unwrap().0;
        assert_eq!(bytes.len() as u64, MAX_BYTES + CUT_NOTE.len() as u64);
        assert!(String::from_utf8(bytes).unwrap().ends_with(CUT_NOTE));
    }
}
