//! The SQLite store.
//!
//! One writer thread owns the only writing connection. [`Store::write`] runs
//! a closure inside an `IMMEDIATE` transaction on that thread. Every closure
//! that changes an entity must also call [`Tx::emit`] (or the `save_*`
//! methods, which do), so the feed entry lands in the same transaction as the
//! change. After the commit, the writer broadcasts the new feed entries.
//!
//! Readers are plain read-only connections from a small pool. [`Store::read`]
//! runs a closure on one of them in a blocking task. At most
//! `READER_POOL_SIZE` reads run at the same time; more reads wait for a free
//! connection. WAL mode lets readers and the writer run at the same time.
//!
//! Naming convention in the entity modules:
//!
//! - A free function that takes `&Connection` reads. It works inside
//!   [`Store::read`] and inside [`Store::write`] (through [`Tx::db`]).
//! - A method on [`Tx`] writes. `save_*` methods upsert the row and emit the
//!   feed entry. `delete_*` and `purge_*` methods remove rows and emit the
//!   feed deletes.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use consort_proto::client_api::{EntityKind, EntityPayload, FeedEntry};
use consort_proto::ids::ChannelId;
use consort_proto::time::Timestamp;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{broadcast, oneshot};

use crate::error::{HubError, Result};

pub mod conversation;
pub mod coordinator;
pub mod feed;
pub mod knowledge;
pub mod machine;
pub mod migrations;
pub mod routines;
pub mod work;

const READER_POOL_SIZE: usize = 8;
const FEED_BROADCAST_CAPACITY: usize = 4096;

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    jobs: mpsc::Sender<Job>,
    readers: Mutex<Vec<Connection>>,
    /// One permit for each reader connection, so the pool never grows.
    read_slots: Arc<tokio::sync::Semaphore>,
    db_path: PathBuf,
    blobs_dir: PathBuf,
    feed_tx: broadcast::Sender<Arc<FeedEntry>>,
}

/// Opens a connection with the pragmas that the hub needs.
pub fn open_connection(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
        row.get::<_, String>(0)
    })?;
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

/// Creates a folder and its parents. On Unix the folder gets mode 0700, also
/// when it exists already, so only the user can read what the hub keeps in
/// it. Parents that exist keep their mode.
pub(crate) fn create_private_dir(path: &Path) -> Result<()> {
    let context = |err: std::io::Error| {
        HubError::Startup(format!(
            "could not make the folder {} private: {err}. Check that you own it.",
            path.display()
        ))
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(context)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(context)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path).map_err(context)
    }
}

/// Gives the database file mode 0600 on Unix before SQLite opens it, or
/// creates it with that mode. SQLite gives new `-wal` and `-shm` files the
/// mode of the database file. Old ones get the mode here too.
fn make_database_private(db_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let context = |path: &Path, err: std::io::Error| {
            HubError::Startup(format!(
                "could not make {} private: {err}. Check that you own it.",
                path.display()
            ))
        };
        if db_path.exists() {
            std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| context(db_path, e))?;
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(db_path)
                .map_err(|e| context(db_path, e))?;
        }
        for suffix in ["-wal", "-shm"] {
            let mut name = db_path.as_os_str().to_owned();
            name.push(suffix);
            let path = PathBuf::from(name);
            if path.exists() {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| context(&path, e))?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = db_path;
    Ok(())
}

/// Opens the database, applies migrations and returns the connection. The
/// `create_setup_code` function uses this to work while a hub runs.
///
/// On Unix the data folder gets mode 0700 and `hub.db` mode 0600, also when
/// they exist already.
pub fn open_and_migrate(data_dir: &Path) -> Result<Connection> {
    create_private_dir(data_dir)?;
    let db_path = data_dir.join("hub.db");
    make_database_private(&db_path)?;
    let mut conn = open_connection(&db_path)?;
    migrations::migrate(&mut conn)?;
    Ok(conn)
}

