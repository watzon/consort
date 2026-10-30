//! The mock content and commands of phase 4: the thread of a Tutti
//! message with reactions, the search results, the task item view, the
//! archived tasks, and the channel settings, as the designs show them
//! (Paper `95W-0`, `9I9-0`, `A0N-0`, `AFG-0`, `CA5-0`, `CPJ-0`, `CYJ-0`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::{
    ClientError, Command, CommandOutput,
    connectors::LoadState,
    convo::{
        ArchivedTaskView, ItemDetailState, ItemDetailView, ItemKind, MessageThreadView,
        ReactionView, RepoChoice, RunsOn, SearchHitView, SearchLoad, SearchScope, SearchSection,
        SearchTarget, StartWhen, query_words,
    },
    format,
    mock_backend_ext::Later,
    view::*,
};

const TUTTI: &str = "chn_tutti";
const SAYSO: &str = "chn_sayso";
const ROOT: &str = "msg_tutti_reply";
const T_SAYSO: &str = "tsk_0192f0c4-sayso-issues";
const T_BACKUPS: &str = "tsk_0192f0c4-nightly-backups";

/// The command item of the item view design (`CA5-0`).
pub const ITEM_BUILD: &str = "itm_cargo_build";
/// The diff item of the item view design (`CPJ-0`).
pub const ITEM_DIFF: &str = "itm_tray_diff";
/// A command on truenas, which is offline (`CPJ-0`).
pub const ITEM_OFFLINE: &str = "itm_zfs_send";
/// A command whose output loads (`CPJ-0`).
pub const ITEM_TEST: &str = "itm_cargo_test";
/// The mock session of the sayso task.
pub const SESSION: &str = "ses_mock_sayso";

static HOLD_SEARCH: AtomicBool = AtomicBool::new(false);
static HOLD_ITEM: AtomicBool = AtomicBool::new(false);

/// Holds the hub search in its loading state, for the screenshot of the
/// skeleton rows.
pub fn hold_search(hold: bool) {
    HOLD_SEARCH.store(hold, Ordering::SeqCst);
}

/// Holds an opened item in its loading state, for a screenshot.
pub fn hold_item(hold: bool) {
    HOLD_ITEM.store(hold, Ordering::SeqCst);
}

/// How long the mock hub takes to answer a search.
const SEARCH_DELAY: Duration = Duration::from_millis(350);
/// How long the mock worker takes to give an item's output.
const ITEM_DELAY: Duration = Duration::from_millis(300);

fn text(s: &str) -> String {
    s.to_owned()
}

fn reply(id: &str, speaker: Speaker, time: &str, body: &str) -> MessageView {
    MessageView {
        id: id.into(),
        channel_id: TUTTI.into(),
        speaker,
        time_label: time.into(),
        body: body.into(),
        ..MessageView::default()
    }
}

fn line(text: &str, item_id: &str) -> ActivityLine {
    ActivityLine {
        text: text.into(),
        session_id: SESSION.into(),
        item_id: item_id.into(),
    }
}

