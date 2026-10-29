//! The Composio key at rest.
//!
//! The hub makes a random 256-bit key once and keeps it in `hub-secret.key`
//! in its data folder. `composio.key` holds the Composio key, encrypted with
//! AES-256-GCM under that key. Both files have mode 0600 on Unix. A copy of
//! `hub.db` or of `composio.key` alone does not give the Composio key.
//!
//! File format of `composio.key`: the 4 bytes `CSK1`, a 12-byte nonce, then
//! the ciphertext with its tag.

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};

use super::composio::ApiKey;
use crate::error::{HubError, Result};
use crate::util::random_bytes;

const MAGIC: &[u8; 4] = b"CSK1";
const NONCE_BYTES: usize = 12;
const SECRET_FILE: &str = "hub-secret.key";
const KEY_FILE: &str = "composio.key";
/// Binds the ciphertext to its use.
const AAD: &[u8] = b"consort composio.key v1";

/// Reads and writes the encrypted Composio key.
#[derive(Debug, Clone)]
pub(crate) struct KeyStore {
    dir: PathBuf,
}

impl KeyStore {
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    fn secret_path(&self) -> PathBuf {
        self.dir.join(SECRET_FILE)
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join(KEY_FILE)
    }

    /// The hub's own key. It makes one when there is none.
    fn secret(&self) -> Result<[u8; 32]> {
        let path = self.secret_path();
        match std::fs::read(&path) {
            Ok(bytes) => bytes
                .try_into()
                .map_err(|_| HubError::internal(format!("{} is damaged", path.display()))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let secret = random_bytes::<32>();
                write_private(&path, &secret)?;
                Ok(secret)
            }
            Err(err) => Err(err.into()),
        }
    }

    fn cipher(&self) -> Result<Aes256Gcm> {
        let secret = self.secret()?;
        Aes256Gcm::new_from_slice(&secret).map_err(|_| HubError::internal("bad key length"))
    }

    /// Encrypts and stores the key.
    pub(crate) fn save(&self, key: &ApiKey) -> Result<()> {
        let cipher = self.cipher()?;
        let nonce = random_bytes::<NONCE_BYTES>();
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: key.expose().as_bytes(),
                    aad: AAD,
                },
            )
            .map_err(|_| HubError::internal("could not encrypt the Composio key"))?;
        let mut out = Vec::with_capacity(MAGIC.len() + NONCE_BYTES + sealed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        write_private(&self.key_path(), &out)
    }

    /// The stored key, or `None`.
    pub(crate) fn load(&self) -> Result<Option<ApiKey>> {
        let bytes = match std::fs::read(self.key_path()) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let damaged = || {
            HubError::internal(
                "composio.key cannot be read. Remove the key in Settings > Connectors and add it again.",
            )
        };
        if bytes.len() <= MAGIC.len() + NONCE_BYTES || &bytes[..MAGIC.len()] != MAGIC {
            return Err(damaged());
        }
        let (nonce, sealed) = bytes[MAGIC.len()..].split_at(NONCE_BYTES);
        let plain = self
            .cipher()?
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: sealed,
                    aad: AAD,
                },
            )
            .map_err(|_| damaged())?;
        let text = String::from_utf8(plain).map_err(|_| damaged())?;
        Ok(Some(ApiKey::new(text)))
    }

    /// Deletes the stored key. The hub's own key stays.
    pub(crate) fn remove(&self) -> Result<()> {
        match std::fs::remove_file(self.key_path()) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

/// Writes a file that only the user can read, through a temp file and a
/// rename, so a crash never leaves half a file.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let temp = path.with_extension(format!("tmp-{}", &crate::util::random_token()[..12]));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_round_trips_and_is_not_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::new(dir.path());
        assert!(store.load().unwrap().is_none());
        store
            .save(&ApiKey::new("ak_plain_text_key".to_owned()))
            .unwrap();
        let raw = std::fs::read(dir.path().join(KEY_FILE)).unwrap();
        assert!(!raw.windows(5).any(|w| w == b"plain"));
        assert_eq!(store.load().unwrap().unwrap().expose(), "ak_plain_text_key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in [KEY_FILE, SECRET_FILE] {
                let mode = std::fs::metadata(dir.path().join(name))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o600, "{name}");
            }
        }
        store.remove().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn another_hub_secret_cannot_read_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::new(dir.path());
        store.save(&ApiKey::new("ak_1".to_owned())).unwrap();
        std::fs::remove_file(dir.path().join(SECRET_FILE)).unwrap();
        assert!(store.load().is_err());
    }
}
