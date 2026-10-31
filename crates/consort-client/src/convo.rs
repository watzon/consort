//! Phase 4, the conversation and the tasks in full: reactions, threads on
//! messages, search, the task item view, the archived tasks, and the forms
//! of the New task and the channel settings dialogs.
//!
//! Everything here is pure: the reducer and the mock build these views, and
//! the hub backend fills [`ConvoCache`]. See `docs/notes/phase4.md`.

use std::collections::BTreeSet;
use std::sync::Arc;

use consort_proto::model::{Author, Reaction};

use crate::connectors::LoadState;
use crate::view::{MessageView, TaskStatus, ViewState};

// Reactions ----------------------------------------------------------------

/// The six emoji at the top of the message menu, in this order.
pub const QUICK_EMOJI: [&str; 6] = [
    "\u{1F44D}",        // thumbs up
    "\u{2764}\u{FE0F}", // red heart
    "\u{1F64F}",        // folded hands
    "\u{1F389}",        // party popper
    "\u{1F440}",        // eyes
    "\u{2705}",         // check mark button
];

/// The groups of the full emoji picker behind More: a name and the emoji.
pub const EMOJI_GROUPS: &[(&str, &[&str])] = &[
    (
        "Smileys",
        &[
            "😀", "😃", "😄", "😁", "😆", "😅", "😂", "🙂", "🙃", "😉", "😊", "😇", "🥰", "😍",
            "🤩", "😘", "😋", "😛", "😜", "🤪", "🤔", "🤨", "😐", "😑", "😶", "🙄", "😏", "😬",
            "😌", "😔", "😴", "😷", "🤒", "🤯", "🥳", "😎", "🤓", "😕", "😟", "😮", "😲", "😳",
            "🥺", "😢", "😭", "😱", "😤", "😠", "🤬", "💀",
        ],
    ),
    (
        "Gestures",
        &[
            "👍", "👎", "👌", "✌️", "🤞", "🤟", "🤘", "👏", "🙌", "👐", "🤲", "🙏", "🤝", "💪",
            "👋", "🤙", "👀", "🧠", "✍️", "👉", "👈", "👆", "👇", "☝️",
        ],
    ),
    (
        "Hearts and symbols",
        &[
            "❤️", "🧡", "💛", "💚", "💙", "💜", "🖤", "🤍", "💔", "💯", "✅", "☑️", "✔️", "❌",
            "❗", "❓", "⚠️", "🚫", "⭐", "🌟", "✨", "⚡", "🔥", "💥", "🎯", "📌",
        ],
    ),
    (
        "Objects",
        &[
            "🎉", "🎊", "🎁", "🏆", "🥇", "🚀", "🛠️", "🔧", "🔨", "⚙️", "🧪", "🐛", "📦", "📝",
            "📎", "📅", "⏰", "⏳", "💡", "🔒", "🔑", "💻", "🖥️", "📱", "☕", "🍕",
        ],
    ),
    (
        "Nature",
        &[
            "🌱", "🌿", "🍀", "🌻", "🌈", "☀️", "🌙", "☁️", "❄️", "🌊", "🐶", "🐱", "🦊", "🐢",
            "🐝", "🦋",
        ],
    ),
];

/// One reaction chip under a bubble: an emoji with who chose it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReactionView {
    pub emoji: String,
    /// You chose it. The chip is in the accent.
    pub mine: bool,
    /// The assistant chose it.
    pub assistant: bool,
}

impl ReactionView {
    /// The count on the chip: only when you and the assistant chose the
    /// same emoji (the approved design).
    pub fn count_label(&self) -> Option<String> {
        (self.mine && self.assistant).then(|| "2".to_owned())
    }
}

/// The chips of a message, in the order the emoji were first chosen. One
/// chip for each emoji. Reactions of the hub itself do not show.
pub fn reaction_views(reactions: &[Reaction]) -> Vec<ReactionView> {
    let mut out: Vec<ReactionView> = Vec::new();
    for reaction in reactions {
        let index = match out.iter().position(|r| r.emoji == reaction.emoji) {
            Some(i) => i,
            None => {
                if reaction.author == Author::System {
                    continue;
                }
                out.push(ReactionView {
                    emoji: reaction.emoji.clone(),
                    ..ReactionView::default()
                });
                out.len() - 1
            }
        };
        match reaction.author {
            Author::User => out[index].mine = true,
            Author::Assistant => out[index].assistant = true,
            Author::System => {}
        }
    }
    out
}

