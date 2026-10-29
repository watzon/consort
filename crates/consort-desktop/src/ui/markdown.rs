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

/// How many text states the cache keeps. A screen shows far fewer, so the
/// states in view are always the newest ones.
pub const TEXT_CACHE_CAP: usize = 512;

/// A map that keeps at most `cap` entries. Reading or writing an entry makes
/// it the newest. When the map is full, the entry that was used longest ago
/// leaves.
#[derive(Debug)]
pub struct Lru<V> {
    entries: HashMap<String, (u64, V)>,
    clock: u64,
    cap: usize,
}

impl<V> Lru<V> {
    pub fn new(cap: usize) -> Lru<V> {
        Lru {
            entries: HashMap::new(),
            clock: 0,
            // A map that holds nothing cannot return what it just stored.
            cap: cap.max(1),
        }
    }

    /// The entry for `key`, marked as the newest.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.clock += 1;
        let clock = self.clock;
        self.entries.get_mut(key).map(|(used, value)| {
            *used = clock;
            value
        })
    }

    /// Stores an entry as the newest. Returns the entry that left to make
    /// room, if any. The entry just stored never leaves.
    pub fn insert(&mut self, key: String, value: V) -> Option<(String, V)> {
        self.clock += 1;
        self.entries.insert(key.clone(), (self.clock, value));
        if self.entries.len() <= self.cap {
            return None;
        }
        let oldest = self
            .entries
            .iter()
            .filter(|(k, _)| **k != key)
            .min_by_key(|(_, (used, _))| *used)
            .map(|(k, _)| k.clone())?;
        self.entries
            .remove(&oldest)
            .map(|(_, value)| (oldest, value))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }
}

/// The text states of every rendered segment, by key. A state keeps its
/// selection across renders. The cache drops the states that were used
/// longest ago, so a long session does not keep every message it ever drew.
pub struct TextCache {
    states: Lru<(String, Entity<TextViewState>)>,
}

impl Default for TextCache {
    fn default() -> TextCache {
        TextCache {
            states: Lru::new(TEXT_CACHE_CAP),
        }
    }
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

/// The text of a selection, exactly as the reader selected it. The text
/// view ends each block it copies with a line break, so a partial selection
/// such as "No other files were changed. The ta" arrives with a "\n" at its
/// end. The reader did not select that break. Cmd+C already removes it, and
/// the menu must give the same text.
pub fn exact_selection(copied: &str) -> String {
    copied.trim_end_matches(['\n', '\r']).to_owned()
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
    fn the_least_recently_used_entry_leaves_first() {
        let mut cache: Lru<u32> = Lru::new(3);
        assert!(cache.insert("a".into(), 1).is_none());
        assert!(cache.insert("b".into(), 2).is_none());
        assert!(cache.insert("c".into(), 3).is_none());
        // Reading "a" makes "b" the oldest.
        assert_eq!(cache.get_mut("a").copied(), Some(1));
        assert_eq!(cache.insert("d".into(), 4), Some(("b".into(), 2)));
        assert_eq!(cache.len(), 3);
        assert!(cache.contains("a") && cache.contains("c") && cache.contains("d"));
        assert!(!cache.contains("b"));
    }

    #[test]
    fn the_cache_never_grows_past_its_cap() {
        let mut cache: Lru<usize> = Lru::new(TEXT_CACHE_CAP);
        for i in 0..TEXT_CACHE_CAP * 3 {
            cache.insert(format!("message-{i}"), i);
            assert!(cache.len() <= TEXT_CACHE_CAP);
        }
        assert_eq!(cache.len(), TEXT_CACHE_CAP);
        // The newest entries stay and the oldest are gone.
        assert!(cache.contains(&format!("message-{}", TEXT_CACHE_CAP * 3 - 1)));
        assert!(!cache.contains("message-0"));
    }

    #[test]
    fn the_entry_just_stored_stays_even_in_a_cache_of_one() {
        let mut cache: Lru<u32> = Lru::new(0);
        cache.insert("a".into(), 1);
        assert_eq!(cache.insert("b".into(), 2), Some(("a".into(), 1)));
        assert!(cache.contains("b"));
        // Replacing an entry evicts nothing.
        assert!(cache.insert("b".into(), 3).is_none());
        assert_eq!(cache.get_mut("b").copied(), Some(3));
    }

    #[test]
    fn a_partial_selection_copies_without_the_block_break() {
        assert_eq!(
            exact_selection("No other files were changed. The ta\n"),
            "No other files were changed. The ta"
        );
        assert_eq!(
            exact_selection("two blocks\nsecond\n\n"),
            "two blocks\nsecond"
        );
        assert_eq!(exact_selection("windows\r\n"), "windows");
        assert_eq!(exact_selection("nothing to trim"), "nothing to trim");
        assert_eq!(exact_selection("\n"), "");
        // Spaces the reader selected stay.
        assert_eq!(exact_selection(" lead and trail \n"), " lead and trail ");
    }

    #[test]
    fn an_open_fence_still_renders_as_code() {
        let parts = segments("```sh\nls");
        assert!(matches!(&parts[0], Segment::Code { code, .. } if code == "ls"));
    }
}
