//! The saved cache: one SQLite file in the client's folder that holds a copy
//! of the [`Cache`], so the app opens with its data while the hub is offline.
//!
//! The file has two tables:
//! - `entity(kind, id, json)`: one row for each entity that the cache keeps.
//!   `kind` names the kind (`message`, `task`, ...), `id` is the key that the
//!   cache uses for it, and `json` is the entity as the hub sends it.
//! - `meta(key, value)`: the feed position (`seq`) and the hub epoch that it
//!   belongs to (`hub_epoch`), the time when the data was last known to be
//!   current (`fresh_at`), the hub that the data came from, and the sets
//!   that say what the cache has loaded.
//!
//! `PRAGMA user_version` holds [`SCHEMA_VERSION`].
//!
//! A save is one transaction. It writes the rows that changed since the last
//! save and removes the rows that are gone, so a crash leaves the old data or
//! the new data and never half of it. The cache is saved only when its first
//! load is done (`Cache::loaded`), so the file never holds a half load.
//!
//! The API is narrow: [`open_or_recover`] at start, [`CacheFile::write`]
//! after a change, and [`delete`] when the user unpairs the hub. Nothing here
//! stops the app. A file that cannot be used is moved aside, and the client
//! starts with no saved data.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
};

use consort_proto::model::{
    Approval, Channel, ChannelFile, Client, Connection, Decision, Group, MemoryNote, Message,
    Preferences, ProactiveWake, ReadMarker, Repo, Routine, RoutineRun, Skill, Task, Worker,
};
use rusqlite::{OptionalExtension as _, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    activity::SessionActivity,
    cache::{Cache, CachedNote, ItemSummary},
    store::Identity,
};

/// The name of the file in the client's folder.
pub const FILE_NAME: &str = "cache.sqlite3";

/// The version of the tables and of the JSON in them. A file with another
/// version is moved aside. Raise it when a change would make an old file
/// read wrong.
pub const SCHEMA_VERSION: i64 = 1;

/// The ends that a file name can have besides the file itself: the journal
/// of an open transaction, the files of write-ahead logging, and the copy of
/// a file that could not be used.
const COMPANIONS: [&str; 4] = ["-journal", "-wal", "-shm", ".broken"];

/// Why the saved cache could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("the cache file cannot be used: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("a row of the cache file cannot be read: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the cache file cannot be reached: {0}")]
    Io(#[from] std::io::Error),
    #[error("the cache file has the schema version {found}, and this app reads {SCHEMA_VERSION}")]
    Version { found: i64 },
    #[error("the cache file failed its integrity check: {0}")]
    Damaged(String),
}

/// Who the saved data belongs to: the client and the hub. Data from another
/// client or another hub is not used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub client_id: String,
    pub hub_url: String,
}

impl Owner {
    pub fn of(identity: &Identity) -> Owner {
        Owner {
            client_id: identity.config.client_id.to_string(),
            hub_url: identity.config.hub_url.clone(),
        }
    }
}

/// What a file held when it was read.
#[derive(Debug)]
pub struct Saved {
    pub cache: Cache,
    /// When the data was last known to be current, in Unix milliseconds. The
    /// app says "You see saved data from" this time.
    pub fresh_at: i64,
}

/// One row of the `entity` table.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    kind: &'static str,
    id: String,
    json: String,
}

/// Everything that one save writes: the entity rows and the meta values.
#[derive(Debug)]
pub struct Rows {
    entities: Vec<Row>,
    meta: BTreeMap<&'static str, String>,
}

impl Rows {
    /// The number of entity rows.
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }
}

// The kinds of row. The names are part of the file format.
const GROUP: &str = "group";
const CHANNEL: &str = "channel";
const REPO: &str = "repo";
const WORKER: &str = "worker";
const MESSAGE: &str = "message";
const CHANNEL_FILE: &str = "channel_file";
const ROUTINE: &str = "routine";
const ROUTINE_RUN: &str = "routine_run";
const MEMORY_NOTE: &str = "memory_note";
const TASK: &str = "task";
const ARCHIVED_TASK: &str = "archived_task";
const APPROVAL: &str = "approval";
const DECISION: &str = "decision";
const READ_MARKER: &str = "read_marker";
const SESSION_ITEM: &str = "session_item";
const SESSION_ACTIVITY: &str = "session_activity";
const CONNECTION: &str = "connection";
const SKILL: &str = "skill";
const CLIENT: &str = "client";
const PROACTIVE_WAKE: &str = "proactive_wake";

// The keys of the meta table.
const SEQ: &str = "seq";
const HUB_EPOCH: &str = "hub_epoch";
const FRESH_AT: &str = "fresh_at";
const CLIENT_ID: &str = "client_id";
const HUB_URL: &str = "hub_url";
const FILES_LOADED: &str = "files_loaded";
const RUNS_LOADED: &str = "runs_loaded";
const MEMORY_LOADED: &str = "memory_loaded";
const MEMORY_DELETED_LOADED: &str = "memory_deleted_loaded";
const TASK_PREVIEWS: &str = "task_previews";
const TASKS_HAVE_MORE: &str = "tasks_have_more";
const TASKS_ABSENT: &str = "tasks_absent";
const ARCHIVED_HAS_MORE: &str = "archived_has_more";
const HAS_OLDER: &str = "has_older";
const CHANNELS_LOADED: &str = "channels_loaded";
const LOADED_THREADS: &str = "loaded_threads";
const PREFERENCES: &str = "preferences";
const PROACTIVITY_SETTINGS: &str = "proactivity_settings";
const PROACTIVITY_LOG_HAS_MORE: &str = "proactivity_log_has_more";
const VOICE_SETTINGS: &str = "voice_settings";

/// A memory note with the flag that says whether its text is loaded.
#[derive(Serialize, Deserialize)]
struct SavedNote {
    note: MemoryNote,
    has_content: bool,
}

