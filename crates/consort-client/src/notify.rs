//! Notification rules: what makes a desktop notification, what it says, and
//! the count on the Dock icon.
//!
//! The rules are pure logic, so every client uses the same ones and they
//! have unit tests. The desktop app only delivers what [`Notifier::observe`]
//! gives it. The inputs are an [`AttentionView`] (built from the cache in
//! [`attention`]), the [`Focus`] of the window, and the user's
//! [`NotificationSettings`], which live on this client, not on the hub.
//!
//! - One event gives one notification: each approval, decision and message
//!   notifies once, and a task notifies when it moves into Ready for review
//!   or Failed.
//! - Nothing that was there when the data first loaded notifies.
//! - Nothing notifies while its place is on screen in the active window: the
//!   open channel for a message, the open task thread for a task.
//! - Message text shows only when the user turns on "Show message text".

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::view::TaskStatus;

/// The switches of Settings > Notifications. They live on this client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    /// An approval or a decision waits for the user.
    pub waiting: bool,
    /// A task is in Ready for review.
    pub review: bool,
    /// A task failed.
    pub failed: bool,
    /// The assistant posted in a channel that is not on screen.
    pub messages: bool,
    /// A routine posted a result.
    pub routines: bool,
    /// Show what was written. Off by default: a notification then names
    /// the channel and what happened, but not the text.
    pub show_text: bool,
    /// Show the count of what waits on the Dock icon.
    pub dock_badge: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            waiting: true,
            review: true,
            failed: true,
            messages: true,
            routines: false,
            show_text: false,
            dock_badge: true,
        }
    }
}

/// What a notification is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NotificationKind {
    Approval,
    Decision,
    Review,
    Failed,
    Message,
    Routine,
    /// Test notification in Settings > Notifications.
    Test,
}

/// Where a click on a notification goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotificationTarget {
    /// The thread of a task.
    Task { channel_id: String, task_id: String },
    /// A channel's timeline.
    Channel { channel_id: String },
    /// Nowhere: the app only comes to the front.
    App,
}

/// One notification to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// A stable tag. A notification with the same tag replaces the old one,
    /// and a click gives the tag back.
    pub tag: String,
    pub kind: NotificationKind,
    /// The channel name.
    pub title: String,
    pub body: String,
    pub target: NotificationTarget,
}

/// What is on screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Focus {
    /// The channel whose timeline shows.
    pub channel_id: Option<String>,
    /// The task whose thread shows.
    pub task_id: Option<String>,
    /// True while the app window is the active window.
    pub active: bool,
}

/// An approval or a decision that waits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaitingItem {
    pub id: String,
    pub decision: bool,
    pub channel_id: String,
    pub channel_name: String,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    /// The approval summary or the decision question.
    pub text: String,
}

/// A task and its status.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskState {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub title: String,
    pub status: TaskStatus,
    /// Why it failed.
    pub reason: Option<String>,
    /// The review summary.
    pub summary: Option<String>,
}

/// A timeline message from the assistant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IncomingMessage {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    /// A routine run posted it.
    pub routine: bool,
    /// The text without Markdown.
    pub text: String,
    /// When the hub stored it, in Unix milliseconds.
    pub created_at: i64,
}

/// What needs the user, for notifications and the Dock badge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttentionView {
    /// True once the first sync is done. Until then nothing notifies.
    pub loaded: bool,
    /// The approvals and decisions that wait, and the tasks in Ready for
    /// review: the Dock badge.
    pub count: u32,
    pub waiting: Vec<WaitingItem>,
    pub tasks: Vec<TaskState>,
    /// The newest assistant messages in timelines, oldest first.
    pub messages: Vec<IncomingMessage>,
}

/// How many assistant messages the attention view keeps.
const RECENT_MESSAGES: usize = 50;
/// The longest text in a notification body.
const BODY_MAX_CHARS: usize = 200;

