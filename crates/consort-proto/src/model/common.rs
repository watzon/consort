//! Values that many entities share: hashes, blob references, model
//! selections, operating systems and validation errors.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::ErrorBody;

/// A field that failed a check, with the reason. Maps to an `invalid` error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{field}` {message}")]
pub struct ValidationError {
    /// The field name, for example `options[2].label`.
    pub field: String,
    /// What is wrong, for example `is longer than 40 characters`.
    pub message: String,
}

impl ValidationError {
    /// Makes a validation error.
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl From<ValidationError> for ErrorBody {
    fn from(err: ValidationError) -> Self {
        ErrorBody::invalid(err.to_string())
    }
}

/// A SHA-256 hash as 64 lowercase hex characters. Parsing accepts either
/// case and stores lowercase.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Hash(String);

/// The error when a string is not a SHA-256 hex hash.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a SHA-256 hash of 64 hex characters")]
pub struct HashParseError(pub String);

impl Sha256Hash {
    /// Makes a hash from the 32 raw bytes of a SHA-256 digest.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(crate::hex::encode(&bytes))
    }

    /// The 32 raw bytes.
    pub fn to_bytes(&self) -> [u8; 32] {
        // The constructor and the parser accept only 64 hex characters, so
        // decoding cannot fail.
        crate::hex::decode_array::<32>(&self.0).unwrap_or([0; 32])
    }

    /// The lowercase hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Sha256Hash {
    type Err = HashParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Self(s.to_ascii_lowercase()))
        } else {
            Err(HashParseError(s.to_owned()))
        }
    }
}

impl fmt::Display for Sha256Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Sha256Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256Hash({})", self.0)
    }
}

impl Serialize for Sha256Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Sha256Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Sha256Hash {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Sha256Hash")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[0-9a-fA-F]{64}$",
            "description": "A SHA-256 hash as 64 hex characters.",
        })
    }
}

/// A reference to a blob on the hub, used for message attachments and turn
/// input attachments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    /// The blob hash. The blob is at `/v1/blobs/{sha256}`.
    pub sha256: Sha256Hash,
    /// The file name to show.
    pub name: String,
    /// The media type, for example `image/png`.
    pub mime: String,
    /// The size in bytes.
    pub size: u64,
}

/// A worker's operating system.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    /// macOS.
    Macos,
    /// Windows.
    Windows,
    /// Linux.
    Linux,
}

impl Os {
    /// Maps a value of `std::env::consts::OS` to an [`Os`].
    pub fn from_std_os(os: &str) -> Option<Self> {
        match os {
            "macos" => Some(Self::Macos),
            "windows" => Some(Self::Windows),
            "linux" => Some(Self::Linux),
            _ => None,
        }
    }

    /// The wire string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Linux => "linux",
        }
    }
}

/// The value of one model option: a select value or a boolean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OptionValue {
    /// The value of a boolean option.
    Bool(bool),
    /// The value of a select option.
    Text(String),
}

/// Model options by option ID, for example `{"effort": "high"}`.
pub type ModelOptions = BTreeMap<String, OptionValue>;

/// A harness account label, a model and its options, such as
/// "Claude Personal, Opus 5.5, high effort". Placement turns it into a worker
/// and an account ID on that worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelSelection {
    /// The harness account label, for example "Claude Personal".
    pub account_label: String,
    /// The model ID, as the harness account reports it.
    pub model: String,
    /// Option values by option ID.
    #[serde(default)]
    pub options: ModelOptions,
}

/// Checks that a path is a safe relative path: not empty, at most 512
/// bytes, no leading `/`, no backslash, no empty, `.` or `..` segment, and no
/// control character. Memory note paths and channel file paths use this.
pub fn validate_relative_path(field: &str, path: &str) -> Result<(), ValidationError> {
    if path.is_empty() {
        return Err(ValidationError::new(field, "is empty"));
    }
    if path.len() > 512 {
        return Err(ValidationError::new(field, "is longer than 512 bytes"));
    }
    if path.contains('\\') || path.chars().any(char::is_control) {
        return Err(ValidationError::new(
            field,
            "contains a backslash or a control character",
        ));
    }
    if path.starts_with('/') {
        return Err(ValidationError::new(field, "must be a relative path"));
    }
    if path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(ValidationError::new(
            field,
            "has an empty, `.` or `..` segment",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_parses_and_normalizes() {
        let upper = "A".repeat(64);
        let hash: Sha256Hash = upper.parse().unwrap();
        assert_eq!(hash.as_str(), "a".repeat(64));
        assert_eq!(hash.to_bytes(), [0xaa; 32]);
        assert_eq!(Sha256Hash::from_bytes([0xaa; 32]), hash);
        assert!("abc".parse::<Sha256Hash>().is_err());
        assert!("g".repeat(64).parse::<Sha256Hash>().is_err());
        assert!(serde_json::from_str::<Sha256Hash>("\"xyz\"").is_err());
    }

    #[test]
    fn model_selection_shape() {
        let json = serde_json::json!({
            "account_label": "Claude Personal",
            "model": "opus-5.5",
            "options": {"effort": "high", "thinking": true}
        });
        let sel: ModelSelection = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(sel.options.get("thinking"), Some(&OptionValue::Bool(true)));
        assert_eq!(serde_json::to_value(&sel).unwrap(), json);
        let no_options: ModelSelection =
            serde_json::from_str(r#"{"account_label":"a","model":"m"}"#).unwrap();
        assert!(no_options.options.is_empty());
    }

    #[test]
    fn relative_paths() {
        for ok in ["people/kevin.md", "a.md", "artifacts/tsk_x/report.pdf"] {
            assert!(validate_relative_path("path", ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "a/./b",
            "a\\b",
            "a/",
            "a\nb",
        ] {
            assert!(validate_relative_path("path", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn os_strings() {
        assert_eq!(Os::from_std_os("macos"), Some(Os::Macos));
        assert_eq!(Os::from_std_os("freebsd"), None);
        assert_eq!(serde_json::to_string(&Os::Windows).unwrap(), "\"windows\"");
    }
}