fn json<T: Serialize>(value: &T) -> Result<String, PersistError> {
    Ok(serde_json::to_string(value)?)
}

fn row<T: Serialize>(kind: &'static str, id: &str, value: &T) -> Result<Row, PersistError> {
    Ok(Row {
        kind,
        id: id.to_owned(),
        json: json(value)?,
    })
}

/// Makes the rows of a save from the cache. `fresh_at` is when the data was
/// last known to be current.
///
/// The cache keeps more than this file holds. What stays out, and why:
/// - `touched`, `sign_ins`, `sign_ins_dismissed`: they guard a load that is
///   in flight, and no load survives a restart.
/// - `convo` (without the archived tasks): the search, the open item and the
///   loaded message threads are screen state. A thread is in `loaded_threads`
///   already, and the app loads the search and the item again when the user
///   opens them.
/// - `pending` and `send_failures`: the outbox has its own file.
/// - `acp_registry*`, `extensions`, `local_blobs`, `admin` (without the
///   paired clients): the app asks the hub for them when a page needs them.
/// - `proactivity` (without the settings, the log entries and whether older
///   entries exist): the load state and the last error are screen state.
/// - `voice` (without the settings): the "Try it" sample stays only in
///   memory, and the guide that waits for Do it comes from the hub again.
pub fn rows(cache: &Cache, fresh_at: i64, owner: &Owner) -> Result<Rows, PersistError> {
    let mut entities = Vec::new();
    for (id, v) in &cache.groups {
        entities.push(row(GROUP, id, v)?);
    }
    for (id, v) in &cache.channels {
        entities.push(row(CHANNEL, id, v)?);
    }
    for (id, v) in &cache.repos {
        entities.push(row(REPO, id, v)?);
    }
    for (id, v) in &cache.workers {
        entities.push(row(WORKER, id, v)?);
    }
    for (id, v) in &cache.messages {
        entities.push(row(MESSAGE, id, v)?);
    }
    for (id, v) in &cache.channel_files {
        entities.push(row(CHANNEL_FILE, id, v)?);
    }
    for (id, v) in &cache.routines {
        entities.push(row(ROUTINE, id, v)?);
    }
    for (routine, runs) in &cache.routine_runs {
        for (number, run) in runs {
            entities.push(row(ROUTINE_RUN, &format!("{routine}:{number}"), run)?);
        }
    }
    for (id, v) in &cache.memory_notes {
        let saved = SavedNote {
            note: v.note.clone(),
            has_content: v.has_content,
        };
        entities.push(row(MEMORY_NOTE, id, &saved)?);
    }
    for (id, v) in &cache.tasks {
        entities.push(row(TASK, id, v)?);
    }
    for v in &cache.convo.archived.tasks {
        entities.push(row(ARCHIVED_TASK, &v.id.to_string(), v)?);
    }
    for (id, v) in &cache.approvals {
        entities.push(row(APPROVAL, id, v)?);
    }
    for (id, v) in &cache.decisions {
        entities.push(row(DECISION, id, v)?);
    }
    for (id, v) in &cache.read_markers {
        entities.push(row(READ_MARKER, id, v)?);
    }
    for (session, items) in &cache.session_items {
        for (item, summary) in items {
            entities.push(row(SESSION_ITEM, &format!("{session}:{item}"), summary)?);
        }
    }
    for (id, v) in &cache.session_activity {
        entities.push(row(SESSION_ACTIVITY, id, v)?);
    }
    for (id, v) in &cache.connections {
        entities.push(row(CONNECTION, id, v)?);
    }
    for (id, v) in &cache.skills {
        entities.push(row(SKILL, id, v)?);
    }
    for (id, v) in &cache.admin.clients {
        entities.push(row(CLIENT, id, v)?);
    }
    for (id, v) in &cache.proactivity.wakes {
        entities.push(row(PROACTIVE_WAKE, &id.to_string(), v)?);
    }
    let mut meta = BTreeMap::new();
    meta.insert(SEQ, cache.seq.to_string());
    meta.insert(HUB_EPOCH, json(&cache.hub_epoch)?);
    meta.insert(FRESH_AT, fresh_at.to_string());
    meta.insert(CLIENT_ID, owner.client_id.clone());
    meta.insert(HUB_URL, owner.hub_url.clone());
    meta.insert(FILES_LOADED, json(&cache.files_loaded)?);
    meta.insert(RUNS_LOADED, json(&cache.runs_loaded)?);
    meta.insert(MEMORY_LOADED, json(&cache.memory_loaded)?);
    meta.insert(MEMORY_DELETED_LOADED, json(&cache.memory_deleted_loaded)?);
    meta.insert(TASK_PREVIEWS, json(&cache.task_previews)?);
    meta.insert(TASKS_HAVE_MORE, json(&cache.tasks_have_more)?);
    meta.insert(TASKS_ABSENT, json(&cache.tasks_absent)?);
    meta.insert(ARCHIVED_HAS_MORE, json(&cache.convo.archived.has_more)?);
    meta.insert(HAS_OLDER, json(&cache.has_older)?);
    meta.insert(CHANNELS_LOADED, json(&cache.channels_loaded)?);
    meta.insert(LOADED_THREADS, json(&cache.loaded_threads)?);
    meta.insert(PREFERENCES, json(&cache.preferences)?);
    meta.insert(PROACTIVITY_SETTINGS, json(&cache.proactivity.settings)?);
    meta.insert(VOICE_SETTINGS, json(&cache.voice.settings)?);
    meta.insert(
        PROACTIVITY_LOG_HAS_MORE,
        json(&cache.proactivity.log_has_more)?,
    );
    Ok(Rows { entities, meta })
}

fn parse<T: DeserializeOwned>(text: &str) -> Result<T, PersistError> {
    Ok(serde_json::from_str(text)?)
}