/// Builds the attention view from the cache.
pub(crate) fn attention(cache: &Cache) -> AttentionView {
    use consort_proto::model::{ApprovalStatus, Author, DecisionStatus, MessageKind};
    let channel_name = |id: &str| {
        cache
            .channels
            .get(id)
            .map(|c| {
                if c.is_tutti() {
                    crate::view::TUTTI.to_owned()
                } else {
                    c.name.clone()
                }
            })
            .unwrap_or_default()
    };
    let task_title = |id: &Option<consort_proto::ids::TaskId>| {
        id.as_ref()
            .and_then(|t| cache.tasks.get(&t.to_string()))
            .map(|t| t.title.clone())
    };
    let mut waiting: Vec<(i64, WaitingItem)> = Vec::new();
    for a in cache.approvals.values() {
        if a.status == ApprovalStatus::Pending {
            waiting.push((
                a.created_at.as_millis(),
                WaitingItem {
                    id: a.id.to_string(),
                    decision: false,
                    channel_id: a.channel_id.to_string(),
                    channel_name: channel_name(&a.channel_id.to_string()),
                    task_id: a.task_id.map(|t| t.to_string()),
                    task_title: task_title(&a.task_id),
                    text: a.summary.clone(),
                },
            ));
        }
    }
    for d in cache.decisions.values() {
        if d.status == DecisionStatus::Pending {
            waiting.push((
                d.created_at.as_millis(),
                WaitingItem {
                    id: d.id.to_string(),
                    decision: true,
                    channel_id: d.channel_id.to_string(),
                    channel_name: channel_name(&d.channel_id.to_string()),
                    task_id: d.task_id.map(|t| t.to_string()),
                    task_title: task_title(&d.task_id),
                    text: d.question.clone(),
                },
            ));
        }
    }
    waiting.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
    let waiting: Vec<WaitingItem> = waiting.into_iter().map(|(_, w)| w).collect();
    let tasks: Vec<TaskState> = cache
        .tasks
        .values()
        .filter(|t| t.archived_at.is_none())
        .map(|t| TaskState {
            id: t.id.to_string(),
            channel_id: t.channel_id.to_string(),
            channel_name: channel_name(&t.channel_id.to_string()),
            title: t.title.clone(),
            status: crate::reduce::task_status(t.status),
            reason: t.status_reason.clone(),
            summary: t.review_summary.clone(),
        })
        .collect();
    let mut messages: Vec<IncomingMessage> = cache
        .messages
        .values()
        .filter(|m| {
            m.author == Author::Assistant
                && m.kind == MessageKind::Text
                && m.thread_root_id.is_none()
                && m.deleted_at.is_none()
        })
        .map(|m| IncomingMessage {
            id: m.id.to_string(),
            channel_id: m.channel_id.to_string(),
            channel_name: channel_name(&m.channel_id.to_string()),
            routine: m.author_routine_id.is_some(),
            text: crate::format::plain_text(&m.body),
            created_at: m.created_at.as_millis(),
        })
        .collect();
    messages.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let skip = messages.len().saturating_sub(RECENT_MESSAGES);
    messages.drain(..skip);
    let review = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Review)
        .count();
    AttentionView {
        loaded: cache.loaded,
        count: u32::try_from(waiting.len() + review).unwrap_or(u32::MAX),
        waiting,
        tasks,
        messages,
    }
}

/// Cuts a text for a notification body.
fn short(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= BODY_MAX_CHARS {
        return flat;
    }
    let mut out: String = flat.chars().take(BODY_MAX_CHARS - 1).collect();
    out.push('…');
    out
}

/// What a waiting item says.
fn waiting_body(item: &WaitingItem, show_text: bool) -> String {
    let what = if item.decision {
        "A decision"
    } else {
        "An approval"
    };
    match (&item.task_title, show_text) {
        (Some(task), false) => format!("{what} waits in {task}."),
        (None, false) => format!("{what} waits for you."),
        (Some(task), true) => short(&format!("{task}: {}", item.text)),
        (None, true) => short(&item.text),
    }
}

