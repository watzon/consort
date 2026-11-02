//! The ways to reach the hub that are on. `consort hub reach` writes the same
//! table from another process, so the hub reads it each time and keeps no copy.

use consort_proto::client_api::ReachMethod;
use consort_proto::time::Timestamp;
use rusqlite::{Connection, params};

use crate::error::Result;

/// The meta key with the URL that a Serve entry forwards to. The hub writes it
/// at each start, and `consort hub reach` reads it.
pub const TARGET_KEY: &str = "reach.hub_target";

/// One way to reach the hub that is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachRow {
    pub method: ReachMethod,
    /// The URL that peers use.
    pub address: String,
    /// The Serve port or the TLS port.
    pub port: u16,
    /// What Serve forwards to, for Tailscale.
    pub target: Option<String>,
    pub turned_on_at: Timestamp,
}

/// Every way that is on. A row with a method that this build does not know
/// is left out.
pub fn rows(conn: &Connection) -> Result<Vec<ReachRow>> {
    let mut statement = conn.prepare(
        "SELECT method, address, port, target, turned_on_at FROM hub_reach ORDER BY method",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut found = Vec::new();
    for row in rows {
        let (method, address, port, target, at) = row?;
        let (Some(method), Ok(port)) = (ReachMethod::from_name(&method), u16::try_from(port))
        else {
            continue;
        };
        found.push(ReachRow {
            method,
            address,
            port,
            target,
            turned_on_at: Timestamp::from_millis(at),
        });
    }
    Ok(found)
}

/// The row of one way, when it is on.
pub fn row(conn: &Connection, method: ReachMethod) -> Result<Option<ReachRow>> {
    Ok(rows(conn)?.into_iter().find(|r| r.method == method))
}

/// Writes the row of one way.
pub fn save(conn: &Connection, row: &ReachRow) -> Result<()> {
    conn.execute(
        "INSERT INTO hub_reach (method, address, port, target, turned_on_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(method) DO UPDATE SET
           address = excluded.address,
           port = excluded.port,
           target = excluded.target,
           turned_on_at = excluded.turned_on_at",
        params![
            row.method.as_str(),
            row.address,
            i64::from(row.port),
            row.target,
            row.turned_on_at.as_millis(),
        ],
    )?;
    Ok(())
}

/// Removes the row of one way. Its address leaves the list of origins.
pub fn remove(conn: &Connection, method: ReachMethod) -> Result<()> {
    conn.execute("DELETE FROM hub_reach WHERE method = ?1", [method.as_str()])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_saved_changed_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::store::open_and_migrate(dir.path()).unwrap();
        assert!(rows(&conn).unwrap().is_empty());
        let mut saved = ReachRow {
            method: ReachMethod::Tailscale,
            address: "https://mac.tail.ts.net:8443".to_owned(),
            port: 8443,
            target: Some("http://127.0.0.1:7600".to_owned()),
            turned_on_at: Timestamp::from_millis(5),
        };
        save(&conn, &saved).unwrap();
        assert_eq!(rows(&conn).unwrap(), vec![saved.clone()]);
        saved.port = 443;
        save(&conn, &saved).unwrap();
        assert_eq!(row(&conn, ReachMethod::Tailscale).unwrap(), Some(saved));
        assert_eq!(row(&conn, ReachMethod::ThisNetwork).unwrap(), None);
        remove(&conn, ReachMethod::Tailscale).unwrap();
        assert!(rows(&conn).unwrap().is_empty());
    }
}
