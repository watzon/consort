//! Drafts and commands in flight. A draft stays in its box until the command
//! that sends it succeeds. These helpers hold the rules that do not need a
//! window.

use std::collections::HashSet;

use consort_client::ClientError;

/// The commands that run now, by key. A key that runs blocks a second
/// submit, so a double click or a double Enter sends once.
#[derive(Debug, Default)]
pub struct InFlight {
    keys: HashSet<String>,
}

impl InFlight {
    /// Starts a command. Returns `false` when a command with this key
    /// already runs.
    pub fn begin(&mut self, key: &str) -> bool {
        self.keys.insert(key.to_owned())
    }

    pub fn finish(&mut self, key: &str) {
        self.keys.remove(key);
    }

    pub fn is_running(&self, key: &str) -> bool {
        self.keys.contains(key)
    }
}

/// A draft is safe to clear after a successful send when the box still holds
/// what was sent. If the user typed more while the command ran, it stays.
pub fn can_clear_after_send(sent: &str, now_in_box: &str) -> bool {
    sent.trim() == now_in_box.trim()
}

/// The message for a command that failed: the error as a sentence, then what
/// the user still has, such as "Your message is still in the box."
pub fn failure_message(error: &str, kept: &str) -> String {
    let error = error.trim();
    if error.ends_with(['.', '!', '?']) {
        format!("{error} {kept}")
    } else {
        format!("{error}. {kept}")
    }
}

/// What a failed command says it kept, so the message can tell the user.
#[derive(Clone, Copy, Debug)]
pub enum Kept {
    /// The user's text is still in its box, such as "Your message is still
    /// in the box."
    Draft(&'static str),
    /// A request still waits for the user's answer, such as "The request
    /// still waits for your answer."
    Request(&'static str),
}

impl Kept {
    /// The message for a command that failed. When the hub says that a
    /// request is closed or gone (`conflict`, `not_found`), the request does
    /// not wait any more, so the message is the hub's own text and nothing
    /// that says otherwise.
    pub fn failure(&self, err: &ClientError) -> String {
        match (self, err) {
            (Kept::Request(_), ClientError::Hub { code, message })
                if code == "conflict" || code == "not_found" =>
            {
                message.trim().to_owned()
            }
            (Kept::Draft(kept) | Kept::Request(kept), err) => {
                failure_message(&err.to_string(), kept)
            }
        }
    }
}

/// Adds `text` to the end of a draft, after a blank line. A draft that is
/// empty becomes the text. Nothing in the draft is lost.
pub fn append_text(draft: &str, text: &str) -> String {
    if draft.trim().is_empty() {
        text.to_owned()
    } else {
        format!("{}\n\n{text}", draft.trim_end())
    }
}

/// Text as a Markdown quote: `> ` before each line.
pub fn quote_block(text: &str) -> String {
    text.lines().map(|line| format!("> {line}\n")).collect()
}

/// Adds a quote after the draft that is in the box. A draft stays as it is.
/// A blank line separates the draft from the quote, and the quote ends with a
/// blank line, so the user can type below it.
pub fn append_quote(draft: &str, quote: &str) -> String {
    let quote = quote.trim_end_matches('\n');
    if draft.trim().is_empty() {
        format!("{quote}\n\n")
    } else {
        format!("{}\n\n{quote}\n\n", draft.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_submit_with_the_same_key_is_refused() {
        let mut running = InFlight::default();
        assert!(running.begin("composer"));
        assert!(!running.begin("composer"));
        assert!(running.is_running("composer"));
        // Another key is independent.
        assert!(running.begin("reply"));
        running.finish("composer");
        assert!(!running.is_running("composer"));
        assert!(running.begin("composer"));
    }

    #[test]
    fn a_draft_clears_only_when_it_is_what_was_sent() {
        assert!(can_clear_after_send("hello", "hello"));
        assert!(can_clear_after_send("hello", "  hello\n"));
        assert!(!can_clear_after_send("hello", "hello and more"));
        assert!(!can_clear_after_send("hello", ""));
    }

    #[test]
    fn a_failure_reads_as_two_sentences() {
        assert_eq!(
            failure_message(
                "Consort is not connected to the hub. Wait until it reconnects, then try again",
                "Your message is still in the box."
            ),
            "Consort is not connected to the hub. Wait until it reconnects, then try again. Your message is still in the box."
        );
        // An error that ends with a period gets no second one.
        assert_eq!(
            failure_message("The hub said no.", "Your edit is still in the box."),
            "The hub said no. Your edit is still in the box."
        );
    }

    fn hub(code: &str, message: &str) -> ClientError {
        ClientError::Hub {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Finding 13 of review cycle 2: after the hub said that a request was
    /// closed, the app added "The request still waits for your answer."
    #[test]
    fn a_closed_request_shows_only_the_hub_text() {
        let kept = Kept::Request("The request still waits for your answer.");
        for code in ["conflict", "not_found"] {
            let text = kept.failure(&hub(code, "This approval was already answered or closed."));
            assert_eq!(text, "This approval was already answered or closed.");
            assert!(!text.contains("still waits"));
        }
        // Another error leaves the request open, and the text says so.
        assert_eq!(
            kept.failure(&hub("internal", "The hub failed.")),
            "The hub failed. The request still waits for your answer."
        );
        assert!(
            kept.failure(&ClientError::NotConnected)
                .ends_with("The request still waits for your answer.")
        );
    }

    #[test]
    fn a_draft_always_says_it_is_still_in_the_box() {
        let kept = Kept::Draft("Your message is still in the box.");
        // A missing channel does not remove the draft.
        assert_eq!(
            kept.failure(&hub("not_found", "That channel does not exist.")),
            "That channel does not exist. Your message is still in the box."
        );
    }

    #[test]
    fn restored_text_goes_after_the_draft() {
        assert_eq!(append_text("", "Lost words"), "Lost words");
        assert_eq!(append_text("  \n", "Lost words"), "Lost words");
        assert_eq!(
            append_text("New draft\n\n", "Lost words"),
            "New draft\n\nLost words"
        );
    }

    #[test]
    fn a_quote_goes_after_the_draft_and_the_draft_stays() {
        let quote = quote_block("First line\nSecond line");
        assert_eq!(quote, "> First line\n> Second line\n");
        assert_eq!(
            append_quote("Why did this fail?", &quote),
            "Why did this fail?\n\n> First line\n> Second line\n\n"
        );
        // Trailing blank lines of the draft do not pile up.
        assert_eq!(
            append_quote("Draft\n\n\n", &quote),
            "Draft\n\n> First line\n> Second line\n\n"
        );
    }

    #[test]
    fn a_quote_in_an_empty_box_has_no_leading_blank_line() {
        let quote = quote_block("Only line");
        assert_eq!(append_quote("", &quote), "> Only line\n\n");
        assert_eq!(append_quote("  \n", &quote), "> Only line\n\n");
    }
}