/// The notification for a waiting item.
pub fn waiting_notification(item: &WaitingItem, show_text: bool) -> Notification {
    let target = match &item.task_id {
        Some(task_id) => NotificationTarget::Task {
            channel_id: item.channel_id.clone(),
            task_id: task_id.clone(),
        },
        None => NotificationTarget::Channel {
            channel_id: item.channel_id.clone(),
        },
    };
    Notification {
        tag: format!(
            "consort:{}:{}",
            if item.decision {
                "decision"
            } else {
                "approval"
            },
            item.id
        ),
        kind: if item.decision {
            NotificationKind::Decision
        } else {
            NotificationKind::Approval
        },
        title: item.channel_name.clone(),
        body: waiting_body(item, show_text),
        target,
    }
}

fn task_notification(task: &TaskState, show_text: bool) -> Notification {
    let (kind, word, extra) = if task.status == TaskStatus::Review {
        (
            NotificationKind::Review,
            "review",
            task.summary.as_deref().map(crate::format::card_summary),
        )
    } else {
        (NotificationKind::Failed, "failed", task.reason.clone())
    };
    let base = if kind == NotificationKind::Review {
        format!("{} is ready for review.", task.title)
    } else {
        format!("{} failed.", task.title)
    };
    let body = match extra.filter(|e| show_text && !e.trim().is_empty()) {
        Some(extra) => short(&format!("{base} {extra}")),
        None => base,
    };
    Notification {
        tag: format!("consort:task:{}:{word}", task.id),
        kind,
        title: task.channel_name.clone(),
        body,
        target: NotificationTarget::Task {
            channel_id: task.channel_id.clone(),
            task_id: task.id.clone(),
        },
    }
}

fn message_notification(message: &IncomingMessage, show_text: bool) -> Notification {
    let (kind, hidden) = if message.routine {
        (NotificationKind::Routine, "A routine posted a result.")
    } else {
        (NotificationKind::Message, "The assistant posted a message.")
    };
    Notification {
        tag: format!("consort:message:{}", message.id),
        kind,
        title: message.channel_name.clone(),
        body: if show_text && !message.text.trim().is_empty() {
            short(&message.text)
        } else {
            hidden.to_owned()
        },
        target: NotificationTarget::Channel {
            channel_id: message.channel_id.clone(),
        },
    }
}

/// The notification of the Test notification button.
pub fn test_notification() -> Notification {
    Notification {
        tag: "consort:test".to_owned(),
        kind: NotificationKind::Test,
        title: "Consort".to_owned(),
        body: "This is a test. Notifications from Consort work on this computer.".to_owned(),
        target: NotificationTarget::App,
    }
}

/// The example banner of Settings > Notifications: the first item that
/// waits, or a sample, with the text as the settings say.
pub fn example_notification(
    attention: &AttentionView,
    settings: &NotificationSettings,
) -> Notification {
    let sample = WaitingItem {
        id: "example".to_owned(),
        decision: false,
        channel_id: String::new(),
        channel_name: "sayso".to_owned(),
        task_id: Some("example".to_owned()),
        task_title: Some("Go through open Sayso issues".to_owned()),
        text: "Run gh issue list --repo watzon/sayso --state open".to_owned(),
    };
    let item = attention
        .waiting
        .iter()
        .find(|w| w.task_id.is_some())
        .unwrap_or(&sample);
    waiting_notification(item, settings.show_text)
}

/// The caption next to the example banner.
pub fn example_caption(example: &Notification, settings: &NotificationSettings) -> String {
    let text = if settings.show_text {
        "With message text on."
    } else {
        "With message text off."
    };
    let click = match (&example.kind, &example.target) {
        (NotificationKind::Approval, NotificationTarget::Task { .. }) => {
            "A click opens the task thread with the approval."
        }
        (NotificationKind::Decision, NotificationTarget::Task { .. }) => {
            "A click opens the task thread with the decision."
        }
        _ => "A click opens the channel.",
    };
    format!("{text} {click}")
}