/// Whether you chose `emoji` on this message, so a second click removes it.
pub fn chose(message: &MessageView, emoji: &str) -> bool {
    message.reactions.iter().any(|r| r.emoji == emoji && r.mine)
}

// Threads on messages ------------------------------------------------------

/// "4 replies" or "1 reply".
pub fn reply_count_label(count: u32) -> String {
    if count == 1 {
        "1 reply".to_owned()
    } else {
        format!("{count} replies")
    }
}

/// "Last reply 9:31 AM", or with the day when it is not today: "Last reply
/// yesterday at 9:31 AM".
pub fn last_reply_label(at_ms: i64, now_ms: i64) -> String {
    let day = crate::format::day_label(at_ms, now_ms);
    let time = crate::format::time_label(at_ms);
    if day == "Today" || day.is_empty() {
        format!("Last reply {time}")
    } else if day == "Yesterday" {
        format!("Last reply yesterday at {time}")
    } else {
        format!("Last reply {day} at {time}")
    }
}

/// The thread of one message: the root and its replies, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageThreadView {
    pub root_id: String,
    pub channel_id: String,
    /// "In Tutti, 4 replies".
    pub subtitle: String,
    pub root: MessageView,
    pub replies: Vec<MessageView>,
    /// The replies came from the hub. Before that, the panel shows the root
    /// and the count only.
    pub loaded: bool,
}

// Search -------------------------------------------------------------------

/// The scope filter of the search palette.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SearchScope {
    #[default]
    All,
    Messages,
    Tasks,
    Memory,
}

impl SearchScope {
    pub const ALL: [SearchScope; 4] = [
        SearchScope::All,
        SearchScope::Messages,
        SearchScope::Tasks,
        SearchScope::Memory,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SearchScope::All => "All",
            SearchScope::Messages => "Messages",
            SearchScope::Tasks => "Tasks",
            SearchScope::Memory => "Memory",
        }
    }

    /// The next scope, for Tab. Shift+Tab goes back.
    pub fn next(self, back: bool) -> SearchScope {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        let n = Self::ALL.len();
        Self::ALL[if back { (i + n - 1) % n } else { (i + 1) % n }]
    }

    /// The kinds that the hub searches for this scope.
    pub fn kinds(self) -> Vec<consort_proto::tools::SearchKind> {
        use consort_proto::tools::SearchKind as K;
        match self {
            SearchScope::All => Vec::new(),
            SearchScope::Messages => vec![K::Message],
            SearchScope::Tasks => vec![K::Task],
            SearchScope::Memory => vec![K::MemoryNote],
        }
    }

    /// Local results (channels and tasks) show in this scope.
    pub fn shows_go_to(self) -> bool {
        matches!(self, SearchScope::All | SearchScope::Tasks)
    }
}

/// Where the hub search is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SearchLoad {
    /// No hub search: the query is empty.
    #[default]
    Idle,
    /// The hub search runs. The palette shows skeleton rows.
    Loading,
    Done,
    /// What went wrong and what to do next.
    Failed(String),
}

/// What a hub result opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchTarget {
    /// A message in a timeline, or a reply in a thread. `task_id` is set
    /// when the thread is a task thread.
    Message {
        channel_id: String,
        message_id: String,
        thread_root_id: Option<String>,
        task_id: Option<String>,
    },
    Task {
        task_id: String,
        channel_id: String,
    },
    /// A memory note. `channel_id` is `None` for global memory.
    Memory {
        channel_id: Option<String>,
        path: String,
    },
}

/// The kind of a hub result, for its section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SearchSection {
    Messages,
    Memory,
    Tasks,
}

impl SearchSection {
    pub fn label(self) -> &'static str {
        match self {
            SearchSection::Messages => "Messages",
            SearchSection::Memory => "Memory",
            SearchSection::Tasks => "Tasks",
        }
    }
}

/// One result from the hub, ready to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHitView {
    pub section: SearchSection,
    pub target: SearchTarget,
    /// The first line: the channel name, the note path, or the task title.
    pub title: String,
    /// Quiet words after the title, such as "in a thread".
    pub title_note: Option<String>,
    /// The icon of the row: the channel icon, `notebook` for a note, or
    /// none for a task (it shows its status glyph).
    pub icon: Option<String>,
    /// Tutti's row shows the Three voices mark instead of an icon.
    pub tutti: bool,
    /// The task status, for a task row.
    pub status: Option<TaskStatus>,
    /// The matching text, plain. The palette makes the query words bold.
    pub snippet: String,
    /// "Today 9:14 AM", "Changed October 25", "sayso, resolved October 22".
    pub detail: String,
}