impl Store {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let conn = open_and_migrate(data_dir)?;
        let blobs_dir = data_dir.join("blobs");
        create_private_dir(&blobs_dir)?;
        let (jobs, receiver) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("hub-db-writer".to_owned())
            .spawn(move || {
                let mut conn = conn;
                while let Ok(job) = receiver.recv() {
                    job(&mut conn);
                }
            })?;
        let (feed_tx, _) = broadcast::channel(FEED_BROADCAST_CAPACITY);
        Ok(Self {
            inner: Arc::new(Inner {
                jobs,
                readers: Mutex::new(Vec::new()),
                read_slots: Arc::new(tokio::sync::Semaphore::new(READER_POOL_SIZE)),
                db_path: data_dir.join("hub.db"),
                blobs_dir,
                feed_tx,
            }),
        })
    }

    /// Runs `f` in a transaction on the writer thread.
    pub async fn write<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Tx<'_>) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (reply, receiver) = oneshot::channel();
        let feed_tx = self.inner.feed_tx.clone();
        let job: Job = Box::new(move |conn| {
            let result = run_write(conn, f).map(|(value, entries)| {
                for entry in entries {
                    // No receiver is fine: no client is connected.
                    let _ = feed_tx.send(Arc::new(entry));
                }
                value
            });
            let _ = reply.send(result);
        });
        self.inner
            .jobs
            .send(job)
            .map_err(|_| HubError::internal("the database writer stopped"))?;
        receiver
            .await
            .map_err(|_| HubError::internal("the database writer stopped"))?
    }

    /// Runs `f` on a read-only connection in a blocking task. It waits while
    /// every reader connection is in use.
    pub async fn read<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let slot = Arc::clone(&self.inner.read_slots)
            .acquire_owned()
            .await
            .map_err(|_| HubError::internal("the reader pool closed"))?;
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            let conn = inner.take_reader()?;
            let result = f(&conn);
            inner.put_reader(conn);
            result
        })
        .await?
    }

    /// Feed entries, in `seq` order, after each commit.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<FeedEntry>> {
        self.inner.feed_tx.subscribe()
    }

    pub fn blobs_dir(&self) -> &Path {
        &self.inner.blobs_dir
    }
}

impl Inner {
    fn take_reader(&self) -> Result<Connection> {
        let pooled = self
            .readers
            .lock()
            .map_err(|_| HubError::internal("reader pool poisoned"))?
            .pop();
        match pooled {
            Some(conn) => Ok(conn),
            None => {
                let conn = open_connection(&self.db_path)?;
                conn.execute_batch("PRAGMA query_only = ON;")?;
                Ok(conn)
            }
        }
    }

    fn put_reader(&self, conn: Connection) {
        if let Ok(mut pool) = self.readers.lock()
            && pool.len() < READER_POOL_SIZE
        {
            pool.push(conn);
        }
    }
}

/// Runs `f` in a write transaction on a connection that this process opened
/// itself, for a command on the hub machine that runs beside a hub. The
/// feed entries land in the database, so clients get them when they catch
/// up. A running hub does not send them live. It reads the change in its
/// maintenance loop and sends the entity again.
pub(crate) fn write_direct<T>(
    conn: &mut Connection,
    f: impl FnOnce(&mut Tx<'_>) -> Result<T>,
) -> Result<T> {
    run_write(conn, f).map(|(value, _)| value)
}

fn run_write<T>(
    conn: &mut Connection,
    f: impl FnOnce(&mut Tx<'_>) -> Result<T>,
) -> Result<(T, Vec<FeedEntry>)> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut wtx = Tx {
        tx,
        now: Timestamp::now(),
        feed: Vec::new(),
    };
    // An error drops the transaction, which rolls it back.
    let value = f(&mut wtx)?;
    let Tx { tx, feed, .. } = wtx;
    tx.commit()?;
    Ok((value, feed))
}

/// A write transaction.
pub struct Tx<'a> {
    tx: rusqlite::Transaction<'a>,
    now: Timestamp,
    feed: Vec<FeedEntry>,
}