fn meta_value<T: DeserializeOwned + Default>(
    meta: &HashMap<String, String>,
    key: &str,
) -> Result<T, PersistError> {
    match meta.get(key) {
        Some(text) => parse(text),
        None => Ok(T::default()),
    }
}

/// Puts one row back in the cache.
fn restore_row(cache: &mut Cache, kind: &str, id: String, text: &str) -> Result<(), PersistError> {
    match kind {
        GROUP => {
            cache.groups.insert(id, parse::<Group>(text)?);
        }
        CHANNEL => {
            cache.channels.insert(id, parse::<Channel>(text)?);
        }
        REPO => {
            cache.repos.insert(id, parse::<Repo>(text)?);
        }
        WORKER => {
            cache.workers.insert(id, parse::<Worker>(text)?);
        }
        MESSAGE => {
            cache.messages.insert(id, parse::<Message>(text)?);
        }
        CHANNEL_FILE => {
            cache.channel_files.insert(id, parse::<ChannelFile>(text)?);
        }
        ROUTINE => {
            cache.routines.insert(id, parse::<Routine>(text)?);
        }
        ROUTINE_RUN => {
            let run: RoutineRun = parse(text)?;
            cache
                .routine_runs
                .entry(run.routine_id.to_string())
                .or_default()
                .insert(run.id, run);
        }
        MEMORY_NOTE => {
            let saved: SavedNote = parse(text)?;
            cache.memory_notes.insert(
                id,
                CachedNote {
                    note: saved.note,
                    has_content: saved.has_content,
                },
            );
        }
        TASK => {
            cache.tasks.insert(id, parse::<Task>(text)?);
        }
        ARCHIVED_TASK => {
            cache.convo.archived.tasks.push(parse::<Task>(text)?);
        }
        APPROVAL => {
            cache.approvals.insert(id, parse::<Approval>(text)?);
        }
        DECISION => {
            cache.decisions.insert(id, parse::<Decision>(text)?);
        }
        READ_MARKER => {
            cache.read_markers.insert(id, parse::<ReadMarker>(text)?);
        }
        SESSION_ITEM => {
            // The key is `<session>:<item>`. Neither ID holds a colon.
            let (session, item) = id.split_once(':').ok_or_else(|| {
                PersistError::Damaged(format!("the session item key {id:?} has no colon"))
            })?;
            cache
                .session_items
                .entry(session.to_owned())
                .or_default()
                .insert(item.to_owned(), parse::<ItemSummary>(text)?);
        }
        SESSION_ACTIVITY => {
            cache
                .session_activity
                .insert(id, parse::<SessionActivity>(text)?);
        }
        CONNECTION => {
            cache.connections.insert(id, parse::<Connection>(text)?);
        }
        SKILL => {
            cache.skills.insert(id, parse::<Skill>(text)?);
        }
        CLIENT => {
            cache.admin.clients.insert(id, parse::<Client>(text)?);
        }
        PROACTIVE_WAKE => {
            let wake = parse::<ProactiveWake>(text)?;
            cache.proactivity.wakes.insert(wake.id, wake);
        }
        // A kind that this version does not know came from a newer one that
        // kept the schema version. Leave it out. The next save removes it.
        _ => {}
    }
    Ok(())
}

/// Makes the cache from the rows and the meta values of a file.
fn restore(
    rows: Vec<(String, String, String)>,
    meta: &HashMap<String, String>,
) -> Result<Cache, PersistError> {
    let mut cache = Cache {
        // A file holds a cache only after its first load was done.
        loaded: true,
        seq: meta_value(meta, SEQ)?,
        hub_epoch: meta_value(meta, HUB_EPOCH)?,
        files_loaded: meta_value(meta, FILES_LOADED)?,
        runs_loaded: meta_value(meta, RUNS_LOADED)?,
        memory_loaded: meta_value(meta, MEMORY_LOADED)?,
        memory_deleted_loaded: meta_value(meta, MEMORY_DELETED_LOADED)?,
        task_previews: meta_value(meta, TASK_PREVIEWS)?,
        tasks_have_more: meta_value(meta, TASKS_HAVE_MORE)?,
        tasks_absent: meta_value(meta, TASKS_ABSENT)?,
        has_older: meta_value(meta, HAS_OLDER)?,
        channels_loaded: meta_value::<BTreeSet<String>>(meta, CHANNELS_LOADED)?,
        loaded_threads: meta_value(meta, LOADED_THREADS)?,
        preferences: meta_value::<Preferences>(meta, PREFERENCES)?,
        ..Cache::default()
    };
    cache.proactivity.settings = meta_value(meta, PROACTIVITY_SETTINGS)?;
    cache.voice.settings = meta_value(meta, VOICE_SETTINGS)?;
    for (kind, id, text) in rows {
        restore_row(&mut cache, &kind, id, &text)?;
    }
    // The rows come in key order. The list shows the newest archive first.
    let archived = &mut cache.convo.archived;
    archived
        .tasks
        .sort_by(|a, b| b.archived_at.cmp(&a.archived_at).then(a.id.cmp(&b.id)));
    if !archived.tasks.is_empty() {
        archived.has_more = meta_value(meta, ARCHIVED_HAS_MORE)?;
        archived.load = crate::connectors::LoadState::Loaded;
    }
    // The log comes with the settings in the first load, and a resume does
    // not load it again. A log with entries counts as loaded.
    if !cache.proactivity.wakes.is_empty() {
        cache.proactivity.log_has_more = meta_value(meta, PROACTIVITY_LOG_HAS_MORE)?;
        cache.proactivity.log_load = crate::connectors::LoadState::Loaded;
    }
    Ok(cache)
}

