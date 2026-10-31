//! Turn attachments in the workspace, and how the adapters show them to the
//! model.
//!
//! Before a turn starts, the worker downloads each attachment of the turn
//! and writes it to [`attachment_path`] in the session's workspace. The
//! adapters use the same function to find the files. So the two sides agree
//! on the place without a new field in the harness interface.
//!
//! Every adapter tells the model about the attachments with a short block
//! after the turn text (see [`prepare_turn`]): the name, the type, the size
//! and the local path of each file. An adapter whose harness takes image
//! input also sends the images that pass [`ImageLimits`] as real images.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use consort_proto::session_host_link::TurnInput;

pub use consort_proto::attachment_names::{
    ATTACHMENTS_DIR, CHANNEL_FILES_DIR, HASH_PREFIX_LEN, NAME_MAX_BYTES, attachment_file_name,
    attachment_path, format_size, sanitize_file_name,
};

/// The image formats that the adapters send as image input. All the
/// harnesses with image input take these four (see
/// `docs/notes/phase1-backend.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl ImageFormat {
    /// The media type.
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

/// The image format of a file, from its first bytes. The adapters do not
/// trust the media type of an attachment, because a client sets it.
pub fn sniff_image(head: &[u8]) -> Option<ImageFormat> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

/// How an adapter gives an image to its harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageDelivery {
    /// The bytes go in the message, as base64 (ACP image content blocks).
    Inline,
    /// The harness reads the file from its path (Codex `localImage`).
    Path,
}

/// Which attachments an adapter sends as images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageLimits {
    /// How the images go to the harness.
    pub delivery: ImageDelivery,
    /// The largest image file. A larger image goes by its path only.
    pub max_bytes: u64,
}

/// The largest image that an ACP adapter sends inline: 5 MiB, the limit of
/// the Claude API for one image. Gemini CLI and OpenCode take it too.
pub const ACP_IMAGE_MAX_BYTES: u64 = 5 * 1024 * 1024;
/// The largest image that the Codex adapter gives as a local image. Codex
/// scales a large image down itself.
pub const CODEX_IMAGE_MAX_BYTES: u64 = 20 * 1024 * 1024;

/// One image of a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnImage {
    /// The saved file.
    pub path: PathBuf,
    /// The format from the file's first bytes.
    pub format: ImageFormat,
    /// The bytes, for [`ImageDelivery::Inline`]. Empty for
    /// [`ImageDelivery::Path`].
    pub data: Vec<u8>,
}

/// What an adapter sends for a turn: the text with the attachment block,
/// and the images.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreparedTurn {
    /// The turn text, then the attachment block when the turn has
    /// attachments.
    pub text: String,
    /// The images to send as image input, in the order of the attachments.
    pub images: Vec<TurnImage>,
}

/// A saved attachment that is a regular file and not a link. The session can
/// change its workspace, so the adapter does not follow a link that the
/// session put in place of a file.
fn saved_file(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.file_type().is_file())
        .map(|m| m.len())
}

