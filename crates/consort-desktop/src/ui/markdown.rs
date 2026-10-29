//! Message text: Markdown split into prose and code blocks, each in its own
//! selectable `TextView`, so a right click on a code block can offer
//! "Copy code" and a right click on prose can offer the message menu.

use std::collections::HashMap;

use gpui_kit::{App, AppContext as _, Entity, component::text::TextViewState};

/// One part of a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    /// Markdown without fenced code.
    Prose(String),
    /// A fenced code block: the whole fence as Markdown, and the code alone.
    Code { markdown: String, code: String },
}

/// Splits Markdown at fenced code blocks (``` or ~~~).
pub fn segments(markdown: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut prose = String::new();
    let mut fence: Option<(String, String, String)> = None; // (marker, markdown, code)
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        match fence.as_mut() {
            None => {
                let marker = if trimmed.starts_with("```") {
                    Some("```")
                } else if trimmed.starts_with("~~~") {
                    Some("~~~")
                } else {
                    None
                };
                if let Some(marker) = marker {
                    if !prose.trim().is_empty() {
                        out.push(Segment::Prose(std::mem::take(&mut prose).trim().to_owned()));
                    }
                    prose.clear();
                    fence = Some((marker.to_owned(), line.to_owned(), String::new()));
                } else {
                    prose.push_str(line);
                }
            }
            Some((marker, md, code)) => {
                md.push_str(line);
                if trimmed.starts_with(marker.as_str()) {
                    let code = code.trim_end_matches('\n').to_owned();
                    out.push(Segment::Code {
                        markdown: md.trim_end().to_owned(),
                        code,
                    });
                    fence = None;
                } else {
                    code.push_str(line);
                }
            }
        }
    }
    if let Some((marker, mut md, code)) = fence {
        // An open fence: close it so it still renders as code.
        md.push('\n');
        md.push_str(&marker);
        out.push(Segment::Code {
            markdown: md,
            code: code.trim_end_matches('\n').to_owned(),
        });
    }
    if !prose.trim().is_empty() {
        out.push(Segment::Prose(prose.trim().to_owned()));
    }
    out
}

/// The text states of every rendered segment, by key. A state keeps its
/// selection across renders.
#[derive(Default)]
pub struct TextCache {
    states: HashMap<String, (String, Entity<TextViewState>)>,
}

impl TextCache {
    /// The state for `key`, made on first use and updated when the text
    /// changed.
    pub fn get(&mut self, key: &str, text: &str, cx: &mut App) -> Entity<TextViewState> {
        if let Some((current, state)) = self.states.get_mut(key) {
            if current != text {
                current.clear();
                current.push_str(text);
                let text = text.to_owned();
                state.update(cx, |s, cx| s.set_text(&text, cx));
            }
            return state.clone();
        }
        let state = cx.new(|cx| TextViewState::markdown(text, cx).selectable(true));
        self.states
            .insert(key.to_owned(), (text.to_owned(), state.clone()));
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_fences_split_prose() {
        let md = "Before.\n\n```toml\na = 1\n```\n\nAfter.";
        let parts = segments(md);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], Segment::Prose("Before.".into()));
        assert_eq!(
            parts[1],
            Segment::Code {
                markdown: "```toml\na = 1\n```".into(),
                code: "a = 1".into()
            }
        );
        assert_eq!(parts[2], Segment::Prose("After.".into()));
    }

    #[test]
    fn plain_text_is_one_segment() {
        assert_eq!(segments("Hi"), vec![Segment::Prose("Hi".into())]);
        assert!(segments("").is_empty());
    }

    #[test]
    fn an_open_fence_still_renders_as_code() {
        let parts = segments("```sh\nls");
        assert!(matches!(&parts[0], Segment::Code { code, .. } if code == "ls"));
    }
}