/// Whether the place of a notification is on screen in the active window.
fn on_screen(target: &NotificationTarget, focus: &Focus) -> bool {
    if !focus.active {
        return false;
    }
    match target {
        NotificationTarget::Task { task_id, .. } => focus.task_id.as_deref() == Some(task_id),
        NotificationTarget::Channel { channel_id } => {
            focus.channel_id.as_deref() == Some(channel_id)
        }
        NotificationTarget::App => false,
    }
}

/// Turns changes of the attention view into notifications, each event once.
#[derive(Clone, Debug, Default)]
pub struct Notifier {
    primed: bool,
    /// The waiting items that were seen.
    seen_waiting: BTreeSet<String>,
    /// The last status of each task.
    task_status: BTreeMap<String, TaskStatus>,
    /// Messages at or before this time are known.
    messages_until: i64,
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// The notifications for what changed since the last call. The first
    /// call after the data loaded only learns what is there.
    pub fn observe(
        &mut self,
        attention: &AttentionView,
        focus: &Focus,
        settings: &NotificationSettings,
    ) -> Vec<Notification> {
        if !attention.loaded {
            return Vec::new();
        }
        let first = !self.primed;
        self.primed = true;
        let mut out = Vec::new();
        let keep = |notification: Notification, wanted: bool, out: &mut Vec<Notification>| {
            if !first && wanted && !on_screen(&notification.target, focus) {
                out.push(notification);
            }
        };

        let pending: BTreeSet<String> = attention.waiting.iter().map(|w| w.id.clone()).collect();
        for item in &attention.waiting {
            if self.seen_waiting.insert(item.id.clone()) {
                keep(
                    waiting_notification(item, settings.show_text),
                    settings.waiting,
                    &mut out,
                );
            }
        }
        // An answered item never waits again, so it can go.
        self.seen_waiting.retain(|id| pending.contains(id));

        let mut statuses = BTreeMap::new();
        for task in &attention.tasks {
            let before = self.task_status.get(&task.id).copied();
            let moved = before != Some(task.status);
            if moved && task.status == TaskStatus::Review {
                keep(
                    task_notification(task, settings.show_text),
                    settings.review,
                    &mut out,
                );
            } else if moved && task.status == TaskStatus::Failed {
                keep(
                    task_notification(task, settings.show_text),
                    settings.failed,
                    &mut out,
                );
            }
            statuses.insert(task.id.clone(), task.status);
        }
        self.task_status = statuses;

        let mut until = self.messages_until;
        for message in &attention.messages {
            if message.created_at <= self.messages_until {
                continue;
            }
            until = until.max(message.created_at);
            let wanted = if message.routine {
                settings.routines
            } else {
                settings.messages
            };
            keep(
                message_notification(message, settings.show_text),
                wanted,
                &mut out,
            );
        }
        self.messages_until = until;
        out
    }
}

