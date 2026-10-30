//! What the assistant of a channel does now: the state of the Three voices
//! mark and the short line in the activity pill under it.
//!
//! The client has no "the assistant is busy" event. It reads the work from
//! the events it already has:
//!
//! - **Task status.** A task in Working means its session runs. Tutti shows
//!   the tasks of every channel, because its assistant can read all of them.
//!   Another channel shows its own tasks.
//! - **Session items** (`session_item` feed entries). The newest item of
//!   each session, with its kind, its status and the channel of its feed
//!   entry, is kept in the cache ([`SessionActivity`]). An item in progress
//!   gives the line, such as "Running a command in sayso". A session that is
//!   no task's current session is a coordinator session.
//! - **The newest message.** When the newest message of the channel is one
//!   that the user sent, the coordinator has not answered yet, so it works.
//! - **Your turn.** A task that needs input or waits for review, or an
//!   approval or a decision in the timeline that waits, makes the mark show
//!   Needs you when nothing works.

use consort_proto::model::{ItemPayload, ItemStatus, SessionItem};

use crate::view::{RequestState, Speaker, TaskStatus, TimelineItem, TimelineView};

/// The state of the Three voices mark. Done is not here: the app shows it
/// for a moment when Working ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MarkState {
    #[default]
    Idle,
    Working,
    NeedsYou,
}

/// The mark state and the activity line of one channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivityView {
    pub state: MarkState,
    /// What the assistant does, such as "Searching the web". Only for
    /// Working. "Working" when no detail is known.
    pub line: Option<String>,
}

/// The activity of one channel, in [`crate::view::ViewState::activity`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelActivity {
    pub channel_id: String,
    pub activity: ActivityView,
}

/// What kind of work a session item is, for the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkKind {
    Command,
    FileChange,
    WebSearch,
    ToolCall,
    Subagent,
    Reasoning,
    Writing,
    Plan,
    Error,
}

/// The newest item of one session, as the cache keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionActivity {
    pub session_id: String,
    /// The channel of the feed entry that brought the item.
    pub channel_id: Option<String>,
    pub kind: WorkKind,
    pub in_progress: bool,
    /// When the item last changed, in Unix milliseconds.
    pub updated_at: i64,
}

impl SessionActivity {
    pub fn of(item: &SessionItem, channel_id: Option<String>) -> SessionActivity {
        let kind = match &item.payload {
            ItemPayload::AssistantMessage(_) => WorkKind::Writing,
            ItemPayload::Reasoning(_) => WorkKind::Reasoning,
            ItemPayload::Command(_) => WorkKind::Command,
            ItemPayload::FileChange(_) => WorkKind::FileChange,
            ItemPayload::ToolCall(_) => WorkKind::ToolCall,
            ItemPayload::Plan(_) => WorkKind::Plan,
            ItemPayload::Subagent(_) => WorkKind::Subagent,
            ItemPayload::WebSearch(_) => WorkKind::WebSearch,
            ItemPayload::Error(_) => WorkKind::Error,
        };
        SessionActivity {
            session_id: item.session_id.to_string(),
            channel_id,
            kind,
            in_progress: item.status == ItemStatus::InProgress,
            updated_at: item.updated_at.as_millis(),
        }
    }
}

/// The line for one kind of work. `place` is " in sayso" when the work runs
/// in another channel than the one on screen, else empty.
pub fn work_line(kind: WorkKind, place: &str) -> String {
    match kind {
        WorkKind::Command => format!("Running a command{place}"),
        WorkKind::FileChange => format!("Changing files{place}"),
        WorkKind::WebSearch => "Searching the web".to_owned(),
        WorkKind::ToolCall => format!("Using a tool{place}"),
        WorkKind::Subagent => format!("Working on a part of the task{place}"),
        WorkKind::Reasoning => "Thinking".to_owned(),
        WorkKind::Writing => "Writing".to_owned(),
        WorkKind::Plan => "Making a plan".to_owned(),
        WorkKind::Error => "Working".to_owned(),
    }
}

/// A task as the activity reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskWork<'a> {
    pub channel_id: &'a str,
    pub channel_name: &'a str,
    pub status: TaskStatus,
    pub session_id: Option<&'a str>,
}

/// The inputs for the activity of one channel.
pub struct Inputs<'a> {
    pub channel_id: &'a str,
    /// Tutti reads every channel, so every task counts.
    pub tutti: bool,
    pub tasks: &'a [TaskWork<'a>],
    /// The newest item of each session.
    pub sessions: &'a [SessionActivity],
    pub timeline: Option<&'a TimelineView>,
    /// The name of a channel, by ID, for the line.
    pub channel_name: &'a dyn Fn(&str) -> Option<String>,
}

