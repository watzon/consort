//! The choices of Settings > Dictation, and the Deepgram key. Both are for this
//! computer. They never go to the hub (plan, decision 10).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The shortcut until the user changes it, in the form of a GPUI keystroke.
pub const DEFAULT_SHORTCUT: &str = "ctrl-alt-d";
/// The catalog id of Cleanup 1 by Spokenly, the default cleanup model.
const DEFAULT_CLEANUP: &str = "spokenly-cleanup-1";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot read or write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot write the dictation settings: {0}")]
    Encode(#[from] toml::ser::Error),
}

/// Who writes the final text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    #[default]
    Local,
    Deepgram,
}

/// How long a silence ends dictation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopAfter {
    S15,
    #[default]
    S30,
    S60,
    Never,
}

impl StopAfter {
    pub const ALL: [StopAfter; 4] = [
        StopAfter::S15,
        StopAfter::S30,
        StopAfter::S60,
        StopAfter::Never,
    ];

    pub fn limit(self) -> Option<Duration> {
        match self {
            StopAfter::S15 => Some(Duration::from_secs(15)),
            StopAfter::S30 => Some(Duration::from_secs(30)),
            StopAfter::S60 => Some(Duration::from_secs(60)),
            StopAfter::Never => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StopAfter::S15 => "15 seconds",
            StopAfter::S30 => "30 seconds",
            StopAfter::S60 => "60 seconds",
            StopAfter::Never => "Never",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub engine: Engine,
    pub cleanup_on: bool,
    /// A catalog id of `sayso-cleanup-models`.
    pub cleanup_model: String,
    pub stop_after: StopAfter,
    /// A language code such as `en`.
    pub language: String,
    /// An input device id. None: the default device of the system.
    pub microphone: Option<String>,
    pub shortcut: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            engine: Engine::Local,
            cleanup_on: true,
            cleanup_model: DEFAULT_CLEANUP.into(),
            stop_after: StopAfter::S30,
            language: "en".into(),
            microphone: None,
            shortcut: DEFAULT_SHORTCUT.into(),
        }
    }
}

/// The settings file.
#[derive(Clone, Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(dir: &Path) -> SettingsStore {
        SettingsStore {
            path: dir.join("dictation.toml"),
        }
    }

    /// The settings on disk. A missing or broken file gives the defaults.
    pub fn load(&self) -> Settings {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, settings: &Settings) -> Result<(), StoreError> {
        let text = toml::to_string_pretty(settings)?;
        write_file(&self.path, text.as_bytes(), false)
    }
}

/// The Deepgram key, in a file that only this user can read.
#[derive(Clone, Debug)]
pub struct KeyStore {
    path: PathBuf,
}

impl KeyStore {
    pub fn new(dir: &Path) -> KeyStore {
        KeyStore {
            path: dir.join("deepgram.key"),
        }
    }

    pub fn load(&self) -> Option<String> {
        let key = std::fs::read_to_string(&self.path).ok()?;
        let key = key.trim();
        (!key.is_empty()).then(|| key.to_owned())
    }

    pub fn save(&self, key: &str) -> Result<(), StoreError> {
        write_file(&self.path, key.trim().as_bytes(), true)
    }

    pub fn remove(&self) -> Result<(), StoreError> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(StoreError::Io {
                path: self.path.clone(),
                source: e,
            }),
            _ => Ok(()),
        }
    }
}

fn write_file(path: &Path, bytes: &[u8], private: bool) -> Result<(), StoreError> {
    use std::io::Write as _;
    let io = |source| StoreError::Io {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let mut file = open(path, private).map_err(io)?;
    file.write_all(bytes).map_err(io)
}

#[cfg(unix)]
fn open(path: &Path, private: bool) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let mode = if private { 0o600 } else { 0o644 };
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)?;
    // The mode applies only when the file is new, so set it again.
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    Ok(file)
}

#[cfg(not(unix))]
fn open(path: &Path, _private: bool) -> std::io::Result<std::fs::File> {
    std::fs::File::create(path)
}