/// Adds the phase 4 content to the mock view state.
pub(crate) fn decorate(state: &mut ViewState) {
    // The Tutti answer has a thread and reactions; your question has the
    // assistant's reaction.
    let mut root = None;
    if let Some(tutti) = state.timelines.iter_mut().find(|t| t.channel_id == TUTTI) {
        for item in &mut tutti.items {
            let TimelineItem::Message(m) = item else {
                continue;
            };
            if m.id == ROOT {
                m.reply_count = 4;
                m.last_reply_label = Some(text("Last reply 9:31 AM"));
                m.reactions = vec![
                    ReactionView {
                        emoji: text("🙏"),
                        mine: true,
                        assistant: false,
                    },
                    ReactionView {
                        emoji: text("👍"),
                        mine: true,
                        assistant: true,
                    },
                ];
                root = Some(m.clone());
            } else if m.id == "msg_tutti_you" {
                m.reactions = vec![ReactionView {
                    emoji: text("👀"),
                    mine: false,
                    assistant: true,
                }];
            }
        }
    }
    if let Some(root) = root {
        state.convo.threads.push(MessageThreadView {
            root_id: ROOT.into(),
            channel_id: TUTTI.into(),
            subtitle: text("In Tutti, 4 replies"),
            root,
            replies: vec![
                reply(
                    "msg_thread_1",
                    Speaker::You,
                    "9:24 AM",
                    "Can the build run on homelab? bigboi is busy tonight.",
                ),
                reply(
                    "msg_thread_2",
                    Speaker::Consort,
                    "9:25 AM",
                    "homelab has no Windows toolchain. I can hold the build on bigboi until 11:00 PM instead.",
                ),
                reply("msg_thread_3", Speaker::You, "9:30 AM", "Do that."),
                reply(
                    "msg_thread_4",
                    Speaker::Consort,
                    "9:31 AM",
                    "Done. The build starts on bigboi at 11:00 PM, and I post the result here.",
                ),
            ],
            loaded: true,
        });
    }

    // The activity of the sayso task opens items.
    if let Some(thread) = state.threads.iter_mut().find(|t| t.task_id == T_SAYSO) {
        for post in &mut thread.posts {
            for body in &mut post.bodies {
                if let PostBody::Activity { items, .. } = body {
                    *items = vec![
                        line("cargo test -p sayso-engine", ITEM_TEST),
                        line(
                            "cargo build --release --target x86_64-pc-windows-msvc",
                            ITEM_BUILD,
                        ),
                        line("Edited engine/src/cpu.rs", "itm_cpu_diff"),
                        line("Edited engine/src/tray.rs", ITEM_DIFF),
                        line("Edited .github/workflows/release.yml", "itm_ci_diff"),
                    ];
                }
            }
        }
    }
    if let Some(thread) = state.threads.iter_mut().find(|t| t.task_id == T_BACKUPS) {
        for post in &mut thread.posts {
            for body in &mut post.bodies {
                if let PostBody::Activity { items, .. } = body {
                    items.push(line("zfs send tank/media@2026-10-29", ITEM_OFFLINE));
                }
            }
        }
    }

    // The channel settings of sayso (`CYJ-0`).
    let site = RepoView {
        id: text("rep_sayso_site"),
        name: text("watzon/sayso-site"),
        checkouts: text("Not checked out yet. The first task clones it."),
        url: text("https://github.com/watzon/sayso-site"),
    };
    if let Some(sayso) = state.channels.iter_mut().find(|c| c.id == SAYSO) {
        sayso.instructions = text(
            "Releases go out from main only. Test Windows builds on bigboi before a PR. Ask before you change the CI workflow. Keep changelog entries short.",
        );
        sayso.repos.push(site.clone());
        sayso.allowed_local_mcp_servers = vec![text("windows-signing")];
        sayso.default_approval_mode = ApprovalMode::AutoEdits;
        sayso.icon = text("mic");
    }
    let mut repos: Vec<RepoView> = state
        .channels
        .iter()
        .flat_map(|c| c.repos.iter().cloned())
        .collect();
    repos.push(RepoView {
        id: text("rep_homelab"),
        name: text("watzon/homelab"),
        checkouts: text("Checked out on homelab"),
        url: text("https://github.com/watzon/homelab"),
    });
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    repos.dedup_by(|a, b| a.id == b.id);
    state.convo.repos = repos;

    // Two archived tasks.
    state.convo.archived.tasks = vec![
        ArchivedTaskView {
            id: text("tsk_archived_cert"),
            title: text("Renew the hub certificate"),
            channel_name: text("homelab"),
            status: TaskStatus::Resolved,
            archived_label: text("Archived October 22"),
        },
        ArchivedTaskView {
            id: text("tsk_archived_fallback"),
            title: text("Ship the CPU fallback for the Windows engine"),
            channel_name: text("sayso"),
            status: TaskStatus::Resolved,
            archived_label: text("Archived October 22"),
        },
    ];
}

