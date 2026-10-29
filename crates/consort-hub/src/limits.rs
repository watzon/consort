//! Size limits that keep every frame under the 1 MiB frame limit.
//!
//! A frame holds JSON. So these limits count the bytes of a text as a JSON
//! string: a quote or a backslash counts 2 bytes, and a control character
//! counts 2 or 6 bytes. Other characters count their UTF-8 bytes.
//!
//! The hub refuses input that is larger than a limit. Text that comes from
//! a worker, such as an assistant message, is cut instead, because a worker
//! cannot change what its harness wrote. Pages of messages stop early when
//! they reach their byte budget, and `has_more` is then true.

use consort_proto::frame::MAX_FRAME_BYTES;

use crate::error::{HubError, Result};

/// The largest message body.
pub(crate) const MESSAGE_BODY_MAX_BYTES: usize = 256 * 1024;
/// The largest task brief, and the largest routine prompt (a routine can
/// make its prompt the brief of a task). The `all` snapshot holds every open
/// task, so a brief must stay small. Long input goes in a channel file.
pub(crate) const BRIEF_MAX_BYTES: usize = 32 * 1024;
/// The largest channel purpose. It goes into every system prompt of the
/// channel, and the `all` snapshot holds every channel.
pub(crate) const CHANNEL_PURPOSE_MAX_BYTES: usize = 4 * 1024;
/// The largest channel instructions. They go into every system prompt of the
/// channel, and the `all` snapshot holds every channel.
pub(crate) const CHANNEL_INSTRUCTIONS_MAX_BYTES: usize = 16 * 1024;
/// The largest turn text that the hub queues for a session. Events or inbox
/// messages that do not fit wait for the next turn.
pub(crate) const TURN_TEXT_MAX_BYTES: usize = 512 * 1024;
/// The smallest room that the events of a first coordinator turn get, after
/// the context pack and the system prompt.
pub(crate) const TURN_TEXT_MIN_BYTES: usize = 32 * 1024;
/// The messages in one `message.list` page.
pub(crate) const MESSAGE_PAGE_MAX_BYTES: usize = 768 * 1024;
/// The messages in a channel snapshot. The rest of the snapshot needs room
/// too.
pub(crate) const SNAPSHOT_MESSAGES_MAX_BYTES: usize = 512 * 1024;
/// The messages in a tool result. A tool result holds its JSON twice (as
/// text and as structured content), so it gets less than half a frame.
pub(crate) const TOOL_MESSAGES_MAX_BYTES: usize = 320 * 1024;
/// The largest text of a notice or of a task status reason. These often
/// hold an error text from a worker, which the hub cuts to this size.
pub(crate) const NOTICE_TEXT_MAX_BYTES: usize = 4 * 1024;
/// Room in a frame for the envelope and the other fields of a command.
pub(crate) const FRAME_ENVELOPE_BYTES: usize = 16 * 1024;

/// The number of bytes that `text` takes as a JSON string, without the quotes.
pub(crate) fn json_len(text: &str) -> usize {
    text.chars()
        .map(|c| match c {
            '"' | '\\' | '\u{08}' | '\u{0C}' | '\n' | '\r' | '\t' => 2,
            c if u32::from(c) < 0x20 => 6,
            c => c.len_utf8(),
        })
        .sum()
}

/// Refuses a text that is larger than `max` as a JSON string.
pub(crate) fn check_text(what: &str, text: &str, max: usize) -> Result<()> {
    let size = json_len(text);
    if size <= max {
        return Ok(());
    }
    Err(HubError::invalid(format!(
        "The {what} has {} KiB, and the limit is {} KiB. Shorten it, or put the long part in a file and attach the file.",
        size.div_ceil(1024),
        max / 1024
    )))
}

/// The longest start of `text` that takes at most `max` bytes as a JSON
/// string. It cuts at a character boundary.
pub(crate) fn json_prefix(text: &str, max: usize) -> &str {
    let mut used = 0;
    for (index, c) in text.char_indices() {
        used += json_len(c.encode_utf8(&mut [0; 4]));
        if used > max {
            return &text[..index];
        }
    }
    text
}

/// Cuts a text from a worker to at most `max` bytes as a JSON string, and
/// says so at the end. A text that fits is returned as it is.
pub(crate) fn cut_text(text: &str, max: usize) -> String {
    if json_len(text) <= max {
        return text.to_owned();
    }
    const MARK: &str = "\n\n[The hub cut this text here, because it is too long.]";
    let mut out = json_prefix(text, max.saturating_sub(json_len(MARK))).to_owned();
    out.push_str(MARK);
    out
}

/// The room for the turn text of a command whose other text fields take
/// `other` bytes, capped at [`TURN_TEXT_MAX_BYTES`].
pub(crate) fn turn_room(other: usize) -> usize {
    MAX_FRAME_BYTES
        .saturating_sub(FRAME_ENVELOPE_BYTES + other)
        .min(TURN_TEXT_MAX_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_len_counts_escapes() {
        assert_eq!(json_len("abc"), 3);
        assert_eq!(json_len("a\"b\\c"), 7);
        assert_eq!(json_len("\n\t"), 4);
        assert_eq!(json_len("\u{0}"), 6);
        assert_eq!(json_len("\u{e9}"), 2);
        let text = "a\"\n\u{1}\u{e9}<";
        assert_eq!(
            json_len(text) + 2,
            serde_json::to_string(text).unwrap().len()
        );
    }

    #[test]
    fn a_text_over_the_limit_is_refused_and_one_at_the_limit_is_not() {
        assert!(check_text("message", &"a".repeat(10), 10).is_ok());
        let err = check_text("message", &"a".repeat(11), 10).unwrap_err();
        assert!(err.to_error_body().message.contains("limit"));
        // A quote counts twice.
        assert!(check_text("message", &"\"".repeat(6), 10).is_err());
    }

    #[test]
    fn a_cut_text_fits_and_ends_with_a_mark() {
        let text = "\u{e9}".repeat(1000);
        let cut = cut_text(&text, 300);
        assert!(json_len(&cut) <= 300);
        assert!(cut.ends_with("too long.]"));
        assert_eq!(cut_text("short", 300), "short");
        assert_eq!(json_prefix("\u{e9}\u{e9}", 3), "\u{e9}");
    }
}