fn hash_of(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// An open cache file, and what it holds, so a save writes only the change.
pub struct CacheFile {
    path: PathBuf,
    conn: rusqlite::Connection,
    /// The hash of the JSON of each row in the file.
    written: HashMap<(String, String), u64>,
    /// The meta values in the file.
    written_meta: HashMap<String, String>,
}

impl std::fmt::Debug for CacheFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheFile")
            .field("path", &self.path)
            .field("rows", &self.written.len())
            .finish()
    }
}

impl CacheFile {
    /// Opens the file at `path`. A file that is missing is made. The data in
    /// it comes back when it belongs to `owner`. Data of another owner is
    /// removed, and the answer has no data.
    ///
    /// An error means that the file cannot be used. [`open_or_recover`] moves
    /// such a file aside and starts again.
    pub fn open(path: &Path, owner: &Owner) -> Result<(CacheFile, Option<Saved>), PersistError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        create_private(path)?;
        let conn = rusqlite::Connection::open(path)?;
        // Waits instead of failing when another process holds the file.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let has_tables: bool = conn.query_row(
            "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name IN ('entity', 'meta')",
            [],
            |r| r.get(0),
        )?;
        if version == 0 && !has_tables {
            conn.execute_batch(&format!(
                "BEGIN;
                 CREATE TABLE entity (
                     kind TEXT NOT NULL,
                     id TEXT NOT NULL,
                     json TEXT NOT NULL,
                     PRIMARY KEY (kind, id)
                 ) WITHOUT ROWID;
                 CREATE TABLE meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL) WITHOUT ROWID;
                 PRAGMA user_version = {SCHEMA_VERSION};
                 COMMIT;"
            ))?;
        } else if version != SCHEMA_VERSION || !has_tables {
            return Err(PersistError::Version { found: version });
        }
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(PersistError::Damaged(check));
        }
        let mut file = CacheFile {
            path: path.to_owned(),
            conn,
            written: HashMap::new(),
            written_meta: HashMap::new(),
        };
        let rows = file.read_rows()?;
        let meta = file.read_meta()?;
        file.written = rows
            .iter()
            .map(|(kind, id, text)| ((kind.clone(), id.clone()), hash_of(text)))
            .collect();
        file.written_meta = meta.clone();
        let mine = meta.get(CLIENT_ID).map(String::as_str) == Some(owner.client_id.as_str())
            && meta.get(HUB_URL).map(String::as_str) == Some(owner.hub_url.as_str());
        if !mine {
            if !rows.is_empty() || !meta.is_empty() {
                tracing::info!(
                    path = %path.display(),
                    "the saved data belongs to another hub or client, so the client removes it"
                );
                file.clear()?;
            }
            return Ok((file, None));
        }
        let fresh_at = meta_value(&meta, FRESH_AT)?;
        let cache = restore(rows, &meta)?;
        Ok((file, Some(Saved { cache, fresh_at })))
    }

    fn read_rows(&self) -> Result<Vec<(String, String, String)>, PersistError> {
        let mut statement = self.conn.prepare("SELECT kind, id, json FROM entity")?;
        let rows = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn read_meta(&self) -> Result<HashMap<String, String>, PersistError> {
        let mut statement = self.conn.prepare("SELECT key, value FROM meta")?;
        let meta = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<HashMap<String, String>, _>>()?;
        Ok(meta)
    }

    /// Removes every row.
    fn clear(&mut self) -> Result<(), PersistError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM entity", [])?;
        tx.execute("DELETE FROM meta", [])?;
        tx.commit()?;
        self.written.clear();
        self.written_meta.clear();
        Ok(())
    }

    /// Saves `rows` in one transaction: the rows that changed, and the
    /// removal of the rows that are gone. Returns how many rows changed. A
    /// failed save leaves the file as it was.
    pub fn write(&mut self, rows: Rows) -> Result<usize, PersistError> {
        let mut changed = Vec::new();
        let mut keep: BTreeSet<(&str, &str)> = BTreeSet::new();
        for row in &rows.entities {
            keep.insert((row.kind, row.id.as_str()));
            let key = (row.kind.to_owned(), row.id.clone());
            if self.written.get(&key) != Some(&hash_of(&row.json)) {
                changed.push(row);
            }
        }
        let gone: Vec<(String, String)> = self
            .written
            .keys()
            .filter(|(kind, id)| !keep.contains(&(kind.as_str(), id.as_str())))
            .cloned()
            .collect();
        let meta_changed: Vec<(&&str, &String)> = rows
            .meta
            .iter()
            .filter(|(key, value)| self.written_meta.get(**key) != Some(*value))
            .collect();
        if changed.is_empty() && gone.is_empty() && meta_changed.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        {
            let mut put = tx.prepare_cached(
                "INSERT OR REPLACE INTO entity (kind, id, json) VALUES (?1, ?2, ?3)",
            )?;
            for row in &changed {
                put.execute(params![row.kind, row.id, row.json])?;
            }
            let mut remove = tx.prepare_cached("DELETE FROM entity WHERE kind = ?1 AND id = ?2")?;
            for (kind, id) in &gone {
                remove.execute(params![kind, id])?;
            }
            let mut put_meta =
                tx.prepare_cached("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)")?;
            for (key, value) in &meta_changed {
                put_meta.execute(params![key, value])?;
            }
        }
        tx.commit()?;
        // Only now, after the commit, does the file hold the new rows.
        for row in changed.iter() {
            self.written
                .insert((row.kind.to_owned(), row.id.clone()), hash_of(&row.json));
        }
        for key in &gone {
            self.written.remove(key);
        }
        for (key, value) in rows.meta.iter() {
            self.written_meta.insert((*key).to_owned(), value.clone());
        }
        Ok(changed.len() + gone.len())
    }

    /// The version of the file's tables.
    pub fn schema_version(&self) -> Result<i64, PersistError> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Opens the file for `owner`, and never fails. A file that cannot be used