/// The repos of the channel settings, from the known repos and new URLs.
pub(crate) fn channel_repos(known: &[RepoView], choices: Vec<RepoChoice>) -> Vec<RepoView> {
    choices
        .into_iter()
        .map(|choice| match choice {
            RepoChoice::Existing(id) => {
                known
                    .iter()
                    .find(|r| r.id == id)
                    .cloned()
                    .unwrap_or(RepoView {
                        id: id.clone(),
                        name: id,
                        ..RepoView::default()
                    })
            }
            RepoChoice::New(url) => {
                let name = url
                    .trim_end_matches('/')
                    .trim_end_matches(".git")
                    .rsplit(['/', ':'])
                    .take(2)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("/");
                RepoView {
                    id: format!("rep_{}", name.replace('/', "_")),
                    name,
                    checkouts: text("Not checked out yet. The first task clones it."),
                    url,
                }
            }
        })
        .collect()
}

/// The hub results that the designs show, and the mock search finds.
struct Corpus {
    hits: Vec<(SearchHitView, String)>,
}

fn corpus(state: &ViewState) -> Corpus {
    let mut hits = Vec::new();
    let channel = |id: &str| state.channel(id).cloned().unwrap_or_default();
    // Every message of the timelines and the threads.
    for timeline in &state.timelines {
        let c = channel(&timeline.channel_id);
        let tutti = c.kind == ChannelKind::Tutti;
        for item in &timeline.items {
            if let TimelineItem::Message(m) = item {
                hits.push((
                    message_hit(&c, tutti, m, None, &format!("Today {}", m.time_label)),
                    m.body.clone(),
                ));
            }
        }
    }
    for thread in &state.convo.threads {
        let c = channel(&thread.channel_id);
        let tutti = c.kind == ChannelKind::Tutti;
        for m in &thread.replies {
            hits.push((
                message_hit(
                    &c,
                    tutti,
                    m,
                    Some(thread.root_id.clone()),
                    &format!("Today {}", m.time_label),
                ),
                m.body.clone(),
            ));
        }
    }
    // An older reply in a sayso thread, which only the hub has.
    let sayso = channel(SAYSO);
    hits.push((
        message_hit(
            &sayso,
            false,
            &MessageView {
                id: text("msg_sayso_old_reply"),
                body: text(
                    "The Windows engine crashes on CPUs without AVX2. A fallback build fixes it.",
                ),
                ..MessageView::default()
            },
            Some(text("msg_sayso_fix")),
            "October 24",
        ),
        text("The Windows engine crashes on CPUs without AVX2. A fallback build fixes it."),
    ));
    // A memory note of sayso.
    let note = "Build the Windows engine on bigboi with the MSVC target. homelab has no toolchain.";
    hits.push((
        SearchHitView {
            section: SearchSection::Memory,
            target: SearchTarget::Memory {
                channel_id: Some(SAYSO.into()),
                path: text("sayso/build.md"),
            },
            title: text("sayso/build.md"),
            title_note: None,
            icon: Some(text("notebook")),
            tutti: false,
            status: None,
            snippet: text(note),
            detail: text("Changed October 25"),
        },
        text(note),
    ));
    // Tasks: the ones the mock holds, and a resolved one only the hub has.
    for task in &state.tasks {
        hits.push((
            SearchHitView {
                section: SearchSection::Tasks,
                target: SearchTarget::Task {
                    task_id: task.id.clone(),
                    channel_id: task.channel_id.clone(),
                },
                title: task.title.clone(),
                title_note: None,
                icon: None,
                tutti: false,
                status: Some(task.status),
                snippet: String::new(),
                detail: format!(
                    "{}, {}",
                    task.channel_name,
                    task.status.label().to_lowercase()
                ),
            },
            task.title.clone(),
        ));
    }
    hits.push((
        SearchHitView {
            section: SearchSection::Tasks,
            target: SearchTarget::Task {
                task_id: text("tsk_archived_fallback"),
                channel_id: SAYSO.into(),
            },
            title: text("Ship the CPU fallback for the Windows engine"),
            title_note: None,
            icon: None,
            tutti: false,
            status: Some(TaskStatus::Resolved),
            snippet: String::new(),
            detail: text("sayso, resolved October 22"),
        },
        text("Ship the CPU fallback for the Windows engine"),
    ));
    Corpus { hits }
}

