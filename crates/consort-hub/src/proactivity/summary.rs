//! The change summary of a check-in: what changed in the hub since the last
//! check-in of a channel, built from the hub's own state with no model.
//!
//! It lists tasks that changed status, approvals and decisions that wait,
//! failed routine runs, workers that went offline or came back (in Tutti),
//! new channel files, and messages from Tutti in a channel. Tutti's summary
//! covers every channel.

use std::collections::BTreeMap;

use consort_proto::ids::ChannelId;
use consort_proto::model::{
    Approval, ApprovalStatus, CHECK_IN_NOTE_MAX_BYTES, CHECK_IN_WATCH_OUTSIDE, Channel,
    ChannelFile, Decision, DecisionStatus, Worker, WorkerState,
};
use consort_proto::time::Timestamp;
use rusqlite::{Connection, params};

use crate::error::Result;
use crate::store::{conversation, list_docs, parse_doc, proactivity as store};

/// The most lines in one summary. The rest is counted in one line.
const SUMMARY_MAX_LINES: usize = 30;

/// Builds the summary of `channel` since `since`. An empty string means that
/// nothing changed.
pub(crate) fn change_summary(
    conn: &Connection,
    channel: &Channel,
    since: Timestamp,
) -> Result<String> {
    let scope = (!channel.is_tutti()).then_some(channel.id);
    let names: BTreeMap<ChannelId, String> = conversation::channels(conn)?
        .into_iter()
        .map(|c| (c.id, c.name))
        .collect();
    let place = |id: ChannelId| -> String {
        if scope.is_some() {
            String::new()
        } else {
            names
                .get(&id)
                .map(|n| format!(" in {n}"))
                .unwrap_or_default()
        }
    };
    let mut lines: Vec<String> = Vec::new();

    for (task, _) in store::status_changes_since(conn, scope, since)? {
        lines.push(format!(
            "- Task \"{}\"{} is now {}.",
            task.title,
            place(task.channel_id),
            task.status.label()
        ));
    }

    let approvals: Vec<Approval> = list_docs(
        conn,
        "SELECT data FROM approvals WHERE status = 'pending' AND (?1 IS NULL OR channel_id = ?1)",
        params![scope.map(|c| c.to_string())],
    )?;
    for approval in approvals
        .iter()
        .filter(|a| a.status == ApprovalStatus::Pending && a.created_at > since)
    {
        lines.push(format!(
            "- An approval waits{}: {}",
            place(approval.channel_id),
            one_line(&approval.summary)
        ));
    }
    let decisions: Vec<Decision> = list_docs(
        conn,
        "SELECT data FROM decisions WHERE status = 'pending' AND (?1 IS NULL OR channel_id = ?1)",
        params![scope.map(|c| c.to_string())],
    )?;
    for decision in decisions
        .iter()
        .filter(|d| d.status == DecisionStatus::Pending && d.created_at > since)
    {
        lines.push(format!(
            "- A decision waits{}: {}",
            place(decision.channel_id),
            one_line(&decision.question)
        ));
    }

    let mut failed = conn.prepare_cached(
        "SELECT r.data, rr.error FROM routine_runs rr JOIN routines r ON r.id = rr.routine_id
         WHERE rr.status = 'error' AND rr.ended_at > ?1 AND (?2 IS NULL OR rr.channel_id = ?2)
         ORDER BY rr.id",
    )?;
    let rows = failed.query_map(
        params![since.as_millis(), scope.map(|c| c.to_string())],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    )?;
    for row in rows {
        let (data, error) = row?;
        let routine: consort_proto::model::Routine = parse_doc(&data)?;
        lines.push(format!(
            "- The routine \"{}\"{} failed: {}",
            routine.name,
            place(routine.channel_id),
            one_line(error.as_deref().unwrap_or("no reason given"))
        ));
    }

    if scope.is_none() {
        for worker in latest_feed::<Worker>(conn, "worker", None, since)? {
            if worker.revoked_at.is_some() {
                continue;
            }
            let state = match worker.state {
                WorkerState::Online => "came back online",
                WorkerState::Offline => "went offline",
            };
            lines.push(format!("- The worker {} {state}.", worker.name));
        }
    }

    for file in latest_feed::<ChannelFile>(conn, "channel_file", scope, since)? {
        if file.deleted_at.is_some() || file.updated_at <= since {
            continue;
        }
        lines.push(format!(
            "- New file{}: {}",
            place(file.channel_id),
            file.path
        ));
    }

    if let Some(channel_id) = scope {
        let mut from_tutti = conn.prepare_cached(
            "SELECT m.data FROM messages m JOIN sessions s
                ON s.id = json_extract(m.data, '$.author_session_id')
             WHERE m.channel_id = ?1 AND m.created_at > ?2 AND m.deleted_at IS NULL
               AND s.channel_id <> ?1
             ORDER BY m.created_at",
        )?;
        let rows = from_tutti
            .query_map(params![channel_id.to_string(), since.as_millis()], |row| {
                row.get::<_, String>(0)
            })?;
        for row in rows {
            let message: consort_proto::model::Message = parse_doc(&row?)?;
            lines.push(format!("- Tutti wrote here: {}", one_line(&message.body)));
        }
    }

    if lines.len() > SUMMARY_MAX_LINES {
        let more = lines.len() - SUMMARY_MAX_LINES;
        lines.truncate(SUMMARY_MAX_LINES);
        lines.push(format!("- And {more} more changes."));
    }
    Ok(lines.join("\n"))
}