/// (not a database, a damaged one, or one from a newer or an unknown schema)
/// is moved to `<name>.broken`, and the client starts with no saved data. A
/// log line says what happened. `None` for the file means that saving is not
/// possible, and the app runs on without it.
pub fn open_or_recover(path: &Path, owner: &Owner) -> (Option<CacheFile>, Option<Saved>) {
    match CacheFile::open(path, owner) {
        Ok((file, saved)) => (Some(file), saved),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "the saved data cannot be used, so the client moves it aside and starts with no saved data"
            );
            move_aside(path);
            match CacheFile::open(path, owner) {
                Ok((file, saved)) => (Some(file), saved),
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "the client cannot make a new cache file, so it runs without saved data"
                    );
                    (None, None)
                }
            }
        }
    }
}

/// The path of a companion of the cache file.
fn companion(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Moves the file to `<name>.broken` and removes the files that go with it.
/// A journal that stays would apply to the new file and damage it.
fn move_aside(path: &Path) {
    let broken = companion(path, ".broken");
    let _ = std::fs::remove_file(&broken);
    if path.exists()
        && let Err(err) = std::fs::rename(path, &broken)
    {
        tracing::warn!(error = %err, "cannot move the cache file aside, so the client removes it");
        let _ = std::fs::remove_file(path);
    }
    for suffix in ["-journal", "-wal", "-shm"] {
        let _ = std::fs::remove_file(companion(path, suffix));
    }
}

/// Removes the cache file and every file that goes with it. Close the
/// [`CacheFile`] first. A file that is not there is not an error.
pub fn delete(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in COMPANIONS {
        let _ = std::fs::remove_file(companion(path, suffix));
    }
}

/// Makes the file with a mode that only this user can use, when it is not
/// there. SQLite then opens it as an empty database.
fn create_private(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map(|_| ())
    }
    #[cfg(not(unix))]
    {
        // On Windows the file inherits the ACL of the user's config folder,
        // which only that user can read.
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::{
        auth::PublicKey,
        ids::{
            ApprovalId, ChannelId, ClientId, DecisionId, GroupId, MessageId, RepoId, RequestId,
            RoutineId, SessionId, TaskId, WorkerId,
        },
        model::{
            ApprovalDetails, ApprovalMode, ApprovalStatus, Author, ClientKind, DecisionOption,
            DecisionStatus, FavoriteModel, MemoryAuthor, MemoryScope, MessageKind, Os,
            PlacementRequest, RequestOption, RequestOptionKind, RoutineAction, RoutineRunStatus,
            RoutineSchedule, Sha256Hash, TaskCreator, WorkerState,
        },
        status::TaskStatus,
        time::Timestamp,
    };

    use super::*;
    use crate::{activity::WorkKind, test_entities};

    fn owner() -> Owner {
        Owner {
            client_id: "cli_one".into(),
            hub_url: "http://hub.test:7600".into(),
        }
    }

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    fn message(id: MessageId, channel: ChannelId, body: &str) -> Message {
        Message {
            id,
            channel_id: channel,
            thread_root_id: None,
            author: Author::Assistant,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind: MessageKind::Text,
            body: body.into(),
            attachments: vec![],
            in_timeline: true,
            client_message_id: None,
            reply_count: 0,
            last_reply_at: None,
            reactions: vec![],
            edited_at: None,
            deleted_at: None,
            created_at: at(5),
            proactive: None,
            suggestions: Vec::new(),
            source_message_id: None,
        }
    }

    fn task(id: TaskId, channel: ChannelId, card: MessageId) -> Task {
        Task {
            id,
            channel_id: channel,
            title: "Build the Windows engine".into(),
            brief: "Build it on bigboi.".into(),
            status: TaskStatus::Review,
            status_reason: None,
            created_by: TaskCreator::Assistant,
            card_message_id: card,
            model_selection: None,
            placement: PlacementRequest::default(),
            approval_mode: ApprovalMode::Ask,
            start_at: None,
            current_session_id: None,
            worker_id: None,
            review_summary: Some("Ready.".into()),
            resolved_at: None,
            session_ended: false,
            archived_at: None,
            created_at: at(1),
            updated_at: at(2),
        }
    }

    fn worker(id: WorkerId) -> Worker {
        Worker {
            id,
            name: "bigboi".into(),
            hostname: "bigboi.local".into(),
            os: Os::Windows,
            arch: "x86_64".into(),
            public_key: PublicKey::from_bytes([3; 32]),
            version: "0.1.0".into(),
            protocol_version: 1,
            tags: vec!["gpu".into()],
            paused: false,
            state: WorkerState::Online,
            last_seen_at: Some(at(9)),
            inventory: None,
            paired_at: at(1),
            revoked_at: None,
        }
    }

    /// One entity of every kind that the file holds, and a value for every
    /// set and flag. The `Debug` text of a field says what it holds.
    fn full_cache() -> Cache {
        let channel_id = ChannelId::new();
        let card = MessageId::new();
        let task_id = TaskId::new();
        let routine_id = RoutineId::new();
        let session = SessionId::new();
        let mut cache = Cache {
            seq: 4_242,
            loaded: true,
            hub_epoch: Some("epoch-1".into()),
            tasks_have_more: true,
            ..Cache::default()
        };
        let group = Group {
            id: GroupId::new(),
            name: "Home".into(),
            position: 2,
            collapsed: true,
        };
        cache.groups.insert(group.id.to_string(), group);
        cache.channels.insert(
            channel_id.to_string(),
            test_entities::channel(channel_id, "sayso"),
        );
        let repo = Repo {
            id: RepoId::new(),
            url: "https://github.com/watzon/sayso".into(),
            name: "sayso".into(),
            default_branch: "main".into(),
        };
        cache.repos.insert(repo.id.to_string(), repo);
        let w = worker(WorkerId::new());
        cache.workers.insert(w.id.to_string(), w);
        let m = message(MessageId::new(), channel_id, "Good morning, Chris.");
        cache.messages.insert(m.id.to_string(), m);
        let file = ChannelFile {
            channel_id,
            path: "plans/windows.pdf".into(),
            blob_sha256: Sha256Hash::from_bytes([9; 32]),
            size: 1_024,
            mime: "application/pdf".into(),
            task_id: None,
            title: Some("The plan".into()),
            updated_by: "user".into(),
            updated_at: at(7),
            deleted_at: None,
        };
        cache.channel_files.insert(file.entity_id(), file);
        cache.files_loaded.insert(channel_id.to_string());
        let routine = Routine {
            id: routine_id,
            channel_id,
            name: "Sunset check".into(),
            schedule: RoutineSchedule::Interval { seconds: 3_600 },
            prompt: "Check.".into(),
            action: RoutineAction::CoordinatorTurn,
            enabled: true,
            next_run_at: Some(at(100)),
            last_run_at: None,
            last_run_status: None,
            kind: Default::default(),
            origin: Default::default(),
            created_at: None,
        };
        let run = RoutineRun {
            id: 12,
            routine_id,
            channel_id,
            started_at: at(3),
            ended_at: Some(at(4)),
            status: RoutineRunStatus::Ok,
            manual: true,
            message_id: None,
            task_id: None,
            error: None,
        };
        cache.routines.insert(routine_id.to_string(), routine);
        cache
            .routine_runs
            .entry(routine_id.to_string())
            .or_default()
            .insert(run.id, run);
        cache.runs_loaded.insert(routine_id.to_string());
        let note = MemoryNote {
            scope: MemoryScope::Channel(channel_id),
            path: "people/kevin.md".into(),
            content: "Kevin runs the lights.".into(),
            sha256: Sha256Hash::from_bytes([5; 32]),
            version: 3,
            updated_by: MemoryAuthor::User,
            updated_at: at(8),
            suggested_by: None,
            deleted_at: None,
        };
        cache.memory_notes.insert(
            note.entity_id(),
            CachedNote {
                note,
                has_content: true,
            },
        );
        cache.memory_loaded.insert(channel_id.to_string());
        cache.memory_deleted_loaded.insert("global".into());
        cache
            .tasks
            .insert(task_id.to_string(), task(task_id, channel_id, card));
        cache.task_previews.insert(task_id.to_string());
        let mut archived = task(TaskId::new(), channel_id, card);
        archived.archived_at = Some(at(9));
        cache.convo.archived.tasks.push(archived);
        cache.convo.archived.has_more = true;
        cache.tasks_absent.insert(TaskId::new().to_string());
        let approval = Approval {
            id: ApprovalId::new(),
            channel_id,
            session_id: session,
            task_id: Some(task_id),
            message_id: card,
            request_id: RequestId::new("req_1"),
            summary: "Codex wants to run a command.".into(),
            details: ApprovalDetails::Command {
                command: "cargo build".into(),
                cwd: Some("C:\\Code\\sayso".into()),
            },
            options: vec![RequestOption {
                id: "allow".into(),
                label: "Allow once".into(),
                kind: RequestOptionKind::AllowOnce,
                description: None,
            }],
            status: ApprovalStatus::Pending,
            answer_option_id: None,
            answered_at: None,
            answered_by_client_id: None,
            created_at: at(6),
        };
        cache.approvals.insert(approval.id.to_string(), approval);
        let decision = Decision {
            id: DecisionId::new(),
            channel_id,
            task_id: None,
            session_id: session,
            message_id: card,
            request_id: None,
            question: "Which one?".into(),
            context: None,
            options: vec![
                DecisionOption {
                    label: "A".into(),
                    consequence: "Does A.".into(),
                    id: None,
                },
                DecisionOption {
                    label: "B".into(),
                    consequence: "Does B.".into(),
                    id: None,
                },
            ],
            recommended_index: Some(1),
            allow_free_text: false,
            status: DecisionStatus::Pending,
            answer: None,
            answered_at: None,
            created_at: at(6),
        };
        cache.decisions.insert(decision.id.to_string(), decision);
        let marker = ReadMarker {
            channel_id,
            thread_root_id: Some(card),
            last_read_message_id: card,
        };
        cache
            .read_markers
            .insert(format!("{channel_id}:{card}"), marker);
        cache
            .session_items
            .entry(session.to_string())
            .or_default()
            .insert(
                "itm_1".into(),
                ItemSummary::Command("cargo build --release".into()),
            );
        cache
            .session_items
            .entry(session.to_string())
            .or_default()
            .insert(
                "itm_2".into(),
                ItemSummary::Files(vec!["src/main.rs".into(), "Cargo.toml".into()]),
            );
        cache
            .session_items
            .entry(session.to_string())
            .or_default()
            .insert(
                "itm_3".into(),
                ItemSummary::Plan {
                    steps: vec![consort_proto::model::PlanStep {
                        text: "Read the plan".into(),
                        status: consort_proto::model::PlanStepStatus::InProgress,
                    }],
                    updated_at: 78,
                },
            );
        cache.session_activity.insert(
            session.to_string(),
            SessionActivity {
                session_id: session.to_string(),
                channel_id: Some(channel_id.to_string()),
                kind: WorkKind::Command,
                in_progress: true,
                updated_at: 77,
            },
        );
        cache.has_older.insert(channel_id.to_string(), true);
        cache.channels_loaded.insert(channel_id.to_string());
        cache.loaded_threads.insert(card.to_string());
        cache.preferences = Preferences {
            account_fallback: true,
            favorite_models: vec![FavoriteModel {
                account_label: "Claude Work".into(),
                model: "opus-5.5".into(),
            }],
        };
        let connection =
            test_entities::connection("Work Gmail", consort_proto::model::UseIn::EveryChannel, 4);
        cache
            .connections
            .insert(connection.id.to_string(), connection);
        let skill = test_entities::skill("pdf", 4);
        cache.skills.insert(skill.id.to_string(), skill);
        let client = Client {
            id: ClientId::new(),
            name: "MacBook Pro".into(),
            kind: ClientKind::Desktop,
            public_key: PublicKey::from_bytes([4; 32]),
            push_provider: None,
            paired_at: at(1),
            last_seen_at: Some(at(2)),
            revoked_at: None,
            os: Some(Os::Macos),
            paired_by: None,
        };
        cache.admin.clients.insert(client.id.to_string(), client);
        cache.voice.settings = Some(consort_proto::model::VoiceSettings {
            enabled: true,
            ..Default::default()
        });
        cache.proactivity.settings = Some(consort_proto::model::ProactivitySettings {
            paused: true,
            time_zone: Some("America/Chicago".into()),
            ..Default::default()
        });
        cache.proactivity.wakes.insert(
            7,
            ProactiveWake {
                id: 7,
                channel_id,
                kind: consort_proto::model::ProactiveKind::CheckIn,
                at: at(9),
                outcome: consort_proto::model::ProactiveOutcome::Silent,
                detail: Some("Nothing new.".into()),
                message_ids: vec![],
                task_id: None,
                routine_id: Some(routine_id),
                notify_held: false,
                ended_at: Some(at(10)),
            },
        );
        cache.proactivity.log_has_more = true;
        cache
    }

    /// Compares what the file keeps. A new field of `Cache` stops this
    /// function from compiling, so its author must decide: keep the field
    /// in the file (add it to `rows` and `restore`, and compare it here) or
    /// leave it out (say why in the comment of `rows`).
    fn assert_same(a: &Cache, b: &Cache) {
        let Cache {
            seq,
            loaded,
            hub_epoch,
            groups,
            channels,
            repos,
            workers,
            messages,
            channel_files,
            files_loaded,
            routines,
            routine_runs,
            runs_loaded,
            memory_notes,
            memory_loaded,
            memory_deleted_loaded,
            local_blobs: _,
            tasks,
            task_previews,
            tasks_have_more,
            tasks_absent,
            approvals,
            decisions,
            read_markers,
            session_items,
            session_activity,
            pending: _,
            send_failures: _,
            has_older,
            channels_loaded,
            loaded_threads,
            touched: _,
            sign_ins: _,
            sign_ins_dismissed: _,
            acp_registry: _,
            acp_registry_load: _,
            acp_icons: _,
            preferences,
            connections,
            skills,
            extensions: _,
            admin,
            convo,
            proactivity,
            voice,
        } = a;
        macro_rules! same {
            ($($field:ident),+ $(,)?) => {
                $(assert_eq!(
                    format!("{:?}", $field),
                    format!("{:?}", b.$field),
                    "the field {} differs", stringify!($field)
                );)+
            };
        }
        same!(
            seq,
            loaded,
            hub_epoch,
            groups,
            channels,
            repos,
            workers,
            messages,
            channel_files,
            files_loaded,
            routines,
            routine_runs,
            runs_loaded,
            memory_notes,
            memory_loaded,
            memory_deleted_loaded,
            tasks,
            task_previews,
            tasks_have_more,
            tasks_absent,
            approvals,
            decisions,
            read_markers,
            session_items,
            session_activity,
            has_older,
            channels_loaded,
            loaded_threads,
            preferences,
            connections,
            skills,
        );
        // The archived tasks come back in order. The search, the open item
        // and the loaded threads of `convo` are screen state, so they stay out.
        assert_eq!(
            format!("{:?}", (&convo.archived.tasks, convo.archived.has_more)),
            format!("{:?}", (&b.convo.archived.tasks, b.convo.archived.has_more)),
            "the archived tasks differ"
        );
        assert_eq!(
            format!("{:?}", admin.clients),
            format!("{:?}", b.admin.clients),
            "the paired clients differ"
        );
        // The settings, the log entries and whether older entries exist are
        // hub data. The load state and the last error are screen state.
        assert_eq!(
            format!(
                "{:?}",
                (
                    &proactivity.settings,
                    &proactivity.wakes,
                    proactivity.log_has_more
                )
            ),
            format!(
                "{:?}",
                (
                    &b.proactivity.settings,
                    &b.proactivity.wakes,
                    b.proactivity.log_has_more
                )
            ),
            "the proactivity data differs"
        );
        // The voice settings are hub data. The sample and the proposal are
        // not saved.
        assert_eq!(
            voice.settings, b.voice.settings,
            "the voice settings differ"
        );
    }

    fn save(file: &mut CacheFile, cache: &Cache, fresh_at: i64) -> usize {
        file.write(rows(cache, fresh_at, &owner()).unwrap())
            .unwrap()
    }

    #[test]
    fn every_kind_that_the_cache_keeps_comes_back_from_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let cache = full_cache();
        let (mut file, saved) = CacheFile::open(&path, &owner()).unwrap();
        assert!(saved.is_none(), "a new file holds no data");
        let written = save(&mut file, &cache, 1_800_000_000_000);
        // One row for each entity: 20 kinds, and three session items (a
        // command, a file change and a plan).
        assert_eq!(written, 22);
        drop(file);

        let (_, saved) = CacheFile::open(&path, &owner()).unwrap();
        let saved = saved.expect("the saved data");
        assert_eq!(saved.fresh_at, 1_800_000_000_000);
        assert_same(&cache, &saved.cache);
        // The file is a cache that was loaded, so a reconnect can resume.
        assert!(saved.cache.loaded);
        assert_eq!(saved.cache.seq, 4_242);
        // What the file does not hold starts empty.
        assert!(saved.cache.pending.is_empty());
        assert!(saved.cache.touched.is_empty());
    }

    #[test]
    fn a_save_writes_only_the_rows_that_changed_in_one_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut cache = full_cache();
        let (mut file, _) = CacheFile::open(&path, &owner()).unwrap();
        save(&mut file, &cache, 10);
        // Nothing changed, so nothing is written.
        assert_eq!(save(&mut file, &cache, 10), 0);
        // One message changes, one task goes, the position moves on.
        let id = cache.messages.keys().next().unwrap().clone();
        cache.messages.get_mut(&id).unwrap().body = "Edited.".into();
        let task = cache.tasks.keys().next().unwrap().clone();
        cache.tasks.remove(&task);
        cache.seq += 1;
        assert_eq!(save(&mut file, &cache, 11), 2);
        drop(file);
        let (_, saved) = CacheFile::open(&path, &owner()).unwrap();
        let saved = saved.unwrap();
        assert_eq!(saved.fresh_at, 11);
        assert_eq!(saved.cache.messages[&id].body, "Edited.");
        assert!(saved.cache.tasks.is_empty());
        assert_eq!(saved.cache.seq, 4_243);
    }

    #[test]
    fn a_time_that_moves_alone_is_saved_without_a_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let cache = full_cache();
        let (mut file, _) = CacheFile::open(&path, &owner()).unwrap();
        save(&mut file, &cache, 10);
        // Only `fresh_at` changes. No row changes, and the time is saved.
        assert_eq!(save(&mut file, &cache, 20), 0);
        drop(file);
        let (_, saved) = CacheFile::open(&path, &owner()).unwrap();
        assert_eq!(saved.unwrap().fresh_at, 20);
    }

    #[test]
    fn a_file_that_is_not_a_database_is_moved_aside_and_the_client_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "this is not a database, it is a note\n".repeat(50)).unwrap();
        let (file, saved) = open_or_recover(&path, &owner());
        assert!(saved.is_none());
        let mut file = file.expect("a new file");
        assert!(
            dir.path().join("cache.sqlite3.broken").exists(),
            "the old file is kept for a look"
        );
        // The new file works.
        save(&mut file, &full_cache(), 5);
        drop(file);
        let (_, saved) = CacheFile::open(&path, &owner()).unwrap();
        assert!(saved.is_some());
    }

    #[test]
    fn a_file_cut_short_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (mut file, _) = CacheFile::open(&path, &owner()).unwrap();
        // Enough rows for several pages.
        let mut cache = full_cache();
        let channel = ChannelId::new();
        for n in 0..400 {
            let m = message(
                MessageId::new(),
                channel,
                &format!("message {n} {}", "x".repeat(200)),
            );
            cache.messages.insert(m.id.to_string(), m);
        }
        save(&mut file, &cache, 5);
        drop(file);
        let size = std::fs::metadata(&path).unwrap().len();
        let cut = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        cut.set_len(size / 2).unwrap();
        drop(cut);
        let (file, saved) = open_or_recover(&path, &owner());
        assert!(saved.is_none(), "half a file is not data");
        assert!(file.is_some());
        assert!(dir.path().join("cache.sqlite3.broken").exists());
    }

    #[test]
    fn a_file_from_a_newer_schema_is_moved_aside_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (mut file, _) = CacheFile::open(&path, &owner()).unwrap();
        save(&mut file, &full_cache(), 5);
        drop(file);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
                .unwrap();
        }
        let (file, saved) = open_or_recover(&path, &owner());
        assert!(saved.is_none());
        let file = file.expect("a new file");
        assert_eq!(file.schema_version().unwrap(), SCHEMA_VERSION);
        // The newer file is still there, for the newer app.
        let broken = rusqlite::Connection::open(dir.path().join("cache.sqlite3.broken")).unwrap();
        let version: i64 = broken
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION + 1);
    }

    #[test]
    fn a_row_that_cannot_be_read_moves_the_file_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (mut file, _) = CacheFile::open(&path, &owner()).unwrap();
        save(&mut file, &full_cache(), 5);
        drop(file);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "UPDATE entity SET json = '{\"id\": 1}' WHERE kind = 'message'",
                [],
            )
            .unwrap();
        }
        let (file, saved) = open_or_recover(&path, &owner());
        assert!(saved.is_none());
        assert!(file.is_some());
        assert!(dir.path().join("cache.sqlite3.broken").exists());
    }

    #[test]
    fn data_of_another_hub_or_client_is_removed_and_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (mut file, _) = CacheFile::open(&path, &owner()).unwrap();
        save(&mut file, &full_cache(), 5);
        drop(file);
        for other in [
            Owner {
                client_id: "cli_two".into(),
                ..owner()
            },
            Owner {
                hub_url: "http://other.test".into(),
                ..owner()
            },
        ] {
            let (_, saved) = CacheFile::open(&path, &other).unwrap();
            assert!(saved.is_none(), "{other:?}");
            // The rows are gone. The first owner gets nothing back either.
            let (_, saved) = CacheFile::open(&path, &owner()).unwrap();
            assert!(saved.is_none());
            let (mut file, _) = CacheFile::open(&path, &owner()).unwrap();
            save(&mut file, &full_cache(), 5);
        }
    }

    #[test]
    fn delete_removes_the_file_and_the_files_that_go_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (mut file, _) = CacheFile::open(&path, &owner()).unwrap();
        save(&mut file, &full_cache(), 5);
        drop(file);
        for suffix in COMPANIONS {
            std::fs::write(companion(&path, suffix), "x").unwrap();
        }
        delete(&path);
        assert!(!path.exists());
        for suffix in COMPANIONS {
            assert!(!companion(&path, suffix).exists(), "{suffix}");
        }
        // A second delete is not an error.
        delete(&path);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_to_this_user() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (mut file, _) = CacheFile::open(&path, &owner()).unwrap();
        save(&mut file, &full_cache(), 5);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