/// The search palette's hub part.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchView {
    /// The query that `hits` answer.
    pub query: String,
    pub scope: SearchScope,
    pub load: SearchLoad,
    pub hits: Vec<SearchHitView>,
}

/// A local result: a channel or a task that the client holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalHit {
    Channel {
        id: String,
        name: String,
        icon: String,
        purpose: String,
        tutti: bool,
    },
    Task {
        id: String,
        title: String,
        channel_name: String,
        status: TaskStatus,
    },
}

/// The words of a query, in lower case. Words of one character count only
/// when they are the whole query.
pub fn query_words(query: &str) -> Vec<String> {
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    words
}

/// Whether every word of the query starts a word in `text`, ignoring case.
/// The hub matches the same way (each word as a prefix).
pub fn matches_all(text: &str, words: &[String]) -> bool {
    let lower = text.to_lowercase();
    let text_words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words
        .iter()
        .all(|w| text_words.iter().any(|t| t.starts_with(w.as_str())))
}

/// Channels whose name or purpose matches every word, then tasks whose
/// title matches, at most `limit` of each. The palette shows them at once.
pub fn local_results(state: &ViewState, query: &str, limit: usize) -> Vec<LocalHit> {
    let words = query_words(query);
    if words.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for channel in state
        .channels
        .iter()
        .filter(|c| matches_all(&format!("{} {}", c.name, c.purpose), &words))
        .take(limit)
    {
        let tutti = channel.kind == crate::view::ChannelKind::Tutti;
        out.push(LocalHit::Channel {
            id: channel.id.clone(),
            name: if tutti {
                crate::view::TUTTI.to_owned()
            } else {
                channel.name.clone()
            },
            icon: channel.icon.clone(),
            purpose: channel.purpose.clone(),
            tutti,
        });
    }
    for task in state
        .tasks
        .iter()
        .filter(|t| matches_all(&t.title, &words))
        .take(limit)
    {
        out.push(LocalHit::Task {
            id: task.id.clone(),
            title: task.title.clone(),
            channel_name: task.channel_name.clone(),
            status: task.status,
        });
    }
    out
}

/// Splits `text` into runs, with `true` for the runs that a query word
/// starts. The palette draws them bold in `text.primary`.
pub fn highlight(text: &str, words: &[String]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let push = |s: &str, bold: bool, out: &mut Vec<(String, bool)>| {
        if s.is_empty() {
            return;
        }
        match out.last_mut() {
            Some((last, b)) if *b == bold => last.push_str(s),
            _ => out.push((s.to_owned(), bold)),
        }
    };
    // Each word of the text, with the part that a query word covers.
    let mut rest = text;
    while !rest.is_empty() {
        let start = rest
            .char_indices()
            .find(|(_, c)| c.is_alphanumeric())
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        push(&rest[..start], false, &mut out);
        rest = &rest[start..];
        let end = rest
            .char_indices()
            .find(|(_, c)| !c.is_alphanumeric())
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let word = &rest[..end];
        let lower = word.to_lowercase();
        let cover = words
            .iter()
            .filter(|w| lower.starts_with(w.as_str()))
            .map(|w| w.chars().count())
            .max()
            .unwrap_or(0);
        if cover == 0 {
            push(word, false, &mut out);
        } else {
            let split = word
                .char_indices()
                .nth(cover)
                .map(|(i, _)| i)
                .unwrap_or(word.len());
            push(&word[..split], true, &mut out);
            push(&word[split..], false, &mut out);
        }
        rest = &rest[end..];
    }
    out
}

/// The search palette's footer line on the right: "14 results from the
/// hub", "Searching the hub", or the failure.
pub fn search_footer(search: &SearchView) -> String {
    match &search.load {
        SearchLoad::Idle => String::new(),
        SearchLoad::Loading => "Searching the hub".to_owned(),
        SearchLoad::Done => match search.hits.len() {
            1 => "1 result from the hub".to_owned(),
            n => format!("{n} results from the hub"),
        },
        SearchLoad::Failed(_) => "The hub search did not work".to_owned(),
    }
}

// The task item view -------------------------------------------------------

/// What an item shows in full.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// Command output.
    #[default]
    Output,
    /// A file diff.
    Diff,
}

