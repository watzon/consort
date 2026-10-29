//! Size limits that the client checks before it sends text, so the hub never
//! gets a frame it must refuse and the link never breaks on a frame that is
//! too large.
//!
//! The limit and the way to measure a text, [`MESSAGE_BODY_MAX_BYTES`] and
//! [`json_len`], come from `consort-proto`. The hub uses the same ones, so a
//! body that passes here passes there.

use consort_proto::frame::{MESSAGE_BODY_MAX_BYTES, json_len};

use crate::ClientError;

/// Refuses a message body that the hub would refuse. The error says how
/// large the text is, what the limit is and what to do. The caller keeps
/// its draft.
pub fn check_message_body(body: &str) -> Result<(), ClientError> {
    let size = json_len(body);
    if size <= MESSAGE_BODY_MAX_BYTES {
        return Ok(());
    }
    Err(ClientError::Hub {
        code: "invalid".into(),
        message: format!(
            "The message has {} KiB, and the limit is {} KiB. Shorten it, or send it in several messages. Your text is not lost",
            size.div_ceil(1024),
            MESSAGE_BODY_MAX_BYTES / 1024
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_len_counts_escapes_like_the_hub() {
        assert_eq!(json_len("abc"), 3);
        assert_eq!(json_len("a\"b\\c"), 7);
        assert_eq!(json_len("\n\t"), 4);
        assert_eq!(json_len("\u{0}"), 6);
        assert_eq!(json_len("\u{e9}"), 2);
        let text = "a\"\n\u{1}\u{e9}<";
        assert_eq!(
            json_len(text) + 2,
            serde_json::to_string(text).map(|s| s.len()).unwrap_or(0)
        );
    }

    #[test]
    fn a_body_at_the_limit_passes_and_one_byte_more_does_not() {
        let at_limit = "a".repeat(MESSAGE_BODY_MAX_BYTES);
        assert!(check_message_body(&at_limit).is_ok());
        let over = "a".repeat(MESSAGE_BODY_MAX_BYTES + 1);
        let err = check_message_body(&over).unwrap_err();
        let ClientError::Hub { code, message } = err else {
            panic!("expected a hub error");
        };
        assert_eq!(code, "invalid");
        assert!(message.contains("256 KiB"), "{message}");
        assert!(message.contains("Shorten"), "{message}");
    }

    #[test]
    fn the_size_counts_escapes_not_characters() {
        // 130 000 quotes are 130 000 characters but 260 000 bytes as JSON.
        let quotes = "\"".repeat(MESSAGE_BODY_MAX_BYTES / 2 + 1);
        assert!(check_message_body(&quotes).is_err());
        let fewer = "\"".repeat(MESSAGE_BODY_MAX_BYTES / 2);
        assert!(check_message_body(&fewer).is_ok());
    }
}
