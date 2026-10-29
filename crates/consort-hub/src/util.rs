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

/// Cuts text to at most `max_chars` characters and adds an ellipsis.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
