//! Where a paired client keeps its hub URL, client ID and private key: the
//! OS config folder, `<config dir>/consort/`. Both files have mode 0600 on
//! Unix, and every write goes to a temporary file first and then renames it,
//! so a crash never leaves half a file. The outbox, the messages that wait
//! for the hub, lives in the same folder, so one folder holds one client.

use std::path::{Path, PathBuf};

use consort_proto::ids::ClientId;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use crate::{ClientError, cache::PendingSend};

/// The client configuration that pairing writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientConfig {
    pub hub_url: String,
    pub client_id: ClientId,
    /// The name the hub knows this client by.
    pub name: String,
}

/// The files of one client: `client.json` and `client.key`.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

/// A paired client: its configuration and its key.
#[derive(Clone)]
pub struct Identity {
    pub config: ClientConfig,
    pub key: SigningKey,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the private key.
        f.debug_struct("Identity")
            .field("config", &self.config)
            .finish()
    }
}

fn storage_error(path: &Path, err: impl std::fmt::Display) -> ClientError {
    ClientError::Storage {
        path: path.display().to_string(),
        message: err.to_string(),
    }
}

impl Store {
    /// The store in `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    /// The default store: `$CONSORT_CONFIG_DIR` when it is set, else
    /// `<config dir>/consort`.
    pub fn default_location() -> Option<Store> {
        if let Some(dir) = std::env::var_os("CONSORT_CONFIG_DIR") {
            return Some(Store::new(dir));
        }
        directories::BaseDirs::new().map(|dirs| Store::new(dirs.config_dir().join("consort")))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn config_path(&self) -> PathBuf {
        self.dir.join("client.json")
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join("client.key")
    }

    fn outbox_path(&self) -> PathBuf {
        self.dir.join("outbox.json")
    }

    /// The saved cache: see [`crate::persist`].
    pub fn cache_path(&self) -> PathBuf {
        self.dir.join(crate::persist::FILE_NAME)
    }

    /// The saved identity, or `None` when this client is not paired. With
    /// only one of the two files present, the pairing is damaged, and the
    /// answer is an error, not `None`.
    pub fn load(&self) -> Result<Option<Identity>, ClientError> {
        let config_path = self.config_path();
        let key_path = self.key_path();
        match (config_path.exists(), key_path.exists()) {
            (false, false) => return Ok(None),
            (true, false) => {
                return Err(storage_error(
                    &key_path,
                    "the file is missing, but client.json exists",
                ));
            }
            (false, true) => {
                return Err(storage_error(
                    &config_path,
                    "the file is missing, but client.key exists",
                ));
            }
            (true, true) => {}
        }
        let text =
            std::fs::read_to_string(&config_path).map_err(|e| storage_error(&config_path, e))?;
        let config: ClientConfig =
            serde_json::from_str(&text).map_err(|e| storage_error(&config_path, e))?;
        let hex = std::fs::read_to_string(&key_path).map_err(|e| storage_error(&key_path, e))?;
        let bytes = decode_key(hex.trim()).ok_or_else(|| {
            storage_error(&key_path, "the file does not hold a 32-byte key in hex")
        })?;
        Ok(Some(Identity {
            config,
            key: SigningKey::from_bytes(&bytes),
        }))
    }

    /// Saves the identity. Both files are written with mode 0600 on Unix,
    /// each through a temporary file and a rename.
    pub fn save(&self, identity: &Identity) -> Result<(), ClientError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| storage_error(&self.dir, e))?;
        let key_path = self.key_path();
        let config_path = self.config_path();
        let text = serde_json::to_string_pretty(&identity.config)
            .map_err(|e| storage_error(&config_path, e))?;
        write_private(&key_path, &encode_key(&identity.key.to_bytes()))
            .map_err(|e| storage_error(&key_path, e))?;
        write_private(&config_path, &text).map_err(|e| storage_error(&config_path, e))?;
        Ok(())
    }

    /// Removes the identity, the outbox and the saved cache, for "pair
    /// again" and for unpairing. The saved cache holds the data of the old
    /// hub, so it goes with the identity.
    pub fn clear(&self) -> Result<(), ClientError> {
        for path in [self.config_path(), self.key_path(), self.outbox_path()] {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| storage_error(&path, e))?;
            }
        }
        crate::persist::delete(&self.cache_path());
        Ok(())
    }
}