/// Where the full text of an item is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ItemDetailState {
    /// The hub asks the worker. "Getting the output from bigboi".
    #[default]
    Loading,
    Loaded {
        text: Arc<String>,
    },
    /// The worker is offline. The summary in the thread is still there.
    Offline,
    /// The worker has no full text for the item, or another failure. The
    /// text says what happened and what to do next.
    Failed(String),
}

/// The open item of a task: the whole output of a command or a diff.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemDetailView {
    pub task_id: String,
    pub session_id: String,
    pub item_id: String,
    pub kind: ItemKind,
    /// The command line or the file path.
    pub title: String,
    /// The worker that ran it.
    pub worker_name: String,
    /// "Exit code 0, ran 2 min 14 s on bigboi, 7:41 PM" or "10 lines
    /// added, 4 removed, on bigboi at 7:38 PM". Empty until the items load.
    pub subtitle: String,
    pub lines_added: u32,
    pub lines_removed: u32,
    /// The place of the item among the task's commands and file changes,
    /// from 1, and how many there are. 0 until the items load.
    pub index: usize,
    pub total: usize,
    /// What the command check said, for a command that it looked at.
    pub check: Option<crate::command_check::CheckDetailView>,
    pub state: ItemDetailState,
}

impl ItemDetailView {
    /// "Item 9 of 20".
    pub fn position_label(&self) -> Option<String> {
        (self.index > 0 && self.total > 0).then(|| format!("Item {} of {}", self.index, self.total))
    }
}

/// "2 min 14 s", "8 s", "1 h 4 min".
pub fn duration_label(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds < 60 {
        return format!("{} s", seconds.max(if ms > 0 { 1 } else { 0 }));
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        let s = seconds % 60;
        return if s == 0 {
            format!("{minutes} min")
        } else {
            format!("{minutes} min {s} s")
        };
    }
    let m = minutes % 60;
    if m == 0 {
        format!("{} h", minutes / 60)
    } else {
        format!("{} h {m} min", minutes / 60)
    }
}

/// One line of a diff, as the item view draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    /// A file header (`--- a/x` and `+++ b/x` are joined into the path).
    File(String),
    /// `@@ -80,14 +80,22 @@ fn draw()`.
    Hunk(String),
    Context {
        old: u32,
        new: u32,
        text: String,
    },
    Added {
        new: u32,
        text: String,
    },
    Removed {
        old: u32,
        text: String,
    },
}

/// The lines of a unified diff with their old and new line numbers.
pub fn diff_lines(diff: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            let path = path.trim_start_matches("b/").to_owned();
            out.push(DiffLine::File(path));
        } else if line.starts_with("--- ")
            || line.starts_with("diff ")
            || line.starts_with("index ")
        {
            continue;
        } else if line.starts_with("@@") {
            let (o, n) = hunk_starts(line);
            old = o;
            new = n;
            out.push(DiffLine::Hunk(line.to_owned()));
        } else if let Some(text) = line.strip_prefix('+') {
            out.push(DiffLine::Added {
                new,
                text: text.to_owned(),
            });
            new = new.saturating_add(1);
        } else if let Some(text) = line.strip_prefix('-') {
            out.push(DiffLine::Removed {
                old,
                text: text.to_owned(),
            });
            old = old.saturating_add(1);
        } else if line.starts_with('\\') {
            continue;
        } else {
            let text = line.strip_prefix(' ').unwrap_or(line).to_owned();
            out.push(DiffLine::Context { old, new, text });
            old = old.saturating_add(1);
            new = new.saturating_add(1);
        }
    }
    out
}

/// The lines of a diff between two texts of one file, with 3 lines of
/// context, for the review of a new skill version. A file that is new has
/// no old text, and a removed file no new text.
pub fn text_diff(path: &str, old: Option<&str>, new: Option<&str>) -> Vec<DiffLine> {
    let unified = similar::TextDiff::from_lines(old.unwrap_or(""), new.unwrap_or(""))
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();
    diff_lines(&unified)
}

/// The start lines of a hunk header: `@@ -80,14 +80,22 @@` gives (80, 80).
fn hunk_starts(header: &str) -> (u32, u32) {
    let mut old = 1;
    let mut new = 1;
    for part in header.split_whitespace() {
        let number = |p: &str| {
            p.split(',')
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(1)
        };
        if let Some(p) = part.strip_prefix('-') {
            old = number(p);
        } else if let Some(p) = part.strip_prefix('+') {
            new = number(p);
        }
    }
    (old, new)
}

/// The lines of command output. A last empty line from the final newline
/// is not a line.
pub fn output_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
        .into_iter()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

