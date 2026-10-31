//! The Ed25519 key of a session host or a worker.
//!
//! The key file holds the 32-byte secret key as lowercase hex. On Unix it
//! has mode 0600. This build keeps the key in a file, not in the OS
//! keychain.

use std::path::Path;

use consort_proto::auth::{PublicKey, Signature};
use ed25519_dalek::{Signer, SigningKey};

use crate::LinkError;

/// The signing key of this machine.
#[derive(Clone)]
pub struct Identity {
    key: SigningKey,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity({})", self.public_key())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 {
        return None;
    }
    let mut out = [0_u8; 32];
    for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

impl Identity {
    /// A new random key.
    pub fn generate() -> Self {
        let mut rng = rand::rngs::OsRng;
        Self {
            key: SigningKey::generate(&mut rng),
        }
    }

    /// Reads the key file.
    pub fn load(path: &Path) -> Result<Self, LinkError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| LinkError::io(format!("could not read {}", path.display()), e))?;
        let bytes = unhex(&text)
            .ok_or_else(|| LinkError::Key("the key file is not 64 hex characters".to_owned()))?;
        Ok(Self {
            key: SigningKey::from_bytes(&bytes),
        })
    }

    /// Writes the key file with mode 0600 on Unix.
    pub fn save(&self, path: &Path) -> Result<(), LinkError> {
        let text = hex(&self.key.to_bytes());
        write_private(path, text.as_bytes())
            .map_err(|e| LinkError::io(format!("could not write {}", path.display()), e))
    }

    /// Reads the key file, or makes and writes a new key.
    pub fn load_or_create(path: &Path) -> Result<Self, LinkError> {
        if path.exists() {
            return Self::load(path);
        }
        let identity = Self::generate();
        identity.save(path)?;
        Ok(identity)
    }

    /// The public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey::from_bytes(self.key.verifying_key().to_bytes())
    }

    /// Signs bytes.
    pub fn sign(&self, message: &[u8]) -> Signature {
        Signature::from_bytes(self.key.sign(message).to_bytes())
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // An existing file keeps its mode, so set it again.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // On Windows, the file inherits the ACL of the user's profile folder,
    // which only the user and administrators can read.
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Verifier, VerifyingKey};

    #[test]
    fn key_round_trips_and_signs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.key");
        let a = Identity::load_or_create(&path).unwrap();
        let b = Identity::load_or_create(&path).unwrap();
        assert_eq!(a.public_key(), b.public_key());
        let sig = a.sign(b"hello");
        let vk = VerifyingKey::from_bytes(a.public_key().as_bytes()).unwrap();
        vk.verify(
            b"hello",
            &ed25519_dalek::Signature::from_bytes(sig.as_bytes()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(&path, "nope").unwrap();
        assert!(Identity::load(&path).is_err());
    }
}