impl Tx<'_> {
    pub fn db(&self) -> &Connection {
        &self.tx
    }

    /// The time of this transaction. Use it for every timestamp that the
    /// transaction writes, so related rows agree.
    pub fn now(&self) -> Timestamp {
        self.now
    }

    /// Adds an upsert feed entry for `payload`.
    pub fn emit(&mut self, payload: EntityPayload) -> Result<()> {
        let entry = FeedEntry::upsert(0, self.now, payload);
        self.insert_feed(entry)
    }

    /// Like [`Tx::emit`], with an explicit channel for clients that filter by
    /// channel. Session items use it.
    pub fn emit_in(&mut self, payload: EntityPayload, channel_id: ChannelId) -> Result<()> {
        let mut entry = FeedEntry::upsert(0, self.now, payload);
        entry.channel_id = Some(channel_id);
        self.insert_feed(entry)
    }

    /// Adds a delete feed entry.
    pub fn emit_delete(
        &mut self,
        kind: EntityKind,
        entity_id: impl Into<String>,
        channel_id: Option<ChannelId>,
    ) -> Result<()> {
        let entry = FeedEntry::delete(0, self.now, kind, entity_id, channel_id);
        self.insert_feed(entry)
    }

    fn insert_feed(&mut self, mut entry: FeedEntry) -> Result<()> {
        let value = serde_json::to_value(&entry)?;
        let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        let payload = match value.get("payload") {
            Some(serde_json::Value::Null) | None => None,
            Some(other) => Some(serde_json::to_string(other)?),
        };
        self.tx.execute(
            "INSERT INTO feed (at, channel_id, entity, entity_id, op, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                entry.at.as_millis(),
                entry.channel_id.map(|c| c.to_string()),
                text("entity"),
                entry.entity_id,
                text("op"),
                payload,
            ],
        )?;
        entry.seq = u64::try_from(self.tx.last_insert_rowid())
            .map_err(|_| HubError::internal("feed sequence out of range"))?;
        self.feed.push(entry);
        Ok(())
    }
}

// Document helpers -------------------------------------------------------

/// Parses one JSON `data` column.
pub(crate) fn parse_doc<T: DeserializeOwned>(text: &str) -> Result<T> {
    Ok(serde_json::from_str(text)?)
}

pub(crate) fn to_doc<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

/// Reads the `data` column of the first row, or `None`.
pub(crate) fn get_doc<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Option<T>> {
    let text: Option<String> = conn.query_row(sql, params, |row| row.get(0)).optional()?;
    text.as_deref().map(parse_doc).transpose()
}

/// Reads the `data` column of every row.
pub(crate) fn list_docs<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params, |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(parse_doc(&row?)?);
    }
    Ok(out)
}

/// A meta value.
pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

impl Tx<'_> {
    pub fn meta_set(&mut self, key: &str, value: &str) -> Result<()> {
        self.tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A store in a temp dir. Keep the guard alive for the test.
    pub fn temp_store() -> (Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (store, dir)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::temp_store;
    use super::*;
    use consort_proto::ids::GroupId;
    use consort_proto::model::Group;

    #[tokio::test]
    async fn a_write_adds_a_feed_entry_and_broadcasts_after_commit() {
        let (store, _dir) = temp_store();
        let mut feed = store.subscribe();
        let group = Group {
            id: GroupId::new(),
            name: "Home".into(),
            position: 0,
            collapsed: false,
        };
        let id = group.id;
        store.write(move |tx| tx.save_group(&group)).await.unwrap();
        let entry = feed.recv().await.unwrap();
        assert_eq!(entry.entity_id, id.to_string());
        let head = store.read(feed::head_seq).await.unwrap();
        assert_eq!(head, entry.seq);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn at_most_the_pool_size_of_reads_run_at_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (store, _dir) = temp_store();
        let running = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..READER_POOL_SIZE * 4 {
            let (store, running, most) = (store.clone(), running.clone(), most.clone());
            tasks.push(tokio::spawn(async move {
                store
                    .read(move |_| {
                        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        running.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .await
            }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        let most = most.load(Ordering::SeqCst);
        assert!(most <= READER_POOL_SIZE, "{most} reads ran at once");
        assert!(store.inner.readers.lock().unwrap().len() <= READER_POOL_SIZE);
    }

    #[tokio::test]
    async fn a_failed_write_rolls_back_and_adds_no_feed_entry() {
        let (store, _dir) = temp_store();
        let group = Group {
            id: GroupId::new(),
            name: "Home".into(),
            position: 0,
            collapsed: false,
        };
        let result: Result<()> = store
            .write(move |tx| {
                tx.save_group(&group)?;
                Err(HubError::conflict("stop"))
            })
            .await;
        assert!(result.is_err());
        let head = store.read(feed::head_seq).await.unwrap();
        let groups = store.read(conversation::groups).await.unwrap();
        assert_eq!(head, 0);
        assert!(groups.is_empty());
    }
}