/// "1,284 lines from bigboi. The end shows first. Scroll up for the start."
pub fn output_caption(lines: usize, worker: &str) -> String {
    let count = thousands(lines);
    let noun = if lines == 1 { "line" } else { "lines" };
    format!("{count} {noun} from {worker}. The end shows first. Scroll up for the start.")
}

/// 1284 as "1,284".
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// Archived tasks -----------------------------------------------------------

/// One archived task, in the Archived view of the Tasks panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArchivedTaskView {
    pub id: String,
    pub title: String,
    pub channel_name: String,
    pub status: TaskStatus,
    /// "Archived October 22".
    pub archived_label: String,
}

/// The archived tasks that the client loaded with `task.list_archived`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArchivedTasksView {
    pub load: LoadState,
    pub tasks: Vec<ArchivedTaskView>,
    pub has_more: bool,
}

// What the reducer adds to the view state ---------------------------------

/// The phase 4 part of the view state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConvoView {
    /// The message threads that the client loaded, at most the open one and
    /// those opened before.
    pub threads: Vec<MessageThreadView>,
    pub search: SearchView,
    /// The open task item, if one is open.
    pub item: Option<ItemDetailView>,
    pub archived: ArchivedTasksView,
    /// Every repo that the hub knows, by name, for the channel settings.
    pub repos: Vec<crate::view::RepoView>,
}

impl ConvoView {
    pub fn thread(&self, root_id: &str) -> Option<&MessageThreadView> {
        self.threads.iter().find(|t| t.root_id == root_id)
    }
}

/// The hub search as the client holds it.
#[derive(Clone, Debug, Default)]
pub struct SearchCache {
    pub query: String,
    pub scope: SearchScope,
    pub load: SearchLoad,
    pub hits: Vec<consort_proto::tools::SearchHit>,
    /// Counts the searches, so an answer to an old query does nothing.
    pub serial: u64,
}

/// The archived tasks as the client holds them.
#[derive(Clone, Debug, Default)]
pub struct ArchivedCache {
    pub load: LoadState,
    pub tasks: Vec<consort_proto::model::Task>,
    pub has_more: bool,
}

/// The phase 4 part of the client cache. None of it is a feed entity, so a
/// reload keeps it.
#[derive(Clone, Debug, Default)]
pub struct ConvoCache {
    pub search: SearchCache,
    /// The roots of the message threads that the client loaded.
    pub threads: BTreeSet<String>,
    pub item: Option<ItemDetailView>,
    pub archived: ArchivedCache,
}

// Channel settings -------------------------------------------------------

/// A repo of the channel settings dialog: one the hub knows, or a URL for
/// a new one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoChoice {
    Existing(String),
    New(String),
}

/// "3 workers match", "1 worker matches" or "No worker matches".
pub fn workers_match_label(n: usize) -> String {
    match n {
        0 => "No worker matches".to_owned(),
        1 => "1 worker matches".to_owned(),
        n => format!("{n} workers match"),
    }
}

/// "2 connections and 4 skills." and "1 skill is off here." for the
/// Connectors and skills row of the channel settings.
pub fn extensions_summary(state: &ViewState, channel_id: &str) -> (String, Option<String>) {
    let Some(channel) = state.channel(channel_id) else {
        return (String::new(), None);
    };
    let connections = state.connections.len();
    let skills = state.skills.len();
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let first = format!(
        "{} and {}.",
        plural(connections, "connection", "connections"),
        plural(skills, "skill", "skills")
    );
    let off = channel.disabled_connections.len() + channel.disabled_skills.len();
    let second = match (
        channel.disabled_connections.len(),
        channel.disabled_skills.len(),
    ) {
        (0, 0) => None,
        (0, s) => Some(format!("{} off here.", plural(s, "skill is", "skills are"))),
        (c, 0) => Some(format!(
            "{} off here.",
            plural(c, "connection is", "connections are")
        )),
        _ => Some(format!("{off} items are off here.")),
    };
    (first, second)
}

// New task --------------------------------------------------------------

/// Where a new task runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RunsOn {
    /// Any worker with the channel's tags. Placement picks one.
    #[default]
    AnyTagged,
    /// This worker.
    Worker(String),
}

/// When a new task starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartWhen {
    #[default]
    Now,
    /// At this time, in Unix milliseconds.
    At(i64),
}