/// The newest message of the timeline is one that the user sent, and it
/// reached the hub: the coordinator has not answered it yet.
fn waits_for_reply(timeline: Option<&TimelineView>) -> bool {
    let Some(timeline) = timeline else {
        return false;
    };
    timeline
        .items
        .iter()
        .rev()
        .find(|item| !matches!(item, TimelineItem::Day(_)))
        .is_some_and(|item| {
            matches!(item, TimelineItem::Message(m) if m.speaker == Speaker::You && !m.pending)
        })
}

/// An approval or a decision in the timeline waits for the user.
fn timeline_waits_for_you(timeline: Option<&TimelineView>) -> bool {
    timeline.is_some_and(|t| {
        t.items.iter().any(|item| match item {
            TimelineItem::Approval(a) => a.approval.state == RequestState::Pending,
            TimelineItem::Decision(d) => d.decision.state == RequestState::Pending,
            _ => false,
        })
    })
}

/// The activity of one channel. See the module docs for the rules.
pub fn channel_activity(inputs: &Inputs<'_>) -> ActivityView {
    let relevant: Vec<&TaskWork<'_>> = inputs
        .tasks
        .iter()
        .filter(|t| inputs.tutti || t.channel_id == inputs.channel_id)
        .collect();
    let task_sessions: Vec<&str> = inputs.tasks.iter().filter_map(|t| t.session_id).collect();
    // The sessions whose items speak for this channel's work: the sessions
    // of its working tasks, and its coordinator sessions.
    let working_tasks: Vec<&&TaskWork<'_>> = relevant
        .iter()
        .filter(|t| t.status == TaskStatus::Working)
        .collect();
    let coordinator_in_progress = inputs.sessions.iter().any(|s| {
        s.in_progress
            && !task_sessions.contains(&s.session_id.as_str())
            && s.channel_id.as_deref() == Some(inputs.channel_id)
    });
    let working =
        !working_tasks.is_empty() || coordinator_in_progress || waits_for_reply(inputs.timeline);
    if working {
        // The newest item in progress gives the line.
        let newest = inputs
            .sessions
            .iter()
            .filter(|s| s.in_progress)
            .filter_map(|s| {
                let task = working_tasks
                    .iter()
                    .find(|t| t.session_id == Some(s.session_id.as_str()));
                match task {
                    Some(task) => Some((s, task.channel_id)),
                    None if !task_sessions.contains(&s.session_id.as_str())
                        && s.channel_id.as_deref() == Some(inputs.channel_id) =>
                    {
                        Some((s, inputs.channel_id))
                    }
                    None => None,
                }
            })
            .max_by_key(|(s, _)| s.updated_at);
        let line = match newest {
            Some((session, channel)) => {
                let place = if channel == inputs.channel_id {
                    String::new()
                } else {
                    (inputs.channel_name)(channel)
                        .or_else(|| {
                            working_tasks
                                .iter()
                                .find(|t| t.channel_id == channel)
                                .map(|t| t.channel_name.to_owned())
                        })
                        .map(|name| format!(" in {name}"))
                        .unwrap_or_default()
                };
                work_line(session.kind, &place)
            }
            None => "Working".to_owned(),
        };
        return ActivityView {
            state: MarkState::Working,
            line: Some(line),
        };
    }
    let your_turn =
        relevant.iter().any(|t| t.status.is_your_turn()) || timeline_waits_for_you(inputs.timeline);
    ActivityView {
        state: if your_turn {
            MarkState::NeedsYou
        } else {
            MarkState::Idle
        },
        line: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{ApprovalCardView, ApprovalView, MessageView};

    fn session(
        id: &str,
        channel: &str,
        kind: WorkKind,
        in_progress: bool,
        at: i64,
    ) -> SessionActivity {
        SessionActivity {
            session_id: id.into(),
            channel_id: Some(channel.into()),
            kind,
            in_progress,
            updated_at: at,
        }
    }

    fn task<'a>(
        channel: &'a str,
        name: &'a str,
        status: TaskStatus,
        session: Option<&'a str>,
    ) -> TaskWork<'a> {
        TaskWork {
            channel_id: channel,
            channel_name: name,
            status,
            session_id: session,
        }
    }

    fn names(id: &str) -> Option<String> {
        match id {
            "c_sayso" => Some("sayso".into()),
            "c_home" => Some("homelab".into()),
            _ => None,
        }
    }

    fn run(
        channel: &str,
        tutti: bool,
        tasks: &[TaskWork<'_>],
        sessions: &[SessionActivity],
        timeline: Option<&TimelineView>,
    ) -> ActivityView {
        channel_activity(&Inputs {
            channel_id: channel,
            tutti,
            tasks,
            sessions,
            timeline,
            channel_name: &names,
        })
    }

    #[test]
    fn a_working_task_with_a_command_in_progress_names_its_channel_in_tutti() {
        let tasks = [task("c_sayso", "sayso", TaskStatus::Working, Some("s1"))];
        let sessions = [session("s1", "c_sayso", WorkKind::Command, true, 5)];
        let a = run("c_tutti", true, &tasks, &sessions, None);
        assert_eq!(a.state, MarkState::Working);
        assert_eq!(a.line.as_deref(), Some("Running a command in sayso"));
        // In its own channel the line leaves out the channel.
        let a = run("c_sayso", false, &tasks, &sessions, None);
        assert_eq!(a.line.as_deref(), Some("Running a command"));
    }

    #[test]
    fn the_newest_item_in_progress_wins() {
        let tasks = [
            task("c_sayso", "sayso", TaskStatus::Working, Some("s1")),
            task("c_home", "homelab", TaskStatus::Working, Some("s2")),
        ];
        let sessions = [
            session("s1", "c_sayso", WorkKind::Command, true, 5),
            session("s2", "c_home", WorkKind::WebSearch, true, 9),
        ];
        let a = run("c_tutti", true, &tasks, &sessions, None);
        assert_eq!(a.line.as_deref(), Some("Searching the web"));
    }

    #[test]
    fn no_detail_says_working() {
        let tasks = [task("c_sayso", "sayso", TaskStatus::Working, Some("s1"))];
        let done = [session("s1", "c_sayso", WorkKind::Command, false, 5)];
        let a = run("c_sayso", false, &tasks, &done, None);
        assert_eq!(a.state, MarkState::Working);
        assert_eq!(a.line.as_deref(), Some("Working"));
    }

    #[test]
    fn another_channels_task_does_not_make_a_channel_work() {
        let tasks = [task("c_home", "homelab", TaskStatus::Working, Some("s2"))];
        let a = run("c_sayso", false, &tasks, &[], None);
        assert_eq!(a, ActivityView::default());
    }

    #[test]
    fn a_coordinator_session_in_progress_works_and_gives_its_line() {
        // s9 is no task's session, so it is the channel's coordinator.
        let sessions = [session("s9", "c_sayso", WorkKind::FileChange, true, 3)];
        let a = run("c_sayso", false, &[], &sessions, None);
        assert_eq!(a.state, MarkState::Working);
        assert_eq!(a.line.as_deref(), Some("Changing files"));
        // Tutti does not take another channel's coordinator.
        assert_eq!(
            run("c_tutti", true, &[], &sessions, None).state,
            MarkState::Idle
        );
    }

    #[test]
    fn a_message_that_waits_for_a_reply_means_the_assistant_works() {
        let mut timeline = TimelineView {
            channel_id: "c_sayso".into(),
            items: vec![TimelineItem::Message(MessageView {
                speaker: Speaker::You,
                ..MessageView::default()
            })],
            ..TimelineView::default()
        };
        let a = run("c_sayso", false, &[], &[], Some(&timeline));
        assert_eq!(a.state, MarkState::Working);
        assert_eq!(a.line.as_deref(), Some("Working"));
        // A message still on its way does not count.
        if let TimelineItem::Message(m) = &mut timeline.items[0] {
            m.pending = true;
        }
        assert_eq!(
            run("c_sayso", false, &[], &[], Some(&timeline)).state,
            MarkState::Idle
        );
    }

    #[test]
    fn your_turn_shows_needs_you_when_nothing_works() {
        let tasks = [task("c_sayso", "sayso", TaskStatus::NeedsInput, None)];
        let a = run("c_sayso", false, &tasks, &[], None);
        assert_eq!(
            a,
            ActivityView {
                state: MarkState::NeedsYou,
                line: None
            }
        );
        // Work comes first.
        let tasks = [
            task("c_sayso", "sayso", TaskStatus::NeedsInput, None),
            task("c_sayso", "sayso", TaskStatus::Working, None),
        ];
        assert_eq!(
            run("c_sayso", false, &tasks, &[], None).state,
            MarkState::Working
        );
    }

    #[test]
    fn a_pending_approval_in_the_timeline_needs_you() {
        let timeline = TimelineView {
            items: vec![TimelineItem::Approval(ApprovalCardView {
                approval: ApprovalView {
                    state: RequestState::Pending,
                    ..ApprovalView::default()
                },
                ..ApprovalCardView::default()
            })],
            ..TimelineView::default()
        };
        assert_eq!(
            run("c_sayso", false, &[], &[], Some(&timeline)).state,
            MarkState::NeedsYou
        );
    }

    #[test]
    fn every_kind_has_a_short_line() {
        for kind in [
            WorkKind::Command,
            WorkKind::FileChange,
            WorkKind::WebSearch,
            WorkKind::ToolCall,
            WorkKind::Subagent,
            WorkKind::Reasoning,
            WorkKind::Writing,
            WorkKind::Plan,
            WorkKind::Error,
        ] {
            let line = work_line(kind, " in sayso");
            assert!(!line.is_empty() && line.len() <= 40, "{kind:?}: {line}");
            assert!(line.chars().next().is_some_and(char::is_uppercase));
        }
    }
}
