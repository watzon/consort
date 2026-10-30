//! Versioned migrations. The database stores the applied version in
//! `PRAGMA user_version`. Each migration runs in its own transaction.
//!
//! Add a new migration to the end of [`MIGRATIONS`]. Never edit an old one.

use rusqlite::Connection;

use crate::error::{HubError, Result};

pub const MIGRATIONS: &[&str] = &[
    include_str!("schema_v1.sql"),
    include_str!("schema_v2.sql"),
    include_str!("schema_v3.sql"),
    include_str!("schema_v4.sql"),
    include_str!("schema_v5.sql"),
    include_str!("schema_v6.sql"),
    include_str!("schema_v7.sql"),
    include_str!("schema_v8.sql"),
    include_str!("schema_v9.sql"),
    include_str!("schema_v10.sql"),
];

pub fn current_version() -> i64 {
    // The list is a constant with a handful of entries.
    i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX)
}

pub fn migrate(conn: &mut Connection) -> Result<()> {
    let applied: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if applied > current_version() {
        return Err(HubError::Startup(format!(
            "hub.db has schema version {applied}, but this hub knows version {}. \
             Start a newer hub.",
            current_version()
        )));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = i64::try_from(index + 1).unwrap_or(i64::MAX);
        if version <= applied {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {version}"))?;
        tx.commit()?;
        tracing::info!(version, "applied database migration");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_once_and_are_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, current_version());
        conn.execute(
            "INSERT INTO search_idx (kind, ref, channel_id, scope, title, body) VALUES ('m','1','c','','t','hello world')",
            [],
        )
        .unwrap();
        let hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM search_idx WHERE search_idx MATCH 'hello'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1);
    }

    /// Opens a database at schema version 5, the last one before Tutti, so a
    /// test can fill it with rows from the old hub.
    fn database_at_version_5() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 5").unwrap();
        conn
    }

    /// The JSON of a main channel as the old hub stored it.
    fn old_main_channel_json() -> String {
        use consort_proto::ids::ChannelId;
        use consort_proto::model::{ApprovalMode, Channel, ChannelKind, Isolation};
        use consort_proto::time::Timestamp;
        let now = Timestamp::now();
        let channel = Channel {
            id: ChannelId::new(),
            kind: ChannelKind::Tutti,
            name: "Tutti".to_owned(),
            icon: None,
            color: None,
            group_id: None,
            position: 0,
            purpose: String::new(),
            instructions: String::new(),
            coordinator_selection: None,
            task_selection: None,
            preferred_workers: Vec::new(),
            repo_ids: Vec::new(),
            default_isolation: Isolation::Worktree,
            default_approval_mode: ApprovalMode::Ask,
            allowed_local_mcp_servers: Vec::new(),
            memory_enabled: true,
            tags: Vec::new(),
            disabled_connections: Vec::new(),
            disabled_skills: Vec::new(),
            archived_at: None,
            created_at: now,
            updated_at: now,
            proactive_tasks: Default::default(),
            proactivity: None,
        };
        let json = serde_json::to_string(&channel).unwrap();
        let old = json
            .replace("\"kind\":\"tutti\"", "\"kind\":\"everything\"")
            .replace("\"name\":\"Tutti\"", "\"name\":\"Everything\"");
        assert!(old.contains("\"kind\":\"everything\""), "{old}");
        old
    }

    #[test]
    fn version_6_rewrites_everything_rows_to_tutti_and_they_read_as_tutti() {
        use consort_proto::model::{Channel, ChannelKind};
        use consort_proto::turn_input::WakeEvent;

        let mut conn = database_at_version_5();
        let channel_json = old_main_channel_json();
        let channel: serde_json::Value = serde_json::from_str(&channel_json).unwrap();
        let id = channel["id"].as_str().unwrap().to_owned();
        conn.execute(
            "INSERT INTO channels (id, kind, position, data) VALUES (?1, 'everything', 0, ?2)",
            rusqlite::params![id, channel_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feed (at, channel_id, entity, entity_id, op, payload)
             VALUES (1, NULL, 'channel', ?1, 'upsert', ?2)",
            rusqlite::params![id, channel_json],
        )
        .unwrap();
        let old_event = r#"{"type":"everything_message","id":"msg_00000000-0000-0000-0000-000000000001","at":"2026-10-28T21:49:00Z","text":"Check \"type\":\"everything_message\" now."}"#;
        conn.execute(
            "INSERT INTO wake_events (channel_id, event, created_at) VALUES (?1, ?2, 1)",
            rusqlite::params![id, old_event],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO coordinators (channel_id, turn_open, turn_events) VALUES (?1, 1, ?2)",
            rusqlite::params![id, format!("[{old_event}]")],
        )
        .unwrap();

        migrate(&mut conn).unwrap();

        let (kind, data): (String, String) = conn
            .query_row("SELECT kind, data FROM channels", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(kind, "tutti");
        let channel: Channel = serde_json::from_str(&data).unwrap();
        assert_eq!(channel.kind, ChannelKind::Tutti);
        assert_eq!(channel.name, "Tutti");
        assert!(!data.contains("everything") && !data.contains("Everything"));
        let read = crate::store::conversation::tutti_channel(&conn).unwrap();
        assert_eq!(read.kind, ChannelKind::Tutti);

        let payload: String = conn
            .query_row("SELECT payload FROM feed", [], |r| r.get(0))
            .unwrap();
        let from_feed: Channel = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            (from_feed.kind, from_feed.name.as_str()),
            (ChannelKind::Tutti, "Tutti")
        );
        assert!(!payload.contains("everything"));

        let event: String = conn
            .query_row("SELECT event FROM wake_events", [], |r| r.get(0))
            .unwrap();
        assert!(event.contains("\"type\":\"tutti_message\""), "{event}");
        assert!(matches!(
            serde_json::from_str::<WakeEvent>(&event).unwrap(),
            WakeEvent::TuttiMessage { .. }
        ));
        let turn: String = conn
            .query_row("SELECT turn_events FROM coordinators", [], |r| r.get(0))
            .unwrap();
        let events: Vec<WakeEvent> = serde_json::from_str(&turn).unwrap();
        match &events[0] {
            WakeEvent::TuttiMessage { text, .. } => {
                // Text that looks like an old event stays as the user wrote it.
                assert_eq!(text, "Check \"type\":\"everything_message\" now.");
            }
            other => panic!("not a Tutti message: {other:?}"),
        }

        // The unique index now guards `tutti`. A second main channel fails.
        let second = conn.execute(
            "INSERT INTO channels (id, kind, position, data) VALUES ('chn_x', 'tutti', 1, '{}')",
            [],
        );
        assert!(second.is_err());
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, current_version());
    }

    #[test]
    fn version_7_keeps_waiting_inbox_messages_with_no_attachments() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..6] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 6").unwrap();
        conn.execute(
            "INSERT INTO task_inbox (task_id, text, created_at) VALUES ('tsk_1', 'Hi', 1)",
            [],
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let attachments: String = conn
            .query_row("SELECT attachments FROM task_inbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(attachments, "[]");
    }

    #[test]
    fn version_8_ends_old_routine_runs_and_keeps_memory_notes() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..7] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 7").unwrap();
        conn.execute(
            "INSERT INTO routines (id, channel_id, enabled, next_run_at, data) VALUES ('rtn_1', 'chn_1', 1, NULL, '{}')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO routine_runs (routine_id, started_at, error) VALUES ('rtn_1', 5, NULL);
             INSERT INTO routine_runs (routine_id, started_at, error) VALUES ('rtn_1', 9, 'It broke.');
             INSERT INTO memory_notes (scope, path, content, sha256, version, updated_by, updated_at)
                 VALUES ('global', 'a.md', 'A', 'x', 1, 'user', 1);",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let mut stmt = conn
            .prepare("SELECT status, ended_at, channel_id, manual FROM routine_runs ORDER BY id")
            .unwrap();
        let rows: Vec<(String, Option<i64>, String, bool)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![
                ("ok".to_owned(), Some(5), "chn_1".to_owned(), false),
                ("error".to_owned(), Some(9), "chn_1".to_owned(), false),
            ]
        );
        let (deleted, suggested): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT deleted_at, suggested_by FROM memory_notes",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((deleted, suggested), (None, None));
    }

    #[test]
    fn a_newer_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 999").unwrap();
        assert!(matches!(migrate(&mut conn), Err(HubError::Startup(_))));
    }
}