/// The fields of the New task dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewTask {
    pub channel_id: String,
    pub title: String,
    pub brief: String,
    /// `None` uses the channel's task model.
    pub selection: Option<crate::view::SelectionView>,
    pub runs_on: RunsOn,
    /// `None` uses the channel's repos.
    pub repo_id: Option<String>,
    pub isolation: Option<crate::view::Isolation>,
    pub approval_mode: Option<crate::view::ApprovalMode>,
    pub start: StartWhen,
}

/// The days that the Start select offers: today and the next `days - 1`,
/// as ("Today, October 29", the local midnight in Unix milliseconds).
pub fn day_choices(now_ms: i64, days: usize) -> Vec<(String, i64)> {
    use chrono::{Duration as Days, Local, TimeZone as _};
    let Some(now) = Local.timestamp_millis_opt(now_ms).single() else {
        return Vec::new();
    };
    let today = now.date_naive();
    (0..days)
        .filter_map(|n| {
            let day = today + Days::days(i64::try_from(n).unwrap_or(0));
            let midnight = day.and_hms_opt(0, 0, 0)?;
            let local = Local.from_local_datetime(&midnight).earliest()?;
            let name = match n {
                0 => "Today".to_owned(),
                1 => "Tomorrow".to_owned(),
                _ => local.format("%A").to_string(),
            };
            Some((
                format!("{name}, {}", local.format("%B %-d")),
                local.timestamp_millis(),
            ))
        })
        .collect()
}

/// The times that the Start select offers: every 30 minutes, as ("7:00
/// AM", minutes after midnight).
pub fn time_choices() -> Vec<(String, u32)> {
    (0..48)
        .map(|half| {
            let minutes = half * 30;
            let (h, m) = (minutes / 60, minutes % 60);
            let (h12, part) = match h {
                0 => (12, "AM"),
                1..=11 => (h, "AM"),
                12 => (12, "PM"),
                _ => (h - 12, "PM"),
            };
            (format!("{h12}:{m:02} {part}"), minutes)
        })
        .collect()
}

/// The time of `minutes` after the local midnight `day_start`, in Unix
/// milliseconds. A day with a clock change still gets the wall time.
pub fn start_time(day_start: i64, minutes: u32) -> i64 {
    use chrono::{Local, TimeZone as _};
    let Some(day) = Local.timestamp_millis_opt(day_start).single() else {
        return day_start + i64::from(minutes) * 60_000;
    };
    let wall = day
        .date_naive()
        .and_hms_opt(minutes / 60, minutes % 60, 0)
        .and_then(|t| Local.from_local_datetime(&t).earliest());
    wall.map(|t| t.timestamp_millis())
        .unwrap_or(day_start + i64::from(minutes) * 60_000)
}

/// The line under "Where it runs": which workers match the channel's tags.
pub fn placement_caption(state: &ViewState, channel_id: &str, runs_on: &RunsOn) -> String {
    let Some(channel) = state.channel(channel_id) else {
        return String::new();
    };
    match runs_on {
        RunsOn::Worker(id) => match state.worker(id) {
            Some(w) if w.online => format!("The task runs on {}.", w.name),
            Some(w) => format!(
                "The task runs on {} when it comes online. It waits as Up next until then.",
                w.name
            ),
            None => "Choose a worker.".to_owned(),
        },
        RunsOn::AnyTagged => {
            let matching = state.workers_with_tags(&channel.tags);
            let names: Vec<&str> = matching.iter().map(|w| w.name.as_str()).collect();
            let which = if channel.tags.is_empty() {
                "The channel has no tags, so any worker can run it.".to_owned()
            } else {
                "The channel's tags.".to_owned()
            };
            match names.len() {
                0 => format!("{which} No worker matches. Add the tags to a worker first."),
                1 => format!("{which} 1 worker matches: {}.", names[0]),
                n => format!(
                    "{which} {n} workers match: {}. Consort picks one that has the repo.",
                    join_names(&names)
                ),
            }
        }
    }
}

/// "a, b and c".
pub fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The line under the approval mode choice.
pub fn approval_caption(mode: crate::view::ApprovalMode) -> &'static str {
    use crate::view::ApprovalMode as M;
    match mode {
        M::Ask => {
            "The task asks before it edits a file or runs a command. A worker can set a lower limit."
        }
        M::AutoEdits => {
            "The task edits files without asking. It asks before it runs a command. A worker can set a lower limit."
        }
        M::Auto => {
            "On Claude Code, the task asks only for destructive actions and for actions outside the workspace. On Codex, it runs commands in the harness sandbox. On Gemini CLI and OpenCode, it still asks before commands. A worker can set a lower limit."
        }
        M::FullAccess => {
            "The task runs without asking and without a sandbox. A worker can set a lower limit."
        }
    }
}

