//! Versioned migrations. The database stores the applied version in
//! `PRAGMA user_version`. Each migration runs in its own transaction.
//!
//! Add a new migration to the end of [`MIGRATIONS`]. Never edit an old one.

use rusqlite::Connection;

use crate::error::{HubError, Result};

pub const MIGRATIONS: &[&str] = &[include_str!("schema_v1.sql")];

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

    #[test]
    fn a_newer_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 999").unwrap();
        assert!(matches!(migrate(&mut conn), Err(HubError::Startup(_))));
    }
}
