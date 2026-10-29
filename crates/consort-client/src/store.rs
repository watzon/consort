//! Where a paired client keeps its hub URL, client ID and private key: the
//! OS config folder, `<config dir>/consort/`. The key file has mode 0600 on
//! Unix.

use std::path::{Path, PathBuf};

use consort_proto::ids::ClientId;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use crate::ClientError;

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

    /// The saved identity, or `None` when this client is not paired.
    pub fn load(&self) -> Result<Option<Identity>, ClientError> {
        let config_path = self.config_path();
        let key_path = self.key_path();
        if !config_path.exists() || !key_path.exists() {
            return Ok(None);
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

    /// Saves the identity. The key file is written with mode 0600 on Unix.
    pub fn save(&self, identity: &Identity) -> Result<(), ClientError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| storage_error(&self.dir, e))?;
        let key_path = self.key_path();
        write_private(&key_path, &encode_key(&identity.key.to_bytes()))
            .map_err(|e| storage_error(&key_path, e))?;
        let config_path = self.config_path();
        let text = serde_json::to_string_pretty(&identity.config)
            .map_err(|e| storage_error(&config_path, e))?;
        std::fs::write(&config_path, text).map_err(|e| storage_error(&config_path, e))?;
        Ok(())
    }

    /// Removes the identity, for "pair again".
    pub fn clear(&self) -> Result<(), ClientError> {
        for path in [self.config_path(), self.key_path()] {
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| storage_error(&path, e))?;
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // The mode applies only when the file is new, so set it again.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(contents.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    // On Windows the file inherits the ACL of the user's config folder, which
    // only that user can read.
    std::fs::write(path, contents)
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
    fn the_key_file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.save(&identity()).unwrap();
        let mode = std::fs::metadata(dir.path().join("client.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