/// The count on the Dock icon: empty when the setting is off or nothing
/// waits.
pub fn badge_label(attention: &AttentionView, settings: &NotificationSettings) -> Option<String> {
    (settings.dock_badge && attention.count > 0).then(|| {
        if attention.count > 99 {
            "99+".to_owned()
        } else {
            attention.count.to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval(id: &str, task: Option<&str>) -> WaitingItem {
        WaitingItem {
            id: id.into(),
            decision: false,
            channel_id: "chn_sayso".into(),
            channel_name: "sayso".into(),
            task_id: task.map(Into::into),
            task_title: task.map(|_| "Go through open Sayso issues".into()),
            text: "Run gh issue list".into(),
        }
    }

    fn task(id: &str, status: TaskStatus) -> TaskState {
        TaskState {
            id: id.into(),
            channel_id: "chn_sayso".into(),
            channel_name: "sayso".into(),
            title: "Fix the build".into(),
            status,
            reason: Some("The tests failed.".into()),
            summary: Some("I fixed the **build**.".into()),
        }
    }

    fn message(id: &str, channel: &str, at: i64, routine: bool) -> IncomingMessage {
        IncomingMessage {
            id: id.into(),
            channel_id: channel.into(),
            channel_name: "house-lights".into(),
            routine,
            text: "The lights are on.".into(),
            created_at: at,
        }
    }

    fn loaded(
        waiting: Vec<WaitingItem>,
        tasks: Vec<TaskState>,
        messages: Vec<IncomingMessage>,
    ) -> AttentionView {
        AttentionView {
            loaded: true,
            count: 0,
            waiting,
            tasks,
            messages,
        }
    }

    fn away() -> Focus {
        Focus::default()
    }

    #[test]
    fn nothing_notifies_before_the_first_load_or_for_what_was_there() {
        let mut n = Notifier::new();
        let settings = NotificationSettings::default();
        let empty = AttentionView::default();
        assert!(n.observe(&empty, &away(), &settings).is_empty());
        let first = loaded(
            vec![approval("apr_1", Some("tsk_1"))],
            vec![task("tsk_2", TaskStatus::Review)],
            vec![message("msg_1", "chn_a", 10, false)],
        );
        assert!(n.observe(&first, &away(), &settings).is_empty());
        // The same state again gives nothing.
        assert!(n.observe(&first, &away(), &settings).is_empty());
    }

    #[test]
    fn each_event_notifies_once() {
        let mut n = Notifier::new();
        let settings = NotificationSettings::default();
        n.observe(
            &loaded(vec![], vec![task("tsk_1", TaskStatus::Working)], vec![]),
            &away(),
            &settings,
        );
        let next = loaded(
            vec![approval("apr_1", Some("tsk_1"))],
            vec![task("tsk_1", TaskStatus::NeedsInput)],
            vec![message("msg_1", "chn_a", 10, false)],
        );
        let shown = n.observe(&next, &away(), &settings);
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].tag, "consort:approval:apr_1");
        assert_eq!(shown[0].title, "sayso");
        assert_eq!(
            shown[0].body,
            "An approval waits in Go through open Sayso issues."
        );
        assert_eq!(
            shown[0].target,
            NotificationTarget::Task {
                channel_id: "chn_sayso".into(),
                task_id: "tsk_1".into()
            }
        );
        assert_eq!(shown[1].body, "The assistant posted a message.");
        assert!(n.observe(&next, &away(), &settings).is_empty());

        let review = loaded(vec![], vec![task("tsk_1", TaskStatus::Review)], vec![]);
        let shown = n.observe(&review, &away(), &settings);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].kind, NotificationKind::Review);
        assert_eq!(shown[0].body, "Fix the build is ready for review.");
        assert!(n.observe(&review, &away(), &settings).is_empty());

        let failed = loaded(vec![], vec![task("tsk_1", TaskStatus::Failed)], vec![]);
        let shown = n.observe(&failed, &away(), &settings);
        assert_eq!(shown[0].body, "Fix the build failed.");
    }

    #[test]
    fn the_channel_or_thread_on_screen_does_not_notify() {
        let mut n = Notifier::new();
        let settings = NotificationSettings::default();
        n.observe(&loaded(vec![], vec![], vec![]), &away(), &settings);
        let focus = Focus {
            channel_id: Some("chn_a".into()),
            task_id: Some("tsk_1".into()),
            active: true,
        };
        let next = loaded(
            vec![approval("apr_1", Some("tsk_1"))],
            vec![],
            vec![
                message("msg_1", "chn_a", 10, false),
                message("msg_2", "chn_b", 11, false),
            ],
        );
        let shown = n.observe(&next, &focus, &settings);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].tag, "consort:message:msg_2");
        // With the window in the background, the open channel notifies too.
        let mut n = Notifier::new();
        n.observe(&loaded(vec![], vec![], vec![]), &away(), &settings);
        let background = Focus {
            active: false,
            ..focus
        };
        assert_eq!(n.observe(&next, &background, &settings).len(), 3);
    }

    #[test]
    fn switches_turn_kinds_off_and_routines_are_off_by_default() {
        let mut n = Notifier::new();
        let settings = NotificationSettings {
            waiting: false,
            ..NotificationSettings::default()
        };
        n.observe(&loaded(vec![], vec![], vec![]), &away(), &settings);
        let next = loaded(
            vec![approval("apr_1", None)],
            vec![],
            vec![message("msg_1", "chn_a", 10, true)],
        );
        assert!(n.observe(&next, &away(), &settings).is_empty());
        let mut n = Notifier::new();
        let settings = NotificationSettings {
            routines: true,
            ..NotificationSettings::default()
        };
        n.observe(&loaded(vec![], vec![], vec![]), &away(), &settings);
        let shown = n.observe(&next, &away(), &settings);
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].body, "An approval waits for you.");
        assert_eq!(shown[1].kind, NotificationKind::Routine);
        assert_eq!(shown[1].body, "A routine posted a result.");
    }

    #[test]
    fn message_text_shows_only_when_it_is_on() {
        let item = approval("apr_1", Some("tsk_1"));
        assert_eq!(
            waiting_notification(&item, true).body,
            "Go through open Sayso issues: Run gh issue list"
        );
        let review = task("tsk_1", TaskStatus::Review);
        assert_eq!(
            task_notification(&review, true).body,
            "Fix the build is ready for review. I fixed the build."
        );
        let failed = task("tsk_1", TaskStatus::Failed);
        assert_eq!(
            task_notification(&failed, true).body,
            "Fix the build failed. The tests failed."
        );
        let msg = message("msg_1", "chn_a", 1, false);
        assert_eq!(message_notification(&msg, true).body, "The lights are on.");
        let long = IncomingMessage {
            text: "word ".repeat(100),
            ..msg
        };
        assert_eq!(
            message_notification(&long, true).body.chars().count(),
            BODY_MAX_CHARS
        );
    }

    #[test]
    fn an_old_message_that_loads_later_does_not_notify() {
        let mut n = Notifier::new();
        let settings = NotificationSettings::default();
        n.observe(
            &loaded(vec![], vec![], vec![message("msg_5", "chn_a", 50, false)]),
            &away(),
            &settings,
        );
        let older = loaded(
            vec![],
            vec![],
            vec![
                message("msg_1", "chn_b", 10, false),
                message("msg_5", "chn_a", 50, false),
            ],
        );
        assert!(n.observe(&older, &away(), &settings).is_empty());
    }

    #[test]
    fn the_badge_counts_what_waits_unless_it_is_off() {
        let mut attention = AttentionView {
            count: 3,
            ..AttentionView::default()
        };
        let mut settings = NotificationSettings::default();
        assert_eq!(badge_label(&attention, &settings).as_deref(), Some("3"));
        attention.count = 120;
        assert_eq!(badge_label(&attention, &settings).as_deref(), Some("99+"));
        settings.dock_badge = false;
        assert_eq!(badge_label(&attention, &settings), None);
        attention.count = 0;
        settings.dock_badge = true;
        assert_eq!(badge_label(&attention, &settings), None);
    }

    #[test]
    fn the_example_follows_the_text_switch() {
        let attention = AttentionView::default();
        let mut settings = NotificationSettings::default();
        let example = example_notification(&attention, &settings);
        assert_eq!(example.title, "sayso");
        assert_eq!(
            example.body,
            "An approval waits in Go through open Sayso issues."
        );
        assert_eq!(
            example_caption(&example, &settings),
            "With message text off. A click opens the task thread with the approval."
        );
        settings.show_text = true;
        let example = example_notification(&attention, &settings);
        assert!(example.body.contains("gh issue list"));
        assert!(example_caption(&example, &settings).starts_with("With message text on."));
    }

    #[test]
    fn settings_default_to_the_approved_switches_and_read_old_files() {
        let settings = NotificationSettings::default();
        assert!(settings.waiting && settings.review && settings.failed && settings.messages);
        assert!(!settings.routines);
        assert!(!settings.show_text);
        assert!(settings.dock_badge);
        let partial: NotificationSettings = serde_json::from_str(r#"{"routines": true}"#).unwrap();
        assert!(partial.routines && partial.waiting && !partial.show_text);
    }
}