fn message_hit(
    c: &ChannelView,
    tutti: bool,
    m: &MessageView,
    thread_root_id: Option<String>,
    detail: &str,
) -> SearchHitView {
    SearchHitView {
        section: SearchSection::Messages,
        target: SearchTarget::Message {
            channel_id: c.id.clone(),
            message_id: m.id.clone(),
            thread_root_id: thread_root_id.clone(),
            task_id: None,
        },
        title: if tutti { text("Tutti") } else { c.name.clone() },
        title_note: thread_root_id.map(|_| text("in a thread")),
        icon: Some(c.icon.clone()),
        tutti,
        status: None,
        snippet: String::new(),
        detail: detail.to_owned(),
    }
}

/// The sentence of `body` that holds the first word of the query, as the
/// hub's snippet does.
fn snippet(body: &str, words: &[String]) -> String {
    let plain = format::plain_text(body);
    let sentences: Vec<&str> = plain
        .split_inclusive(['.', '?', '!'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let Some(at) = sentences
        .iter()
        .position(|s| crate::convo::matches_all(s, &words[..1]))
    else {
        return plain.trim().to_owned();
    };
    // Like the hub's snippet: some words before the match, when they fit.
    match at.checked_sub(1).map(|i| sentences[i]) {
        Some(before) if before.len() + sentences[at].len() < 120 => {
            format!("{before} {}", sentences[at])
        }
        _ => sentences[at].to_owned(),
    }
}

/// What the mock hub finds for a query.
fn search(state: &ViewState, query: &str, scope: SearchScope) -> Vec<SearchHitView> {
    let words = query_words(query);
    if words.is_empty() {
        return Vec::new();
    }
    let wanted = |section: SearchSection| match scope {
        SearchScope::All => true,
        SearchScope::Messages => section == SearchSection::Messages,
        SearchScope::Tasks => section == SearchSection::Tasks,
        SearchScope::Memory => section == SearchSection::Memory,
    };
    corpus(state)
        .hits
        .into_iter()
        .filter(|(hit, body)| wanted(hit.section) && crate::convo::matches_all(body, &words))
        .map(|(mut hit, body)| {
            if hit.section != SearchSection::Tasks {
                hit.snippet = snippet(&body, &words);
            }
            hit
        })
        .collect()
}

/// The output of `cargo build` in the design (`CA5-0`): 1,284 lines that
/// end with the ones it shows.
fn build_output() -> String {
    let mut out = String::new();
    for n in 1..=1244 {
        out.push_str(&format!(
            "   Compiling crate-{n:04} v0.{}.{}\n",
            n % 9,
            n % 13
        ));
    }
    out.push_str(
        "   Compiling serde v1.0.228
   Compiling libc v0.2.177
   Compiling memchr v2.7.6
   Compiling bytes v1.10.1
   Compiling mio v1.1.0
   Compiling socket2 v0.6.1
   Compiling tokio v1.48.0
   Compiling windows-sys v0.61.2
   Compiling ring v0.17.14
   Compiling cpufeatures v0.2.17
   Compiling sayso-audio v0.9.3 (C:\\Code\\sayso\\audio)
   Compiling whisper-rs-sys v0.14.1
warning: unused variable: `scale`
  --> engine\\src\\tray.rs:88:9
   |
88 |     let scale = monitor.scale_factor();
   |         ^^^^^ help: prefix it with an underscore
   |
   = note: `#[warn(unused_variables)]` on by default

   Compiling sayso-engine v0.9.3 (C:\\Code\\sayso\\engine)
   Compiling sayso-engine-avx2 v0.9.3 (C:\\Code\\sayso\\engine)
   Compiling sayso-engine-sse4 v0.9.3 (C:\\Code\\sayso\\engine)
   Compiling sayso-cli v0.9.3 (C:\\Code\\sayso\\cli)
   Compiling sayso v0.9.3 (C:\\Code\\sayso\\app)
warning: `sayso-engine` (lib) generated 1 warning
    Building [=======================>  ] 412/418
    Building [=========================>] 416/418
     Linking sayso.exe
     Linking sayso-cli.exe
    Finished `release` profile [optimized] target(s) in 2m 14s
     Running build script for sayso-engine
        Note: AVX2 build at target\\release\\engine-avx2.dll
        Note: SSE4 build at target\\release\\engine-sse4.dll
        Note: fallback picks SSE4 when the CPU has no AVX2
   Packaging sayso-0.9.3-x86_64-windows-msvc.zip
      Signed sayso.exe with the test certificate
      Signed sayso-cli.exe with the test certificate
     Writing target\\release\\sayso-0.9.3.zip
        Done 2 artifacts, 41.2 MB
",
    );
    out
}

/// The diff in the design (`CPJ-0`).
const TRAY_DIFF: &str = "--- a/engine/src/tray.rs
+++ b/engine/src/tray.rs
@@ -80,14 +80,22 @@ fn draw_tray_icon(monitor: &Monitor)
 fn draw_tray_icon(monitor: &Monitor) -> Result<Icon> {
     let base = load_svg(TRAY_SVG)?;
-    let size = 16;
-    let bitmap = base.render(size, size)?;
+    // Windows asks for the icon at the monitor's scale.
+    let scale = monitor.scale_factor();
+    let size = (16.0 * scale).round() as u32;
+    let bitmap = base.render(size, size)?;

-    Icon::from_rgba(bitmap.data(), 16, 16)
+    Icon::from_rgba(bitmap.data(), size, size)
+        .map_err(|e| TrayError::Icon { size, source: e })
 }

@@ -112,6 +120,12 @@ impl Tray
     pub fn on_dpi_changed(&mut self, monitor: &Monitor) {
-        // TODO: redraw
+        match draw_tray_icon(monitor) {
+            Ok(icon) => self.set_icon(icon),
+            Err(e) => warn!(\"tray icon: {e}\"),
+        }
     }
 }
";

fn item_view(state: &ViewState, task_id: &str, item_id: &str) -> ItemDetailView {
    let title = state
        .threads
        .iter()
        .flat_map(|t| t.posts.iter())
        .flat_map(|p| p.bodies.iter())
        .find_map(|b| match b {
            PostBody::Activity { items, .. } => items
                .iter()
                .find(|l| l.item_id == item_id)
                .map(|l| l.text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let diff = title.starts_with("Edited ");
    let (index, subtitle, worker) = match item_id {
        ITEM_BUILD => (
            9,
            text("Exit code 0, ran 2 min 14 s on bigboi, 7:41 PM"),
            "bigboi",
        ),
        ITEM_DIFF => (
            12,
            text("10 lines added, 4 removed, on bigboi at 7:38 PM"),
            "bigboi",
        ),
        ITEM_TEST => (8, text("Command on bigboi, 7:44 PM"), "bigboi"),
        ITEM_OFFLINE => (4, text("Command on truenas, 6:02 PM"), "truenas"),
        _ => (1, text("Command on bigboi"), "bigboi"),
    };
    ItemDetailView {
        task_id: task_id.into(),
        session_id: SESSION.into(),
        item_id: item_id.into(),
        kind: if diff {
            ItemKind::Diff
        } else {
            ItemKind::Output
        },
        title: title
            .strip_prefix("Edited ")
            .map(str::to_owned)
            .unwrap_or(title),
        worker_name: worker.into(),
        subtitle,
        lines_added: if diff { 10 } else { 0 },
        lines_removed: if diff { 4 } else { 0 },
        index,
        total: 20,
        state: ItemDetailState::Loading,
    }
}

/// Runs a phase 4 command against the mock.
pub(crate) fn apply(
    state: &mut ViewState,
    command: Command,
    next_id: &mut u64,
    later: &mut Vec<Later>,
) -> Result<CommandOutput, ClientError> {
    let mut id = |prefix: &str| {
        *next_id += 1;
        format!("{prefix}_mock_{next_id}")
    };
    match command {
        Command::React {
            message_id,
            emoji,
            remove,
        } => {
            let mut change = |m: &mut MessageView| {
                if m.id != message_id {
                    return;
                }
                match m.reactions.iter().position(|r| r.emoji == emoji) {
                    Some(i) => {
                        m.reactions[i].mine = !remove;
                        if !m.reactions[i].mine && !m.reactions[i].assistant {
                            m.reactions.remove(i);
                        }
                    }
                    None if !remove => m.reactions.push(ReactionView {
                        emoji: emoji.clone(),
                        mine: true,
                        assistant: false,
                    }),
                    None => {}
                }
            };
            for timeline in &mut state.timelines {
                for item in &mut timeline.items {
                    if let TimelineItem::Message(m) = item {
                        change(m);
                    }
                }
            }
            for thread in &mut state.convo.threads {
                change(&mut thread.root);
                thread.replies.iter_mut().for_each(&mut change);
            }
        }
        Command::LoadMessageThread { message_id } => {
            if state.convo.thread(&message_id).is_none() {
                let root = state
                    .timelines
                    .iter()
                    .flat_map(|t| t.items.iter())
                    .find_map(|i| match i {
                        TimelineItem::Message(m) if m.id == message_id => Some(m.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| ClientError::Hub {
                        code: "not_found".into(),
                        message: "That message no longer exists.".into(),
                    })?;
                let channel_name = state
                    .channel(&root.channel_id)
                    .map(|c| {
                        if c.kind == ChannelKind::Tutti {
                            text("Tutti")
                        } else {
                            c.name.clone()
                        }
                    })
                    .unwrap_or_default();
                state.convo.threads.push(MessageThreadView {
                    root_id: message_id,
                    channel_id: root.channel_id.clone(),
                    subtitle: format!(
                        "In {channel_name}, {}",
                        crate::convo::reply_count_label(root.reply_count)
                    ),
                    root,
                    replies: Vec::new(),
                    loaded: true,
                });
            }
        }
        Command::FindMessage { .. } => {}
        Command::ReplyInThread {
            root_id,
            body,
            attachments,
            ..
        } => {
            let attachments = crate::mock_files::take_ready(state, &attachments)?;
            let now = format::now_ms();
            let thread = state
                .convo
                .threads
                .iter_mut()
                .find(|t| t.root_id == root_id)
                .ok_or_else(|| ClientError::Hub {
                    code: "not_found".into(),
                    message: "That thread no longer exists.".into(),
                })?;
            thread.replies.push(MessageView {
                id: id("msg"),
                channel_id: thread.channel_id.clone(),
                speaker: Speaker::You,
                time_label: format::time_label(now),
                created_at: now,
                body,
                attachments,
                ..MessageView::default()
            });
            let count = u32::try_from(thread.replies.len()).unwrap_or(u32::MAX);
            thread.root.reply_count = count;
            let last = format!("Last reply {}", format::time_label(now));
            thread.root.last_reply_label = Some(last.clone());
            let channel_id = thread.channel_id.clone();
            let name = state
                .channel(&channel_id)
                .map(|c| {
                    if c.kind == ChannelKind::Tutti {
                        text("Tutti")
                    } else {
                        c.name.clone()
                    }
                })
                .unwrap_or_default();
            if let Some(thread) = state
                .convo
                .threads
                .iter_mut()
                .find(|t| t.root_id == root_id)
            {
                thread.subtitle = format!("In {name}, {}", crate::convo::reply_count_label(count));
            }
            for timeline in &mut state.timelines {
                for item in &mut timeline.items {
                    if let TimelineItem::Message(m) = item
                        && m.id == root_id
                    {
                        m.reply_count = count;
                        m.last_reply_label = Some(last.clone());
                    }
                }
            }
        }
        Command::Search { query, scope } => {
            let search = &mut state.convo.search;
            search.query = query.clone();
            search.scope = scope;
            search.hits.clear();
            if query_words(&query).is_empty() {
                search.load = SearchLoad::Idle;
            } else {
                search.load = SearchLoad::Loading;
                if !HOLD_SEARCH.load(Ordering::SeqCst) {
                    later.push((
                        SEARCH_DELAY,
                        Box::new(move |s: &mut ViewState| {
                            if s.convo.search.query == query && s.convo.search.scope == scope {
                                s.convo.search.hits = search_hits(s, &query, scope);
                                s.convo.search.load = SearchLoad::Done;
                            }
                        }),
                    ));
                }
            }
        }
        Command::ClearSearch => state.convo.search = Default::default(),
        Command::CreateTask(new) => {
            if new.title.trim().is_empty() {
                return Err(invalid("The task needs a title."));
            }
            if new.brief.trim().is_empty() {
                return Err(invalid(
                    "The task needs a brief. Say what to do and what done means.",
                ));
            }
            let channel = state
                .channel(&new.channel_id)
                .cloned()
                .ok_or_else(|| invalid("That channel no longer exists."))?;
            let task_id = id("tsk");
            let card = format!("msg_card_{task_id}");
            let now = format::now_ms();
            let status = match new.start {
                StartWhen::Now => TaskStatus::Working,
                StartWhen::At(_) => TaskStatus::Scheduled,
            };
            let worker = match &new.runs_on {
                RunsOn::Worker(w) => state.worker(w).map(|w| w.name.clone()),
                RunsOn::AnyTagged => state
                    .workers_with_tags(&channel.tags)
                    .into_iter()
                    .find(|w| w.online)
                    .map(|w| w.name.clone()),
            };
            let reason = match new.start {
                StartWhen::At(at) => Some(format!(
                    "Starts {} at {}",
                    format::day_label(at, now).to_lowercase(),
                    format::time_label(at)
                )),
                StartWhen::Now => None,
            };
            let mode = new.approval_mode.unwrap_or(channel.default_approval_mode);
            state.tasks.insert(
                0,
                TaskView {
                    id: task_id.clone(),
                    channel_id: channel.id.clone(),
                    channel_name: channel.name.clone(),
                    title: new.title.trim().to_owned(),
                    status,
                    worker_name: worker.clone(),
                    account_label: new.selection.as_ref().map(|s| s.account_label.clone()),
                    status_reason: reason,
                    age_label: text("Now"),
                    updated_at: now,
                    card_message_id: card.clone(),
                    approval_mode: mode,
                    ..TaskView::default()
                },
            );
            state.threads.push(TaskThreadView {
                task_id: task_id.clone(),
                title: new.title.trim().to_owned(),
                status,
                brief: new.brief.clone(),
                approval_mode: mode,
                model_label: new.selection.as_ref().map(|s| s.label()),
                ..TaskThreadView::default()
            });
            if let Some(timeline) = state
                .timelines
                .iter_mut()
                .find(|t| t.channel_id == channel.id)
            {
                timeline.items.push(TimelineItem::TaskCard(TaskCardView {
                    message_id: card,
                    task_id,
                    time_label: format::time_label(now),
                    created_at: now,
                    title: new.title.trim().to_owned(),
                    status,
                    worker: worker.map(|name| WorkerChip {
                        name,
                        online: true,
                        account: String::new(),
                    }),
                    summary: status.label().to_owned(),
                    approval_mode: mode,
                    ..TaskCardView::default()
                }));
            }
        }
        Command::DeleteTask { task_id } => {
            if !state.tasks.iter().any(|t| t.id == task_id) {
                return Err(invalid("That task no longer exists."));
            }
            state.tasks.retain(|t| t.id != task_id);
            state.threads.retain(|t| t.task_id != task_id);
            for timeline in &mut state.timelines {
                timeline.items.retain(
                    |item| !matches!(item, TimelineItem::TaskCard(c) if c.task_id == task_id),
                );
            }
        }
        Command::RestoreTask { task_id } => {
            let Some(at) = state
                .convo
                .archived
                .tasks
                .iter()
                .position(|t| t.id == task_id)
            else {
                return Err(invalid("That task is not archived any more."));
            };
            let task = state.convo.archived.tasks.remove(at);
            let channel_id = state
                .channels
                .iter()
                .find(|c| c.name == task.channel_name)
                .map(|c| c.id.clone())
                .unwrap_or_default();
            state.tasks.push(TaskView {
                id: task.id,
                channel_id,
                channel_name: task.channel_name,
                title: task.title,
                status: task.status,
                age_label: text("Now"),
                updated_at: format::now_ms(),
                ..TaskView::default()
            });
        }
        Command::LoadArchivedTasks { .. } => {
            state.convo.archived.load = LoadState::Loaded;
            state.convo.archived.has_more = false;
        }
        Command::OpenTaskItem {
            task_id, item_id, ..
        } => {
            let mut view = item_view(state, &task_id, &item_id);
            if item_id == ITEM_OFFLINE {
                view.state = ItemDetailState::Offline;
            } else if !HOLD_ITEM.load(Ordering::SeqCst) {
                let text = if view.kind == ItemKind::Diff {
                    TRAY_DIFF.to_owned()
                } else {
                    build_output()
                };
                let wanted = item_id.clone();
                later.push((
                    ITEM_DELAY,
                    Box::new(move |s: &mut ViewState| {
                        if let Some(item) = s.convo.item.as_mut()
                            && item.item_id == wanted
                        {
                            item.state = ItemDetailState::Loaded {
                                text: Arc::new(text),
                            };
                        }
                    }),
                ));
            }
            state.convo.item = Some(view);
        }
        Command::CloseTaskItem => state.convo.item = None,
        Command::DeleteGroup { group_id } => {
            if !state.groups.iter().any(|g| g.id == group_id) {
                return Err(invalid("That group no longer exists."));
            }
            state.groups.retain(|g| g.id != group_id);
            state.sidebar.groups.retain(|g| g.id != group_id);
            for channel in &mut state.channels {
                if channel.group_id.as_deref() == Some(group_id.as_str()) {
                    channel.group_id = None;
                }
            }
        }
        Command::RenameGroup { group_id, name } => {
            let name = name.trim().to_owned();
            if name.is_empty() {
                return Err(invalid("Enter a name for the group."));
            }
            for g in &mut state.groups {
                if g.id == group_id {
                    g.name = name.clone();
                }
            }
            for g in &mut state.sidebar.groups {
                if g.id == group_id {
                    g.name = name.clone();
                }
            }
        }
        Command::ReorderGroups { group_ids } => {
            let mut ordered = Vec::new();
            for id in &group_ids {
                if let Some(g) = state.groups.iter().find(|g| &g.id == id) {
                    ordered.push(g.clone());
                }
            }
            for g in &state.groups {
                if !group_ids.contains(&g.id) {
                    ordered.push(g.clone());
                }
            }
            state.groups = ordered;
            let mut sidebar = Vec::new();
            for g in &state.groups {
                if let Some(s) = state.sidebar.groups.iter().find(|s| s.id == g.id) {
                    sidebar.push(s.clone());
                }
            }
            state.sidebar.groups = sidebar;
        }
        other => {
            return Err(ClientError::Other(format!(
                "The mock cannot run {other:?}."
            )));
        }
    }
    Ok(CommandOutput::Done)
}

fn search_hits(state: &ViewState, query: &str, scope: SearchScope) -> Vec<SearchHitView> {
    search(state, query, scope)
}

fn invalid(message: &str) -> ClientError {
    ClientError::Hub {
        code: "invalid".into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_finds_the_design_results_for_windows_engine() {
        let state = crate::mock::view_state();
        let hits = search(&state, "windows engine", SearchScope::All);
        let sections: Vec<SearchSection> = hits.iter().map(|h| h.section).collect();
        assert!(sections.contains(&SearchSection::Messages));
        assert!(sections.contains(&SearchSection::Memory));
        assert!(sections.contains(&SearchSection::Tasks));
        let tutti = hits.iter().find(|h| h.tutti).unwrap();
        assert_eq!(
            tutti.snippet,
            "One step is left. Codex wants to build the Windows engine on bigboi, and it waits for your OK."
        );
        let memory = search(&state, "windows engine", SearchScope::Memory);
        assert!(memory.iter().all(|h| h.section == SearchSection::Memory));
        assert!(search(&state, "zfs snapshot", SearchScope::All).is_empty());
    }

    #[test]
    fn new_repo_urls_get_owner_and_name() {
        let repos = channel_repos(
            &[],
            vec![RepoChoice::New(text("git@github.com:watzon/notes.git"))],
        );
        assert_eq!(repos[0].name, "watzon/notes");
    }
}