/// The outbox file: the messages that wait for the hub, and the client that
/// wrote them. A file from another client (after pairing again) is ignored.
#[derive(Serialize, Deserialize)]
struct OutboxFile {
    client_id: ClientId,
    items: Vec<PendingSend>,
}

impl Store {
    /// The messages that waited for the hub when the client last ran. An
    /// outbox that is missing, damaged or from another client reads as
    /// empty: it holds a convenience copy, so it never stops the app.
    pub fn load_outbox(&self, client_id: &ClientId) -> Vec<PendingSend> {
        let path = self.outbox_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        match serde_json::from_str::<OutboxFile>(&text) {
            Ok(file) if &file.client_id == client_id => file.items,
            Ok(_) => Vec::new(),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "the outbox file is damaged, so the client starts with an empty outbox");
                Vec::new()
            }
        }
    }

    /// Saves the messages that wait for the hub. An empty list removes the
    /// file.
    pub fn save_outbox(
        &self,
        client_id: &ClientId,
        items: &[PendingSend],
    ) -> Result<(), ClientError> {
        let path = self.outbox_path();
        if items.is_empty() {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(storage_error(&path, e)),
            };
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| storage_error(&self.dir, e))?;
        let text = serde_json::to_string(&OutboxFile {
            client_id: *client_id,
            items: items.to_vec(),
        })
        .map_err(|e| storage_error(&path, e))?;
        write_private(&path, &text).map_err(|e| storage_error(&path, e))
    }
}

/// The text that the user wrote in a message box and did not send. One draft
/// for each channel and one for each task thread, so a switch to another
/// channel keeps what was typed. The file is `drafts.json` in the client's
/// folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drafts {
    /// By channel ID.
    #[serde(default)]
    pub channels: std::collections::BTreeMap<String, String>,
    /// By task ID.
    #[serde(default)]
    pub threads: std::collections::BTreeMap<String, String>,
}

impl Drafts {
    /// The draft of a channel, or an empty text.
    pub fn channel(&self, channel_id: &str) -> &str {
        self.channels.get(channel_id).map_or("", String::as_str)
    }

    /// The draft of a task thread, or an empty text.
    pub fn thread(&self, task_id: &str) -> &str {
        self.threads.get(task_id).map_or("", String::as_str)
    }

    /// Keeps the draft of a channel. A text that holds only white space is
    /// no draft, and removes the old one. An empty ID keeps nothing.
    pub fn set_channel(&mut self, channel_id: &str, text: &str) {
        set(&mut self.channels, channel_id, text);
    }

    /// Keeps the draft of a task thread. See [`Drafts::set_channel`].
    pub fn set_thread(&mut self, task_id: &str, text: &str) {
        set(&mut self.threads, task_id, text);
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty() && self.threads.is_empty()
    }

    /// Drops the drafts of channels and tasks that are not in the lists, so
    /// the file does not keep the drafts of a channel that was deleted.
    pub fn keep_only<'a>(
        &mut self,
        channel_ids: impl IntoIterator<Item = &'a str>,
        task_ids: impl IntoIterator<Item = &'a str>,
    ) {
        let channels: std::collections::BTreeSet<&str> = channel_ids.into_iter().collect();
        let tasks: std::collections::BTreeSet<&str> = task_ids.into_iter().collect();
        self.channels.retain(|id, _| channels.contains(id.as_str()));
        self.threads.retain(|id, _| tasks.contains(id.as_str()));
    }
}

fn set(map: &mut std::collections::BTreeMap<String, String>, id: &str, text: &str) {
    if id.is_empty() {
        return;
    }
    if text.trim().is_empty() {
        map.remove(id);
    } else {
        map.insert(id.to_owned(), text.to_owned());
    }
}

impl Store {
    fn drafts_path(&self) -> PathBuf {
        self.dir.join("drafts.json")
    }

    /// The drafts that were saved. A missing or damaged file reads as no
    /// drafts: it holds a convenience copy, so it never stops the app.
    pub fn load_drafts(&self) -> Drafts {
        let path = self.drafts_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Drafts::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), error = %err, "the drafts file is damaged, so the client starts with no drafts");
            Drafts::default()
        })
    }

    /// Saves the drafts through a temporary file and a rename, so a crash
    /// never leaves half a file. No drafts removes the file.
    pub fn save_drafts(&self, drafts: &Drafts) -> Result<(), ClientError> {
        let path = self.drafts_path();
        if drafts.is_empty() {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(storage_error(&path, e)),
            };
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| storage_error(&self.dir, e))?;
        let text = serde_json::to_string(drafts).map_err(|e| storage_error(&path, e))?;
        write_private(&path, &text).map_err(|e| storage_error(&path, e))
    }
}