/// The newest version of each entity of one kind in the feed since a time.
fn latest_feed<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    entity: &str,
    channel_id: Option<ChannelId>,
    since: Timestamp,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare_cached(
        "SELECT entity_id, payload FROM feed
         WHERE entity = ?1 AND op = 'upsert' AND at > ?2 AND (?3 IS NULL OR channel_id = ?3)
         ORDER BY seq",
    )?;
    let rows = stmt.query_map(
        params![entity, since.as_millis(), channel_id.map(|c| c.to_string())],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    )?;
    let mut newest: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for row in rows {
        let (id, payload) = row?;
        let Some(payload) = payload else { continue };
        if !newest.contains_key(&id) {
            order.push(id.clone());
        }
        newest.insert(id, payload);
    }
    let mut out = Vec::new();
    for id in order {
        if let Some(payload) = newest.get(&id) {
            out.push(parse_doc(payload)?);
        }
    }
    Ok(out)
}

/// The first line of a text, cut at 160 characters.
fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > 160 {
        let cut: String = line.chars().take(159).collect();
        format!("{cut}…")
    } else {
        line.to_owned()
    }
}

/// The check-in note for a check-in, cut at 2 KiB with a line that says so.
pub(crate) fn note_for_check_in(content: &str) -> String {
    if content.len() <= CHECK_IN_NOTE_MAX_BYTES {
        return content.to_owned();
    }
    let mut end = CHECK_IN_NOTE_MAX_BYTES;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[The hub cut this note at 2 KiB. Make check-in.md shorter.]",
        &content[..end]
    )
}

/// Whether a check-in note asks to watch an outside source: its front
/// matter has the line `watch: outside`.
pub(crate) fn watches_outside(content: &str) -> bool {
    let mut lines = content.lines();
    let first = lines.next().map(str::trim);
    if first == Some(CHECK_IN_WATCH_OUTSIDE) {
        return true;
    }
    if first != Some("---") {
        return false;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            return false;
        }
        if line == CHECK_IN_WATCH_OUTSIDE {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_watches_outside_only_with_the_front_matter_line() {
        assert!(watches_outside("watch: outside\nWatch mail."));
        assert!(watches_outside("---\nwatch: outside\n---\nWatch mail."));
        assert!(!watches_outside("Watch mail.\nwatch: outside"));
        assert!(!watches_outside("---\ntitle: x\n---\nwatch: outside"));
        assert!(!watches_outside(""));
    }

    #[test]
    fn a_long_note_is_cut_at_two_kib_with_a_line() {
        let short = "Skip receipts.";
        assert_eq!(note_for_check_in(short), short);
        let long = "é".repeat(2_000);
        let cut = note_for_check_in(&long);
        assert!(cut.ends_with("[The hub cut this note at 2 KiB. Make check-in.md shorter.]"));
        assert!(cut.len() < long.len());
    }

    #[test]
    fn one_line_keeps_the_first_line_and_cuts_long_text() {
        assert_eq!(one_line("\n first \nsecond"), "first");
        assert_eq!(one_line(&"a".repeat(200)).chars().count(), 160);
    }
}
