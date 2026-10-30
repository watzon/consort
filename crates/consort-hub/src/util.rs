//! Small helpers: hex, hashes, random bytes and pairing code hashing.

use consort_proto::auth::PairingCode;
use consort_proto::model::Sha256Hash;
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

const HEX: &[u8; 16] = b"0123456789abcdef";

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 15)]));
    }
    out
}

pub fn sha256(bytes: &[u8]) -> Sha256Hash {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    Sha256Hash::from_bytes(digest)
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0_u8; N];
    OsRng.fill_bytes(&mut out);
    out
}

pub fn random_pairing_code() -> PairingCode {
    PairingCode::from_random_bytes(random_bytes::<8>())
}

/// The hub stores pairing codes as a hash, so a copy of the database does
/// not hold a usable code.
pub fn pairing_code_hash(code: &PairingCode) -> String {
    hex_encode(&Sha256::digest(code.canonical().as_bytes()))
}

pub fn random_token() -> String {
    hex_encode(&random_bytes::<32>())
}

/// Opens a new file for writing that only the user can read: mode 0600 on
/// Unix. A file that exists is cut to zero bytes.
pub async fn create_private_file(path: &std::path::Path) -> std::io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).await
}

/// Opens a new file for writing that only the user can read (mode 0600 on
/// Unix), in blocking code. It fails when the file exists.
pub fn create_new_private_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Writes a file that only the user can read, through a temp file in the
/// same folder and a rename, so a reader sees the old file or the new one
/// and never half of it.
pub fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> crate::error::Result<()> {
    use std::io::Write;
    let temp = path.with_extension(format!("tmp-{}", &random_token()[..12]));
    let result = (|| {
        let mut file = create_new_private_file(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    Ok(result?)
}

/// Cuts text to at most `max_chars` characters and adds an ellipsis.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// A time limit as words for a message: "2 minutes", "30 seconds" or
/// "250 milliseconds".
pub fn duration_text(limit: std::time::Duration) -> String {
    let (n, unit) = match limit.as_secs() {
        0 => (limit.as_millis().max(1), "millisecond"),
        s if s % 60 == 0 => (u128::from(s / 60), "minute"),
        s => (u128::from(s), "second"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_read_as_words() {
        use std::time::Duration;
        assert_eq!(
            duration_text(Duration::from_millis(250)),
            "250 milliseconds"
        );
        assert_eq!(duration_text(Duration::from_secs(1)), "1 second");
        assert_eq!(duration_text(Duration::from_secs(90)), "90 seconds");
        assert_eq!(duration_text(Duration::from_secs(120)), "2 minutes");
    }

    #[test]
    fn sha256_of_empty_input() {
        assert_eq!(
            sha256(b"").as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn truncation_counts_characters() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("abcdef", 4), "abc\u{2026}");
        assert_eq!(truncate_chars("\u{e9}\u{e9}\u{e9}", 2), "\u{e9}\u{2026}");
    }
}