impl Store {
    fn notifications_path(&self) -> PathBuf {
        self.dir.join("notifications.json")
    }

    /// The notification settings of this client. A missing or damaged file
    /// reads as the defaults.
    pub fn load_notification_settings(&self) -> crate::notify::NotificationSettings {
        let path = self.notifications_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return crate::notify::NotificationSettings::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), error = %err, "the notification settings file is damaged, so the client uses the defaults");
            crate::notify::NotificationSettings::default()
        })
    }

    /// Saves the notification settings through a temporary file and a
    /// rename.
    pub fn save_notification_settings(
        &self,
        settings: &crate::notify::NotificationSettings,
    ) -> Result<(), ClientError> {
        let path = self.notifications_path();
        std::fs::create_dir_all(&self.dir).map_err(|e| storage_error(&self.dir, e))?;
        let text = serde_json::to_string_pretty(settings).map_err(|e| storage_error(&path, e))?;
        write_private(&path, &text).map_err(|e| storage_error(&path, e))
    }
}

/// The window layout that the user chose. It lives on this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayoutSettings {
    /// The user hid the sidebar.
    #[serde(default)]
    pub sidebar_hidden: bool,
    /// The width that the user dragged the right panel to.
    #[serde(default)]
    pub panel_width: Option<f32>,
}

impl Store {
    fn layout_path(&self) -> PathBuf {
        self.dir.join("layout.json")
    }

    /// The saved window layout. A missing or damaged file reads as the
    /// defaults.
    pub fn load_layout(&self) -> LayoutSettings {
        let path = self.layout_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return LayoutSettings::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), error = %err, "the layout file is damaged, so the client uses the defaults");
            LayoutSettings::default()
        })
    }

    /// Saves the window layout through a temporary file and a rename.
    pub fn save_layout(&self, layout: &LayoutSettings) -> Result<(), ClientError> {
        let path = self.layout_path();
        std::fs::create_dir_all(&self.dir).map_err(|e| storage_error(&self.dir, e))?;
        let text = serde_json::to_string_pretty(layout).map_err(|e| storage_error(&path, e))?;
        write_private(&path, &text).map_err(|e| storage_error(&path, e))
    }
}

