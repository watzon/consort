//! Size caps for session events.
//!
//! A frame on the worker link may hold at most 1 MiB, and a batch of
//! spooled events at most 256 KiB, except that a batch always holds one
//! event. A harness can write an event of any size, for example a tool call
//! with large arguments. So the worker cuts an event to at most
//! [`MAX_EVENT_BYTES`] of JSON before it writes the event to the spool. A cut
//! event stays a valid event, and each cut is marked:
//!
//! - Large tool arguments become an object with `consort_truncated`, the
//!   original size and the start of the JSON.
//! - A long text keeps its start and gets a note with the number of bytes
//!   that the worker removed.
//! - A long list keeps its first entries.
//!
//! If an event still does not fit, it becomes an `error` event that tells
//! the hub what the worker dropped.

use consort_proto::model::{ApprovalDetails, ItemPayload};
use consort_proto::worker_link::{RequestBody, RequestOpened, SessionEvent};
use serde_json::{Value, json};

/// The largest event, as JSON bytes. A batch is then at most
/// 256 KiB + this size, which stays well under the 1 MiB frame limit.
pub const MAX_EVENT_BYTES: usize = 192 * 1024;
/// Tool arguments larger than this, as JSON bytes, are replaced by a
/// preview.
const MAX_ARGUMENTS_BYTES: usize = 16 * 1024;
/// The JSON that the preview of large tool arguments keeps.
const ARGUMENTS_PREVIEW_BYTES: usize = 4 * 1024;
/// The longest text after the first pass. Each next pass keeps a quarter.
const FIRST_TEXT_LIMIT: usize = 64 * 1024;
/// The shortest text limit that the worker tries.
const LAST_TEXT_LIMIT: usize = 256;

/// The first `max` bytes of `text`, cut at a character boundary.
fn head(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn json_size<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_vec(value)
        .map(|v| v.len())
        .unwrap_or(usize::MAX)
}

/// Replaces large tool arguments with a marked preview.
fn shrink_arguments(arguments: &mut Value) {
    let text = arguments.to_string();
    if text.len() <= MAX_ARGUMENTS_BYTES {
        return;
    }
    *arguments = json!({
        "consort_truncated": true,
        "original_bytes": text.len(),
        "preview": head(&text, ARGUMENTS_PREVIEW_BYTES),
    });
}

/// Cuts every text longer than `limit` and every list longer than
/// `limit / 64` entries (at least 4).
fn cut(value: &mut Value, limit: usize) {
    match value {
        Value::String(text) if text.len() > limit => {
            let removed = text.len() - head(text, limit).len();
            let kept = head(text, limit).to_owned();
            *text = format!("{kept}\n[The worker cut {removed} bytes from this text.]");
        }
        Value::Array(items) => {
            items.truncate((limit / 64).max(4));
            for item in items {
                cut(item, limit);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                cut(item, limit);
            }
        }
        _ => {}
    }
}

/// The event that replaces an event that the worker could not cut to size.
fn dropped(size: usize) -> SessionEvent {
    SessionEvent::Error {
        code: "event_too_large".to_owned(),
        message: format!(
            "The harness sent an event of {size} bytes. The limit is {MAX_EVENT_BYTES} bytes, so the worker dropped it."
        ),
    }
}

/// Whether [`cap_event`] would cut this request. For an approval, the
/// worker then denies the request instead, because the user would allow
/// something that they cannot see in full.
pub fn request_must_be_cut(opened: &RequestOpened) -> bool {
    json_size(&SessionEvent::RequestOpened(opened.clone())) > MAX_EVENT_BYTES
}

