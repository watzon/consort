//! Where an attachment or a channel file goes in a workspace. The worker writes
//! the files and the harness adapters name them, so they share these rules.

use std::path::{Path, PathBuf};

use crate::model::BlobRef;

/// The folder in the workspace for the attachments of the session's turns.
pub const ATTACHMENTS_DIR: &str = ".consort/attachments";
/// The folder in the workspace with the read-only copy of the channel files.
pub const CHANNEL_FILES_DIR: &str = ".channel/files";
/// The most bytes of a file name after [`sanitize_file_name`]. The hash
/// prefix comes on top.
pub const NAME_MAX_BYTES: usize = 100;
/// The number of hex characters of the SHA-256 at the start of a saved
/// attachment's file name.
pub const HASH_PREFIX_LEN: usize = 12;

/// Makes a file name from a name that a client or the hub gave. The result
/// has no path separator, no control character and no character that
/// Windows refuses in a name. It does not start or end with a dot or a
/// space, so it cannot be `.` or `..` or a hidden file. It has at most
/// [`NAME_MAX_BYTES`] bytes, and it keeps a short extension when it cuts.
pub fn sanitize_file_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = replaced.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let name = if trimmed.is_empty() { "file" } else { trimmed };
    if name.len() <= NAME_MAX_BYTES {
        return name.to_owned();
    }
    // Keep an extension of up to 16 bytes, and cut the stem.
    let ext = name
        .rfind('.')
        .map(|i| &name[i..])
        .filter(|ext| ext.len() <= 16 && ext.len() > 1)
        .unwrap_or("");
    let room = NAME_MAX_BYTES - ext.len();
    let mut end = room.min(name.len() - ext.len());
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let stem = name[..end].trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    format!("{stem}{ext}")
}

/// The file name of a saved attachment: the start of its hash, then its
/// sanitized name. Two attachments with the same name do not collide, and
/// the name is never a Windows device name such as `CON`.
pub fn attachment_file_name(blob: &BlobRef) -> String {
    let hash = blob.sha256.as_str();
    let prefix = &hash[..HASH_PREFIX_LEN.min(hash.len())];
    format!("{prefix}-{}", sanitize_file_name(&blob.name))
}

/// Where the worker saves an attachment in a workspace.
pub fn attachment_path(workspace: &Path, blob: &BlobRef) -> PathBuf {
    workspace
        .join(ATTACHMENTS_DIR)
        .join(attachment_file_name(blob))
}

/// A size for people: bytes, KB, MB or GB with one decimal.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    if bytes < 1024 {
        return if bytes == 1 {
            "1 byte".to_owned()
        } else {
            format!("{bytes} bytes")
        };
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = UNITS[0];
    for next in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    format!("{value:.1} {unit}")
}