/// The line in the footer of the New task dialog.
pub fn start_caption(start: StartWhen, channel_name: &str) -> String {
    match start {
        StartWhen::Now => format!("The task starts at once in {channel_name}."),
        StartWhen::At(_) => format!("Until then it waits as Scheduled in {channel_name}."),
    }
}

/// Whether Retry shows for a task: a failed task, or a task in Ready for
/// review or Resolved whose session has ended.
pub fn can_retry(status: TaskStatus, session_ended: bool) -> bool {
    match status {
        TaskStatus::Failed => true,
        TaskStatus::Review | TaskStatus::Resolved => session_ended,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::time::Timestamp;

    fn reaction(author: Author, emoji: &str) -> Reaction {
        Reaction {
            author,
            emoji: emoji.into(),
            created_at: Timestamp::from_millis(1),
        }
    }

    #[test]
    fn the_auto_caption_says_what_auto_does_on_each_harness() {
        let caption = approval_caption(crate::view::ApprovalMode::Auto);
        assert!(caption.contains("On Claude Code"), "{caption}");
        assert!(caption.contains("On Gemini CLI and OpenCode"), "{caption}");
        assert!(!caption.contains("without asking, inside"), "{caption}");
    }

    #[test]
    fn reactions_make_one_chip_per_emoji_and_count_only_when_both_chose_it() {
        let chips = reaction_views(&[
            reaction(Author::User, "🙏"),
            reaction(Author::Assistant, "👍"),
            reaction(Author::User, "👍"),
            reaction(Author::System, "🎉"),
        ]);
        assert_eq!(chips.len(), 2);
        assert_eq!(chips[0].emoji, "🙏");
        assert!(chips[0].mine && !chips[0].assistant);
        assert_eq!(chips[0].count_label(), None);
        assert!(chips[1].mine && chips[1].assistant);
        assert_eq!(chips[1].count_label().as_deref(), Some("2"));
    }

    #[test]
    fn the_quick_emoji_are_in_the_picker_too() {
        let all: Vec<&str> = EMOJI_GROUPS
            .iter()
            .flat_map(|(_, e)| e.iter().copied())
            .collect();
        for emoji in QUICK_EMOJI {
            assert!(all.contains(&emoji), "{emoji} is not in the picker");
        }
    }

    #[test]
    fn highlight_bolds_the_part_of_each_word_that_a_query_word_starts() {
        let words = query_words("windows eng");
        let runs = highlight("Build the Windows engine on bigboi.", &words);
        assert_eq!(
            runs,
            vec![
                ("Build the ".to_owned(), false),
                ("Windows".to_owned(), true),
                (" ".to_owned(), false),
                ("eng".to_owned(), true),
                ("ine on bigboi.".to_owned(), false),
            ]
        );
        assert_eq!(highlight("", &words), vec![]);
    }

    #[test]
    fn local_results_match_every_word_as_a_prefix() {
        let mut state = ViewState::default();
        state.channels.push(crate::view::ChannelView {
            id: "c1".into(),
            name: "sayso".into(),
            purpose: "Keep Sayso shipping on macOS, Windows and Linux".into(),
            icon: "mic".into(),
            ..Default::default()
        });
        state.channels.push(crate::view::ChannelView {
            id: "c2".into(),
            name: "homelab".into(),
            ..Default::default()
        });
        state.tasks.push(crate::view::TaskView {
            id: "t1".into(),
            title: "Build the Windows engine on bigboi".into(),
            channel_name: "sayso".into(),
            status: TaskStatus::NeedsInput,
            ..Default::default()
        });
        let hits = local_results(&state, "windows engine", 5);
        assert_eq!(hits.len(), 1, "{hits:?}");
        let hits = local_results(&state, "win", 5);
        assert_eq!(hits.len(), 2);
        assert!(matches!(&hits[0], LocalHit::Channel { id, .. } if id == "c1"));
        assert!(matches!(&hits[1], LocalHit::Task { id, .. } if id == "t1"));
        assert!(local_results(&state, "  ", 5).is_empty());
    }

    #[test]
    fn tab_steps_through_the_scopes() {
        assert_eq!(SearchScope::All.next(false), SearchScope::Messages);
        assert_eq!(SearchScope::Memory.next(false), SearchScope::All);
        assert_eq!(SearchScope::All.next(true), SearchScope::Memory);
        assert!(SearchScope::All.kinds().is_empty());
        assert_eq!(
            SearchScope::Memory.kinds(),
            vec![consort_proto::tools::SearchKind::MemoryNote]
        );
    }

    #[test]
    fn a_diff_gets_old_and_new_line_numbers() {
        let lines = diff_lines(
            "--- a/src/tray.rs\n+++ b/src/tray.rs\n@@ -80,3 +80,4 @@ fn draw()\n fn draw() {\n-    let size = 16;\n+    let scale = 2;\n+    let size = 32;\n }\n",
        );
        assert_eq!(lines[0], DiffLine::File("src/tray.rs".into()));
        assert!(matches!(&lines[1], DiffLine::Hunk(h) if h.starts_with("@@ -80,3")));
        assert_eq!(
            lines[2],
            DiffLine::Context {
                old: 80,
                new: 80,
                text: "fn draw() {".into()
            }
        );
        assert_eq!(
            lines[3],
            DiffLine::Removed {
                old: 81,
                text: "    let size = 16;".into()
            }
        );
        assert_eq!(
            lines[5],
            DiffLine::Added {
                new: 82,
                text: "    let size = 32;".into()
            }
        );
        assert_eq!(
            lines[6],
            DiffLine::Context {
                old: 82,
                new: 83,
                text: "}".into()
            }
        );
    }

    #[test]
    fn a_text_diff_of_two_versions_marks_the_changed_lines() {
        let lines = text_diff("SKILL.md", Some("a\nb\nc\n"), Some("a\nB\nc\nd\n"));
        assert_eq!(lines[0], DiffLine::File("SKILL.md".into()));
        assert!(lines.contains(&DiffLine::Removed {
            old: 2,
            text: "b".into()
        }));
        assert!(lines.contains(&DiffLine::Added {
            new: 2,
            text: "B".into()
        }));
        assert!(lines.contains(&DiffLine::Added {
            new: 4,
            text: "d".into()
        }));
        let added = text_diff("new.sh", None, Some("echo hi\n"));
        assert!(added.contains(&DiffLine::Added {
            new: 1,
            text: "echo hi".into()
        }));
    }

    #[test]
    fn output_lines_and_labels() {
        assert_eq!(output_lines("a\r\nb\n"), vec!["a", "b"]);
        assert_eq!(output_lines("a\n\nb"), vec!["a", "", "b"]);
        assert_eq!(thousands(1284), "1,284");
        assert_eq!(thousands(12), "12");
        assert_eq!(thousands(1_000_000), "1,000,000");
        assert_eq!(
            output_caption(1284, "bigboi"),
            "1,284 lines from bigboi. The end shows first. Scroll up for the start."
        );
        assert_eq!(duration_label(134_000), "2 min 14 s");
        assert_eq!(duration_label(8_000), "8 s");
        assert_eq!(duration_label(120_000), "2 min");
        assert_eq!(duration_label(3_840_000), "1 h 4 min");
        assert_eq!(reply_count_label(1), "1 reply");
        assert_eq!(reply_count_label(4), "4 replies");
    }

    #[test]
    fn retry_shows_for_failed_and_for_an_ended_session_in_review_or_resolved() {
        assert!(can_retry(TaskStatus::Failed, false));
        assert!(!can_retry(TaskStatus::Review, false));
        assert!(can_retry(TaskStatus::Review, true));
        assert!(can_retry(TaskStatus::Resolved, true));
        assert!(!can_retry(TaskStatus::Working, true));
    }

    #[test]
    fn start_choices_name_the_day_and_the_wall_time() {
        let now = crate::mock::MOCK_NOW;
        let days = day_choices(now, 3);
        assert_eq!(days.len(), 3);
        assert!(days[0].0.starts_with("Today, "));
        assert!(days[1].0.starts_with("Tomorrow, "));
        assert!(days[0].1 <= now && now - days[0].1 < 86_400_000);
        let times = time_choices();
        assert_eq!(times.len(), 48);
        assert_eq!(times[0], ("12:00 AM".to_owned(), 0));
        assert_eq!(times[14], ("7:00 AM".to_owned(), 420));
        assert_eq!(times[25], ("12:30 PM".to_owned(), 750));
        let at = start_time(days[1].1, 420);
        assert_eq!(crate::format::time_label(at), "7:00 AM");
    }

    #[test]
    fn names_join_with_and() {
        assert_eq!(join_names(&["a"]), "a");
        assert_eq!(join_names(&["a", "b", "c"]), "a, b and c");
    }
}
