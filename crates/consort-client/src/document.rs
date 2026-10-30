//! A document in the right panel (Paper F1): which files the panel can
//! show, how large a file may be, and how the text comes from the cache.
//!
//! The panel shows a `.md`, `.markdown` or `.txt` file as text. The desktop
//! asks the client for the blob (the same path as Open and Save), and then
//! reads the cached file with [`read`] off the UI thread. A file that is
//! larger than [`MAX_DOCUMENT_BYTES`] never loads: the panel says so and
//! offers Open in another app. See DECISIONS 737.

use std::path::Path;

/// The largest file that the panel loads: 2 MiB. A longer text is too slow to
/// lay out as one page, and an editor of the system does it better.
pub const MAX_DOCUMENT_BYTES: u64 = 2 * 1024 * 1024;

/// The file name endings that open in the panel, in lower case.
const ENDINGS: [&str; 3] = ["md", "markdown", "txt"];

/// Whether a file with this name opens in the panel.
pub fn is_document(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ENDINGS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Whether the file is Markdown. A `.txt` file is plain text, and the panel
/// keeps its line breaks.
pub fn is_markdown(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown"))
}

/// Whether a file of `size` bytes is small enough for the panel.
pub fn fits(size: u64) -> bool {
    size <= MAX_DOCUMENT_BYTES
}

/// The message for a file that is too large. `size_label` is "12.4 MB".
pub fn too_large_message(size_label: &str) -> String {
    let limit = crate::format::file_size_label(MAX_DOCUMENT_BYTES);
    format!(
        "This file is {size_label}. The panel shows files up to {limit}. Open it in another app."
    )
}

/// The text of the bytes of a document.
pub fn from_bytes(bytes: &[u8]) -> Result<String, String> {
    if !fits(bytes.len() as u64) {
        return Err(too_large_message(&crate::format::file_size_label(
            bytes.len() as u64,
        )));
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        "This file is not text that the panel can show. Open it in another app.".to_owned()
    })
}

/// Reads the text of a cached document. This blocks, so the caller runs it
/// off the UI thread. The error is a sentence for the user.
pub fn read(path: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path)
        .map_err(|err| format!("Consort could not read the downloaded file. {err}"))?
        .len();
    if !fits(size) {
        return Err(too_large_message(&crate::format::file_size_label(size)));
    }
    let bytes = std::fs::read(path)
        .map_err(|err| format!("Consort could not read the downloaded file. {err}"))?;
    from_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_and_text_files_open_in_the_panel_and_others_do_not() {
        for name in ["a.md", "A.MD", "notes.markdown", "readme.txt", "x.y.Txt"] {
            assert!(is_document(name), "{name}");
        }
        for name in [
            "a.pdf",
            "photo.jpg",
            "md",
            "notes.md.bak",
            "archive.zip",
            "",
        ] {
            assert!(!is_document(name), "{name}");
        }
    }

    #[test]
    fn only_markdown_files_are_rendered_as_markdown() {
        assert!(is_markdown("a.md") && is_markdown("a.MARKDOWN"));
        assert!(!is_markdown("a.txt") && !is_markdown("a.pdf"));
    }

    #[test]
    fn the_size_limit_is_two_mib_and_the_message_names_the_next_step() {
        assert!(fits(MAX_DOCUMENT_BYTES));
        assert!(!fits(MAX_DOCUMENT_BYTES + 1));
        let message = too_large_message("12.4 MB");
        assert!(message.contains("12.4 MB") && message.contains("2.0 MB"));
        assert!(message.ends_with("Open it in another app."));
    }

    #[test]
    fn text_that_is_not_utf8_has_a_plain_message() {
        assert_eq!(from_bytes("Héllo".as_bytes()).as_deref(), Ok("Héllo"));
        let err = from_bytes(&[0xff, 0xfe, 0x00]).unwrap_err();
        assert!(err.contains("not text"), "{err}");
    }

    #[test]
    fn read_gives_the_text_and_refuses_a_large_or_missing_file() {
        let dir = std::env::temp_dir().join(format!("consort-document-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp folder");
        let small = dir.join("small.md");
        std::fs::write(&small, "# Title\n\nText.").expect("write");
        assert_eq!(read(&small).as_deref(), Ok("# Title\n\nText."));
        let large = dir.join("large.md");
        std::fs::write(&large, vec![b'a'; MAX_DOCUMENT_BYTES as usize + 1]).expect("write");
        assert!(read(&large).unwrap_err().contains("Open it in another app"));
        let missing = read(&dir.join("missing.md")).unwrap_err();
        assert!(missing.starts_with("Consort could not read"), "{missing}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