/// Writes a file that only this user can read, through a temporary file in
/// the same folder and a rename, so a reader sees the old file or the new
/// one and never half of it.
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = open_private(&temp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // The mode applies only when the file is new, so set it again.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    // On Windows the file inherits the ACL of the user's config folder, which
    // only that user can read.
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

fn encode_key(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_key(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity {
            config: ClientConfig {
                hub_url: "http://127.0.0.1:7600".into(),
                client_id: "cli_00000000-0000-0000-0000-000000000001".parse().unwrap(),
                name: "Test".into(),
            },
            key: SigningKey::from_bytes(&[7u8; 32]),
        }
    }

    #[test]
    fn notification_settings_round_trip_and_a_damaged_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let defaults = crate::notify::NotificationSettings::default();
        assert_eq!(store.load_notification_settings(), defaults);
        let changed = crate::notify::NotificationSettings {
            show_text: true,
            routines: true,
            ..defaults
        };
        store.save_notification_settings(&changed).unwrap();
        assert_eq!(store.load_notification_settings(), changed);
        std::fs::write(dir.path().join("notifications.json"), "{ nope").unwrap();
        assert_eq!(store.load_notification_settings(), defaults);
    }

    #[test]
    fn an_identity_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert!(store.load().unwrap().is_none());
        store.save(&identity()).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.config, identity().config);
        assert_eq!(loaded.key.to_bytes(), [7u8; 32]);
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_and_the_config_file_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.save(&identity()).unwrap();
        for name in ["client.key", "client.json"] {
            let mode = std::fs::metadata(dir.path().join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
    }

    #[test]
    fn a_save_leaves_no_temporary_file_and_replaces_the_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.save(&identity()).unwrap();
        let mut other = identity();
        other.config.name = "Other".into();
        store.save(&other).unwrap();
        assert_eq!(store.load().unwrap().unwrap().config.name, "Other");
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["client.json", "client.key"]);
    }

    #[test]
    fn one_file_alone_is_an_error_and_not_an_unpaired_client() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.save(&identity()).unwrap();
        std::fs::remove_file(dir.path().join("client.key")).unwrap();
        let err = store.load().unwrap_err();
        assert!(err.to_string().contains("client.key"), "{err}");
        store.save(&identity()).unwrap();
        std::fs::remove_file(dir.path().join("client.json")).unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn a_damaged_key_is_an_error_that_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.save(&identity()).unwrap();
        std::fs::write(dir.path().join("client.key"), "not a key").unwrap();
        let err = store.load().unwrap_err();
        assert!(err.to_string().contains("client.key"), "{err}");
        assert!(
            !err.to_string().contains("Cannot save"),
            "a load error must not talk about saving: {err}"
        );
    }

    fn pending(id: &str, body: &str) -> PendingSend {
        PendingSend {
            client_message_id: id.into(),
            channel_id: "chn_00000000-0000-0000-0000-000000000009".into(),
            thread_root_id: None,
            body: body.into(),
            created_at: 5,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn drafts_are_kept_per_channel_and_per_thread_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert_eq!(store.load_drafts(), Drafts::default());
        let mut drafts = Drafts::default();
        drafts.set_channel("chn_a", "A draft in A");
        drafts.set_channel("chn_b", "A draft in B\nwith two lines");
        drafts.set_thread("tsk_1", "A reply");
        // White space is no draft, and an empty ID keeps nothing.
        drafts.set_channel("chn_c", "  \n");
        drafts.set_channel("", "Lost");
        assert_eq!(drafts.channel("chn_a"), "A draft in A");
        assert_eq!(drafts.channel("chn_c"), "");
        assert_eq!(drafts.thread("tsk_1"), "A reply");
        store.save_drafts(&drafts).unwrap();
        // A new store on the same folder, as after a restart.
        let again = Store::new(dir.path()).load_drafts();
        assert_eq!(again, drafts);
        assert_eq!(again.channel("chn_b"), "A draft in B\nwith two lines");
        // The write leaves no temporary file.
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["drafts.json"]);
    }

    #[test]
    fn a_cleared_draft_leaves_the_file_and_no_drafts_remove_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let mut drafts = Drafts::default();
        drafts.set_channel("chn_a", "One");
        drafts.set_channel("chn_b", "Two");
        store.save_drafts(&drafts).unwrap();
        drafts.set_channel("chn_a", "");
        store.save_drafts(&drafts).unwrap();
        assert_eq!(store.load_drafts().channel("chn_b"), "Two");
        assert_eq!(store.load_drafts().channel("chn_a"), "");
        drafts.set_channel("chn_b", "");
        store.save_drafts(&drafts).unwrap();
        assert!(!dir.path().join("drafts.json").exists());
        // Removing a file that is not there is fine.
        store.save_drafts(&drafts).unwrap();
    }

    #[test]
    fn a_damaged_drafts_file_reads_as_no_drafts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("drafts.json"), "{broken").unwrap();
        assert_eq!(Store::new(dir.path()).load_drafts(), Drafts::default());
    }

    #[test]
    fn drafts_of_gone_channels_and_tasks_are_dropped() {
        let mut drafts = Drafts::default();
        drafts.set_channel("chn_a", "Keep");
        drafts.set_channel("chn_gone", "Drop");
        drafts.set_thread("tsk_1", "Keep");
        drafts.set_thread("tsk_gone", "Drop");
        drafts.keep_only(["chn_a"], ["tsk_1"]);
        assert_eq!(drafts.channel("chn_a"), "Keep");
        assert_eq!(drafts.channel("chn_gone"), "");
        assert_eq!(drafts.thread("tsk_gone"), "");
    }

    #[test]
    fn the_outbox_survives_and_belongs_to_one_client() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let id = identity().config.client_id;
        assert!(store.load_outbox(&id).is_empty());
        let items = vec![pending("a", "First"), pending("b", "Second")];
        store.save_outbox(&id, &items).unwrap();
        assert_eq!(store.load_outbox(&id), items);
        let other: ClientId = "cli_00000000-0000-0000-0000-000000000002".parse().unwrap();
        assert!(
            store.load_outbox(&other).is_empty(),
            "another client does not read it"
        );
        store.save_outbox(&id, &[]).unwrap();
        assert!(!dir.path().join("outbox.json").exists());
        std::fs::write(dir.path().join("outbox.json"), "{broken").unwrap();
        assert!(store.load_outbox(&id).is_empty());
    }
}