/// Cuts an event to at most [`MAX_EVENT_BYTES`] of JSON. An event that fits
/// comes back as it is.
pub fn cap_event(event: SessionEvent) -> SessionEvent {
    let size = json_size(&event);
    if size <= MAX_EVENT_BYTES {
        return event;
    }
    let mut event = event;
    match &mut event {
        SessionEvent::Item(item) => {
            if let ItemPayload::ToolCall(call) = &mut item.payload {
                shrink_arguments(&mut call.arguments);
            }
        }
        SessionEvent::RequestOpened(opened) => {
            if let RequestBody::Approval(ApprovalDetails::Tool { arguments, .. }) = &mut opened.body
            {
                shrink_arguments(arguments);
            }
        }
        _ => {}
    }
    if json_size(&event) <= MAX_EVENT_BYTES {
        return event;
    }
    let Ok(mut value) = serde_json::to_value(&event) else {
        return dropped(size);
    };
    let mut limit = FIRST_TEXT_LIMIT;
    loop {
        cut(&mut value, limit);
        if json_size(&value) <= MAX_EVENT_BYTES {
            break;
        }
        if limit <= LAST_TEXT_LIMIT {
            return dropped(size);
        }
        limit = (limit / 4).max(LAST_TEXT_LIMIT);
    }
    serde_json::from_value(value).unwrap_or_else(|_| dropped(size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::ids::{ItemId, RequestId, SessionId, TurnId};
    use consort_proto::model::{ItemStatus, SessionItem, ToolCallPayload};
    use consort_proto::time::Timestamp;

    fn tool_item(arguments: Value) -> SessionEvent {
        SessionEvent::Item(SessionItem {
            id: ItemId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new("t"),
            payload: ItemPayload::ToolCall(ToolCallPayload {
                server: Some("github".into()),
                tool: "create_file".into(),
                arguments,
                output: None,
                is_error: false,
            }),
            status: ItemStatus::InProgress,
            created_at: Timestamp::EPOCH,
            updated_at: Timestamp::EPOCH,
        })
    }

    #[test]
    fn small_events_stay_as_they_are() {
        let event = tool_item(json!({"path": "a.txt"}));
        assert_eq!(cap_event(event.clone()), event);
    }

    #[test]
    fn large_tool_arguments_become_a_marked_preview() {
        let event = cap_event(tool_item(json!({"content": "x".repeat(2 * 1024 * 1024)})));
        assert!(json_size(&event) <= MAX_EVENT_BYTES);
        let SessionEvent::Item(item) = event else {
            panic!("the event stays an item")
        };
        let ItemPayload::ToolCall(call) = item.payload else {
            panic!("the item stays a tool call")
        };
        assert_eq!(call.tool, "create_file");
        assert_eq!(call.arguments["consort_truncated"], true);
        assert!(call.arguments["original_bytes"].as_u64().unwrap() > 2 * 1024 * 1024);
        assert!(call.arguments["preview"].as_str().unwrap().len() <= ARGUMENTS_PREVIEW_BYTES);
    }

    #[test]
    fn long_texts_are_cut_and_marked() {
        let event = cap_event(SessionEvent::Error {
            code: "x".into(),
            message: "y".repeat(1_200_000),
        });
        assert!(json_size(&event) <= MAX_EVENT_BYTES);
        let SessionEvent::Error { code, message } = event else {
            panic!("the event stays an error")
        };
        assert_eq!(code, "x");
        assert!(message.starts_with("yyyy"));
        assert!(
            message.ends_with("bytes from this text.]"),
            "{}",
            &message[message.len() - 60..]
        );
    }

    #[test]
    fn approvals_with_large_arguments_stay_valid() {
        let event = SessionEvent::RequestOpened(RequestOpened {
            request_id: RequestId::new("r1"),
            summary: "Use a tool".into(),
            body: RequestBody::Approval(ApprovalDetails::Tool {
                server: Some("x".into()),
                tool: "y".into(),
                arguments: json!({"many": vec!["z".repeat(1000); 2000]}),
            }),
            options: vec![],
        });
        let capped = cap_event(event);
        assert!(json_size(&capped) <= MAX_EVENT_BYTES);
        assert!(
            matches!(&capped, SessionEvent::RequestOpened(r) if matches!(&r.body, RequestBody::Approval(ApprovalDetails::Tool { arguments, .. }) if arguments["consort_truncated"] == true))
        );
    }
}
