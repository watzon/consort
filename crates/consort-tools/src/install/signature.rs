//! OpenPGP checks for the Claude Code release manifest.

use std::io::Cursor;

use pgp::composed::{Deserializable, DetachedSignature, SignedPublicKey};

use crate::ToolError;

/// The Anthropic Claude Code release key, from
/// `https://downloads.claude.ai/keys/claude-code.asc`.
pub(crate) const CLAUDE_CODE_KEY: &str = include_str!("../../keys/claude-code.asc");

/// The fingerprint of [`CLAUDE_CODE_KEY`]
/// (`31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF 1A7E CACE`).
#[cfg(test)]
pub(crate) const CLAUDE_CODE_FINGERPRINT: &str = "31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE";

fn signature_error(err: impl std::fmt::Display) -> ToolError {
    ToolError::Signature {
        message: err.to_string(),
    }
}

/// Reads an armored public key and checks its self-signatures.
fn read_key(armored: &str) -> Result<SignedPublicKey, ToolError> {
    let (key, _) = SignedPublicKey::from_armor_single(Cursor::new(armored.as_bytes()))
        .map_err(signature_error)?;
    key.verify_bindings().map_err(signature_error)?;
    Ok(key)
}

/// The upper-case hex fingerprint of the primary key.
#[cfg(test)]
pub(crate) fn fingerprint(armored: &str) -> Result<String, ToolError> {
    use pgp::types::KeyDetails;
    let key = read_key(armored)?;
    Ok(format!("{:X}", key.primary_key.fingerprint()))
}

/// Checks a detached signature (armored or binary) over `data` with the
/// primary key or a bound subkey of `armored_key`.
pub(crate) fn verify_detached(
    armored_key: &str,
    data: &[u8],
    signature: &[u8],
) -> Result<(), ToolError> {
    let key = read_key(armored_key)?;
    let signature = if signature.starts_with(b"-----BEGIN") {
        DetachedSignature::from_armor_single(Cursor::new(signature))
            .map(|(sig, _)| sig)
            .map_err(signature_error)?
    } else {
        DetachedSignature::from_bytes(Cursor::new(signature)).map_err(signature_error)?
    };
    if signature.verify(&key.primary_key, data).is_ok() {
        return Ok(());
    }
    for subkey in &key.public_subkeys {
        if signature.verify(&subkey.key, data).is_ok() {
            return Ok(());
        }
    }
    Err(ToolError::Signature {
        message: "the signature does not match the data or the key".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &[u8] = include_bytes!("../../tests/fixtures/claude-2.1.308-manifest.json");
    const MANIFEST_SIG: &[u8] =
        include_bytes!("../../tests/fixtures/claude-2.1.308-manifest.json.sig");

    #[test]
    fn embedded_key_has_the_published_fingerprint() {
        assert_eq!(
            fingerprint(CLAUDE_CODE_KEY).unwrap(),
            CLAUDE_CODE_FINGERPRINT
        );
    }

    #[test]
    fn real_manifest_signature_verifies() {
        verify_detached(CLAUDE_CODE_KEY, MANIFEST, MANIFEST_SIG).unwrap();
    }

    #[test]
    fn changed_manifest_fails() {
        let mut changed = MANIFEST.to_vec();
        let index = changed.iter().position(|b| *b == b'2').unwrap();
        changed[index] = b'3';
        let result = verify_detached(CLAUDE_CODE_KEY, &changed, MANIFEST_SIG);
        assert!(
            matches!(result, Err(ToolError::Signature { .. })),
            "{result:?}"
        );
    }
}
