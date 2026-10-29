//! Drafts and commands in flight. A draft stays in its box until the command
//! that sends it succeeds. These helpers hold the rules that do not need a
//! window.

use std::collections::HashSet;

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