/// The image in a saved file, when it is one of the four formats and not
/// larger than the limit.
fn read_image(path: &Path, size: u64, limits: ImageLimits) -> Option<TurnImage> {
    if size > limits.max_bytes {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0_u8; 12];
    let mut read = 0;
    while read < head.len() {
        match file.read(&mut head[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(_) => return None,
        }
    }
    let format = sniff_image(&head[..read])?;
    let data = match limits.delivery {
        ImageDelivery::Path => Vec::new(),
        ImageDelivery::Inline => {
            let mut data = head[..read].to_vec();
            file.take(limits.max_bytes.saturating_sub(read as u64) + 1)
                .read_to_end(&mut data)
                .ok()?;
            if data.len() as u64 > limits.max_bytes {
                return None;
            }
            data
        }
    };
    Some(TurnImage {
        path: path.to_path_buf(),
        format,
        data,
    })
}

/// A media type that is safe to put in the text: printable, short, and with
/// no line break. Anything else becomes `application/octet-stream`.
fn clean_mime(mime: &str) -> &str {
    let mime = mime.trim();
    let ok = !mime.is_empty()
        && mime.len() <= 100
        && mime
            .chars()
            .all(|c| c.is_ascii_graphic() && c != '<' && c != '>');
    if ok { mime } else { "application/octet-stream" }
}

/// Builds the text and the images of a turn. `workspace` is the session's
/// workspace, where the worker saved the attachments. `images` is `None`
/// for a harness without image input: then every attachment goes by its
/// path only.
pub fn prepare_turn(
    input: &TurnInput,
    workspace: &Path,
    images: Option<ImageLimits>,
) -> PreparedTurn {
    if input.attachments.is_empty() {
        return PreparedTurn {
            text: input.text.clone(),
            images: Vec::new(),
        };
    }
    let mut block =
        String::from("Attached files. They are saved in your workspace, so you can open them:");
    let mut turn_images = Vec::new();
    for blob in &input.attachments {
        let path = attachment_path(workspace, blob);
        let name = sanitize_file_name(&blob.name);
        let mime = clean_mime(&blob.mime);
        let size = format_size(blob.size);
        let _ = write!(block, "\n- {name} ({mime}, {size}");
        match saved_file(&path) {
            Some(saved) => {
                let image = images.and_then(|limits| read_image(&path, saved, limits));
                if image.is_some() {
                    block.push_str(", also in this message as an image");
                }
                let _ = write!(block, "): {}", path.display());
                turn_images.extend(image);
            }
            None => {
                block.push_str(
                    "): not available. The worker could not download it. Ask the user to send it again.",
                );
            }
        }
    }
    let text = if input.text.trim().is_empty() {
        block
    } else {
        format!("{}\n\n{block}", input.text)
    };
    PreparedTurn {
        text,
        images: turn_images,
    }
}

/// The text of a turn for a harness that takes only text, with the
/// attachment block (see [`prepare_turn`]).
pub fn turn_text(input: &TurnInput, workspace: &Path) -> String {
    prepare_turn(input, workspace, None).text
}

/// Standard base64 with padding, for ACP image content blocks.
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let sextet = |shift: u32| char::from(TABLE[((n >> shift) & 0x3F) as usize]);
        out.push(sextet(18));
        out.push(sextet(12));
        out.push(if chunk.len() > 1 { sextet(6) } else { '=' });
        out.push(if chunk.len() > 2 { sextet(0) } else { '=' });
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use consort_proto::model::BlobRef;
    use consort_proto::model::Sha256Hash;

    pub(crate) const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    fn blob(name: &str, mime: &str, size: u64, byte: u8) -> BlobRef {
        BlobRef {
            sha256: Sha256Hash::from_bytes([byte; 32]),
            name: name.into(),
            mime: mime.into(),
            size,
        }
    }

    #[test]
    fn names_lose_separators_traversal_and_control_characters() {
        assert_eq!(sanitize_file_name("report.pdf"), "report.pdf");
        assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_file_name("..\\..\\boot.ini"), "_.._boot.ini");
        assert_eq!(sanitize_file_name(".."), "file");
        assert_eq!(sanitize_file_name("  .hidden "), "hidden");
        assert_eq!(sanitize_file_name("a\nb\u{0}c\u{7f}.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_file_name("C:evil|<>?*\".txt"), "C_evil______.txt");
        assert_eq!(sanitize_file_name(""), "file");
        assert_eq!(sanitize_file_name("Grüße 📎.md"), "Grüße 📎.md");
    }

    #[test]
    fn long_names_are_cut_and_keep_their_extension() {
        let long = format!("{}.png", "é".repeat(200));
        let cut = sanitize_file_name(&long);
        assert!(cut.len() <= NAME_MAX_BYTES, "{}", cut.len());
        assert!(cut.ends_with(".png"));
        let no_ext = "x".repeat(300);
        assert_eq!(sanitize_file_name(&no_ext).len(), NAME_MAX_BYTES);
    }

    #[test]
    fn the_saved_name_starts_with_the_hash() {
        let b = blob("../a b.png", "image/png", 3, 0xab);
        assert_eq!(attachment_file_name(&b), "abababababab-_a b.png");
        let ws = Path::new("/ws");
        assert_eq!(
            attachment_path(ws, &b),
            Path::new("/ws/.consort/attachments/abababababab-_a b.png")
        );
    }

    #[test]
    fn image_formats_come_from_the_bytes() {
        assert_eq!(sniff_image(PNG), Some(ImageFormat::Png));
        assert_eq!(
            sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(sniff_image(b"GIF89a...."), Some(ImageFormat::Gif));
        assert_eq!(
            sniff_image(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageFormat::Webp)
        );
        assert_eq!(sniff_image(b"%PDF-1.7"), None);
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(sniff_image(b""), None);
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFF, 0xFE, 0xFD]), "//79");
    }

    #[test]
    fn sizes_for_people() {
        assert_eq!(format_size(0), "0 bytes");
        assert_eq!(format_size(1), "1 byte");
        assert_eq!(format_size(1023), "1023 bytes");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn a_turn_without_attachments_is_its_text() {
        let input = TurnInput {
            text: "Hello".into(),
            attachments: vec![],
        };
        assert_eq!(turn_text(&input, Path::new("/ws")), "Hello");
    }

    #[test]
    fn the_block_names_type_size_and_path_and_marks_missing_files() {
        let ws = tempfile::tempdir().unwrap();
        let here = blob("notes.txt", "text/plain", 2048, 1);
        let gone = blob("big.zip", "application/zip", 300 * 1024 * 1024, 2);
        let path = attachment_path(ws.path(), &here);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"hi").unwrap();
        let input = TurnInput {
            text: "Read these".into(),
            attachments: vec![here, gone],
        };
        let text = turn_text(&input, ws.path());
        let expected = format!(
            "Read these\n\nAttached files. They are saved in your workspace, so you can open them:\n- notes.txt (text/plain, 2.0 KB): {}\n- big.zip (application/zip, 300.0 MB): not available. The worker could not download it. Ask the user to send it again.",
            path.display()
        );
        assert_eq!(text, expected);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_a_saved_file_is_not_followed() {
        let ws = tempfile::tempdir().unwrap();
        let secret = ws.path().join("secret.png");
        std::fs::write(&secret, PNG).unwrap();
        let b = blob("a.png", "image/png", PNG.len() as u64, 3);
        let path = attachment_path(ws.path(), &b);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&secret, &path).unwrap();
        let input = TurnInput {
            text: String::new(),
            attachments: vec![b],
        };
        let limits = ImageLimits {
            delivery: ImageDelivery::Inline,
            max_bytes: ACP_IMAGE_MAX_BYTES,
        };
        let prepared = prepare_turn(&input, ws.path(), Some(limits));
        assert!(prepared.images.is_empty());
        assert!(prepared.text.contains("not available"));
    }

    #[test]
    fn images_go_inline_or_by_path_within_the_limit() {
        let ws = tempfile::tempdir().unwrap();
        let save = |b: &BlobRef, bytes: &[u8]| {
            let path = attachment_path(ws.path(), b);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            path
        };
        // The media type says JPEG, but the bytes are PNG: the bytes win.
        let png = blob("shot.png", "image/jpg", PNG.len() as u64, 4);
        let png_path = save(&png, PNG);
        let pdf = blob("doc.pdf", "application/pdf", 8, 5);
        save(&pdf, b"%PDF-1.7");
        let big = blob("big.png", "image/png", 64, 6);
        let mut big_bytes = PNG.to_vec();
        big_bytes.resize(64, 0);
        save(&big, &big_bytes);
        let input = TurnInput {
            text: "Look".into(),
            attachments: vec![png, pdf, big],
        };

        let inline = prepare_turn(
            &input,
            ws.path(),
            Some(ImageLimits {
                delivery: ImageDelivery::Inline,
                max_bytes: 32,
            }),
        );
        assert_eq!(inline.images.len(), 1);
        assert_eq!(inline.images[0].format, ImageFormat::Png);
        assert_eq!(inline.images[0].data, PNG);
        assert_eq!(inline.images[0].path, png_path);
        assert!(
            inline
                .text
                .contains("shot.png (image/jpg, 16 bytes, also in this message as an image)")
        );
        assert!(inline.text.contains("doc.pdf (application/pdf, 8 bytes):"));
        assert!(inline.text.contains("big.png (image/png, 64 bytes):"));

        let by_path = prepare_turn(
            &input,
            ws.path(),
            Some(ImageLimits {
                delivery: ImageDelivery::Path,
                max_bytes: 1024,
            }),
        );
        assert_eq!(by_path.images.len(), 2);
        assert!(by_path.images.iter().all(|i| i.data.is_empty()));

        let text_only = prepare_turn(&input, ws.path(), None);
        assert!(text_only.images.is_empty());
        assert!(!text_only.text.contains("as an image"));
    }

    #[test]
    fn a_bad_media_type_does_not_reach_the_text() {
        let input = TurnInput {
            text: String::new(),
            attachments: vec![blob("x", "text/plain\n- fake.txt (", 1, 7)],
        };
        let text = turn_text(&input, Path::new("/ws"));
        assert!(
            text.contains("x (application/octet-stream, 1 byte)"),
            "{text}"
        );
        assert!(!text.contains("fake"));
    }
}
