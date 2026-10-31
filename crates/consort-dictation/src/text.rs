//! The words of a dictation while they come: the final text of the parts that
//! ended, then the preview of the speech after them.

/// How the dictated words join the draft on each side of the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spacing {
    pub before: &'static str,
    pub after: &'static str,
}

impl Spacing {
    /// A space on a side where the draft has a word against the cursor. A mark
    /// such as a period after the cursor stays against the dictated words.
    pub fn around(before: &str, after: &str) -> Spacing {
        let glued_before = before
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_whitespace());
        let glued_after = after.chars().next().is_some_and(char::is_alphanumeric);
        Spacing {
            before: if glued_before { " " } else { "" },
            after: if glued_after { " " } else { "" },
        }
    }
}

/// The three parts of the text of a message box during a dictation: the draft
/// before the cursor, the dictated words, and the draft after the cursor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    before: String,
    words: String,
    after: String,
}

/// What an edit of the user did to a [`Draft`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// The edit is in the draft. The dictated words are as they were.
    InDraft,
    /// The edit changed the dictated words. The user took them over.
    TookOver,
}

impl Draft {
    pub fn new(before: impl Into<String>, after: impl Into<String>) -> Draft {
        Draft {
            before: before.into(),
            words: String::new(),
            after: after.into(),
        }
    }

    pub fn set_words(&mut self, words: impl Into<String>) {
        self.words = words.into();
    }

    pub fn words(&self) -> &str {
        &self.words
    }

    fn spacing(&self) -> Spacing {
        if self.words.is_empty() {
            Spacing {
                before: "",
                after: "",
            }
        } else {
            Spacing::around(&self.before, &self.after)
        }
    }

    /// The text that goes between the two parts of the draft.
    pub fn insert(&self) -> String {
        let spacing = self.spacing();
        format!("{}{}{}", spacing.before, self.words, spacing.after)
    }

    /// The whole text of the box.
    pub fn whole(&self) -> String {
        format!("{}{}{}", self.before, self.insert(), self.after)
    }

    /// Where [`Draft::insert`] starts, in bytes.
    pub fn insert_at(&self) -> usize {
        self.before.len()
    }

    /// The bytes of the dictated words, for the gray color.
    pub fn words_range(&self) -> std::ops::Range<usize> {
        let start = self.before.len() + self.spacing().before.len();
        start..start + self.words.len()
    }

    /// Reads an edit of the user. `now` is the text of the box after it. An
    /// edit in the draft moves into its part. Text at either end of the words
    /// joins the draft on that side.
    pub fn on_edit(&mut self, now: &str) -> Edit {
        let old = self.whole();
        let prefix = common_prefix(&old, now);
        let suffix = common_suffix(&old[prefix..], &now[prefix..]);
        let changed = prefix..old.len() - suffix;
        let words = self.words_range();
        let touches = changed.start < words.end && changed.end > words.start;
        if touches && !words.is_empty() {
            return Edit::TookOver;
        }
        if changed.end <= words.start {
            let keep = words.start - self.spacing().before.len();
            let new_end = keep + now.len() - old.len();
            self.before = now[..new_end].to_owned();
        } else {
            let tail = old.len() - words.end - self.spacing().after.len();
            self.after = now[now.len() - tail..].to_owned();
        }
        Edit::InDraft
    }
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((at, _), _)| at)
}

fn common_suffix(a: &str, b: &str) -> usize {
    a.chars()
        .rev()
        .zip(b.chars().rev())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x.len_utf8())
        .sum()
}

/// How far from its place a part may end in the preview words. A preview and a
/// final pass seldom differ by more words than this in one part.
const ALIGN_WINDOW: usize = 4;

/// Joins the final parts and the preview. A part replaces the preview words
/// that it covers, so the text does not repeat and does not jump back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveWords {
    finals: Vec<String>,
    preview: Vec<String>,
    /// The preview words that the final parts have covered.
    covered: usize,
}

impl LiveWords {
    /// The whole text of the preview stream so far.
    pub fn on_partial(&mut self, committed: &str, tentative: &str) {
        self.preview = committed
            .split_whitespace()
            .chain(tentative.split_whitespace())
            .map(str::to_owned)
            .collect();
        self.covered = self.covered.min(self.preview.len());
    }

    /// The final text of one part.
    pub fn on_segment(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.finals.push(text.to_owned());
        self.covered = self.covered_after(text);
    }

    /// Finds where the part ends in the preview. The part has about as many
    /// words as the preview, and its last word often matches one there.
    fn covered_after(&self, part: &str) -> usize {
        let words: Vec<&str> = part.split_whitespace().collect();
        let expected = (self.covered + words.len()).min(self.preview.len());
        let Some(last) = words.last().map(|w| normalize(w)) else {
            return expected;
        };
        let low = expected.saturating_sub(ALIGN_WINDOW).max(self.covered + 1);
        let high = (expected + ALIGN_WINDOW).min(self.preview.len());
        (low..=high)
            .filter(|end| normalize(&self.preview[end - 1]) == last)
            .min_by_key(|end| end.abs_diff(expected))
            .unwrap_or(expected)
    }

    /// The text to show now.
    pub fn text(&self) -> String {
        let finals = self.finals.iter().map(String::as_str);
        let preview = self.preview[self.covered..].iter().map(String::as_str);
        finals.chain(preview).collect::<Vec<_>>().join(" ")
    }

    /// The final parts alone.
    pub fn finals(&self) -> String {
        self.finals.join(" ")
    }
}

/// A word with no case and no marks, to compare a preview with a final pass.
fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
