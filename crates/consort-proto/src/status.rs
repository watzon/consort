//! The task status machine from data-model.md.
//!
//! [`TaskStatus::check_transition`] says whether an [`Actor`] may move a task
//! from one status to another. [`TaskStatus::tab`] gives the Tasks panel tab,
//! and [`TaskStatus::your_turn`] gives the "your turn" group.
//!
//! The actors:
//!
//! - [`Actor::User`]: a client request, such as `task.resolve`.
//! - [`Actor::Coordinator`]: a coordinator's hub tool call, such as
//!   `message_task` or `retry_task`.
//! - [`Actor::TaskSession`]: the task's own hub tool call, such as
//!   `submit_for_review`.
//! - [`Actor::System`]: the hub itself, as it reacts to timers, placement
//!   and session events.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ErrorBody;
use crate::time::Timestamp;

/// Where a task is in its life.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// `start_at` is in the future.
    Scheduled,
    /// Waiting for placement or for a worker to come online.
    UpNext,
    /// A session is running a turn.
    Working,
    /// One or more approvals or decisions are pending.
    NeedsInput,
    /// Ready for review: the session finished its turn and waits for the user.
    Review,
    /// The session cannot continue. `status_reason` says why.
    Failed,
    /// The user marked the task done.
    Resolved,
}

/// Who asks for a status change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    /// The user, from a client.
    User,
    /// A coordinator, through a hub tool.
    Coordinator,
    /// The task's own session, through a hub tool.
    TaskSession,
    /// The hub, from timers, placement and session events.
    System,
}

/// The tab of the Tasks panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTab {
    /// Working, Needs input, Ready for review and Failed.
    Active,
    /// Up next.
    UpNext,
    /// Scheduled.
    Scheduled,
    /// Resolved.
    Resolved,
}

/// The groups at the top of the Active tab, for tasks that wait for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YourTurn {
    /// An approval or a decision waits.
    NeedsInput,
    /// The task waits for review.
    ReadyForReview,
}

/// A status change that the rules do not allow. Maps to a `conflict` error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{actor:?} cannot move a task from {from:?} to {to:?}")]
pub struct TransitionError {
    /// The current status.
    pub from: TaskStatus,
    /// The requested status.
    pub to: TaskStatus,
    /// Who asked.
    pub actor: Actor,
}

impl From<TransitionError> for ErrorBody {
    fn from(err: TransitionError) -> Self {
        ErrorBody::conflict(format!(
            "The task is {}, so it cannot move to {}.",
            err.from.label(),
            err.to.label()
        ))
    }
}

impl TaskStatus {
    /// Every status, in life order.
    pub const ALL: [TaskStatus; 7] = [
        Self::Scheduled,
        Self::UpNext,
        Self::Working,
        Self::NeedsInput,
        Self::Review,
        Self::Failed,
        Self::Resolved,
    ];

    /// The status of a new task: Scheduled when `start_at` is after `now`,
    /// otherwise Up next.
    pub fn initial(start_at: Option<Timestamp>, now: Timestamp) -> Self {
        match start_at {
            Some(at) if at > now => Self::Scheduled,
            _ => Self::UpNext,
        }
    }

    /// The actors that may cause the move from `self` to `to`. Empty when
    /// the move is not in the table.
    pub fn allowed_actors(self, to: TaskStatus) -> &'static [Actor] {
        use Actor::{Coordinator, System, TaskSession, User};
        use TaskStatus::{Failed, NeedsInput, Resolved, Review, Scheduled, UpNext, Working};
        match (self, to) {
            // `start_at` passed.
            (Scheduled, UpNext) => &[System],
            // Placement found a worker and the session started.
            (UpNext, Working) => &[System],
            // An approval or a decision opened.
            (Working, NeedsInput) => &[System],
            // The last pending approval or decision closed.
            (NeedsInput, Working) => &[System],
            // The turn ended with nothing pending, or the task called
            // `submit_for_review` and the turn ended.
            (Working, Review) => &[System, TaskSession],
            // The user or the coordinator sent the task a message.
            (Review, Working) => &[User, Coordinator],
            // Only the user resolves.
            (Review, Resolved) => &[User],
            // The user chose Not done.
            (Resolved, Review) => &[User],
            // The session was lost, the harness failed, or the fixed worker
            // was removed.
            (UpNext | Working | NeedsInput | Review, Failed) => &[System],
            // Retry. From Ready for review and Resolved, the hub allows it
            // only when the session of the task has ended.
            (Failed | Review | Resolved, UpNext) => &[User, Coordinator],
            _ => &[],
        }
    }

    /// Whether `actor` may move a task from `self` to `to`.
    pub fn can_transition(self, to: TaskStatus, actor: Actor) -> bool {
        self.allowed_actors(to).contains(&actor)
    }

    /// Like [`TaskStatus::can_transition`], with an error to return.
    pub fn check_transition(self, to: TaskStatus, actor: Actor) -> Result<(), TransitionError> {
        if self.can_transition(to, actor) {
            Ok(())
        } else {
            Err(TransitionError {
                from: self,
                to,
                actor,
            })
        }
    }

    /// The Tasks panel tab.
    pub fn tab(self) -> TaskTab {
        match self {
            Self::Scheduled => TaskTab::Scheduled,
            Self::UpNext => TaskTab::UpNext,
            Self::Working | Self::NeedsInput | Self::Review | Self::Failed => TaskTab::Active,
            Self::Resolved => TaskTab::Resolved,
        }
    }

    /// The "your turn" group, for tasks that wait for the user.
    pub fn your_turn(self) -> Option<YourTurn> {
        match self {
            Self::NeedsInput => Some(YourTurn::NeedsInput),
            Self::Review => Some(YourTurn::ReadyForReview),
            _ => None,
        }
    }

    /// Whether the hub wakes the channel's coordinator with a `task_update`
    /// when a task enters this status. An approval is the exception: it moves
    /// its task to Needs input and wakes nothing.
    pub fn wakes_coordinator(self) -> bool {
        matches!(self, Self::Review | Self::NeedsInput | Self::Failed)
    }

    /// Whether the hub sends a push notification when a task enters this
    /// status.
    pub fn sends_push(self) -> bool {
        matches!(self, Self::Review | Self::Failed)
    }

    /// The UI name in sentence case, for example "Ready for review".
    pub fn label(self) -> &'static str {
        match self {
            Self::Scheduled => "Scheduled",
            Self::UpNext => "Up next",
            Self::Working => "Working",
            Self::NeedsInput => "Needs input",
            Self::Review => "Ready for review",
            Self::Failed => "Failed",
            Self::Resolved => "Resolved",
        }
    }

    /// The wire string, for example `needs_input`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::UpNext => "up_next",
            Self::Working => "working",
            Self::NeedsInput => "needs_input",
            Self::Review => "review",
            Self::Failed => "failed",
            Self::Resolved => "resolved",
        }
    }
}

impl TaskTab {
    /// The tabs in panel order.
    pub const ALL: [TaskTab; 4] = [Self::Active, Self::UpNext, Self::Scheduled, Self::Resolved];

    /// The tab name in sentence case.
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::UpNext => "Up next",
            Self::Scheduled => "Scheduled",
            Self::Resolved => "Resolved",
        }
    }
}

impl YourTurn {
    /// The group name in sentence case.
    pub fn label(self) -> &'static str {
        match self {
            Self::NeedsInput => "Needs input",
            Self::ReadyForReview => "Ready for review",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Actor::{Coordinator, System, TaskSession, User};
    use TaskStatus::{Failed, NeedsInput, Resolved, Review, Scheduled, UpNext, Working};

    const ACTORS: [Actor; 4] = [User, Coordinator, TaskSession, System];

    /// The full table from data-model.md, as (from, to, actors).
    const TABLE: &[(TaskStatus, TaskStatus, &[Actor])] = &[
        (Scheduled, UpNext, &[System]),
        (UpNext, Working, &[System]),
        (Working, NeedsInput, &[System]),
        (NeedsInput, Working, &[System]),
        (Working, Review, &[System, TaskSession]),
        (Review, Working, &[User, Coordinator]),
        (Review, Resolved, &[User]),
        (Resolved, Review, &[User]),
        (UpNext, Failed, &[System]),
        (Working, Failed, &[System]),
        (NeedsInput, Failed, &[System]),
        (Review, Failed, &[System]),
        (Failed, UpNext, &[User, Coordinator]),
        // Retry after the session of the task ended. The hub checks that.
        (Review, UpNext, &[User, Coordinator]),
        (Resolved, UpNext, &[User, Coordinator]),
    ];

    #[test]
    fn every_pair_and_actor_matches_the_table() {
        for from in TaskStatus::ALL {
            for to in TaskStatus::ALL {
                let row = TABLE.iter().find(|(f, t, _)| *f == from && *t == to);
                for actor in ACTORS {
                    let expected = row.is_some_and(|(_, _, actors)| actors.contains(&actor));
                    assert_eq!(
                        from.can_transition(to, actor),
                        expected,
                        "{from:?} -> {to:?} by {actor:?}"
                    );
                    assert_eq!(from.check_transition(to, actor).is_ok(), expected);
                }
            }
        }
    }

    #[test]
    fn only_the_user_resolves() {
        for from in TaskStatus::ALL {
            for actor in [Coordinator, TaskSession, System] {
                assert!(!from.can_transition(Resolved, actor));
            }
        }
        assert!(Review.can_transition(Resolved, User));
        assert!(!UpNext.can_transition(Resolved, User));
        assert!(!Failed.can_transition(Resolved, User));
    }

    #[test]
    fn no_self_transitions() {
        for status in TaskStatus::ALL {
            for actor in ACTORS {
                assert!(!status.can_transition(status, actor));
            }
        }
    }

    #[test]
    fn resolving_up_next_is_a_conflict() {
        let err = UpNext.check_transition(Resolved, User).unwrap_err();
        let body: ErrorBody = err.into();
        assert_eq!(body.code, crate::error::ErrorCode::Conflict);
        assert_eq!(
            body.message,
            "The task is Up next, so it cannot move to Resolved."
        );
    }

    #[test]
    fn initial_status() {
        let now = Timestamp::from_millis(1_000);
        assert_eq!(TaskStatus::initial(None, now), UpNext);
        assert_eq!(
            TaskStatus::initial(Some(Timestamp::from_millis(2_000)), now),
            Scheduled
        );
        assert_eq!(TaskStatus::initial(Some(now), now), UpNext);
        assert_eq!(
            TaskStatus::initial(Some(Timestamp::from_millis(10)), now),
            UpNext
        );
    }

    #[test]
    fn tabs_and_your_turn() {
        let tabs: Vec<TaskTab> = TaskStatus::ALL.iter().map(|s| s.tab()).collect();
        assert_eq!(
            tabs,
            [
                TaskTab::Scheduled,
                TaskTab::UpNext,
                TaskTab::Active,
                TaskTab::Active,
                TaskTab::Active,
                TaskTab::Active,
                TaskTab::Resolved,
            ]
        );
        let groups: Vec<Option<YourTurn>> = TaskStatus::ALL.iter().map(|s| s.your_turn()).collect();
        assert_eq!(
            groups,
            [
                None,
                None,
                None,
                Some(YourTurn::NeedsInput),
                Some(YourTurn::ReadyForReview),
                None,
                None,
            ]
        );
        for status in TaskStatus::ALL {
            if status.your_turn().is_some() {
                assert_eq!(status.tab(), TaskTab::Active);
            }
        }
    }

    #[test]
    fn wake_and_push_rules() {
        let wakes: Vec<TaskStatus> = TaskStatus::ALL
            .into_iter()
            .filter(|s| s.wakes_coordinator())
            .collect();
        assert_eq!(wakes, [NeedsInput, Review, Failed]);
        let pushes: Vec<TaskStatus> = TaskStatus::ALL
            .into_iter()
            .filter(|s| s.sends_push())
            .collect();
        assert_eq!(pushes, [Review, Failed]);
    }

    #[test]
    fn wire_strings() {
        for status in TaskStatus::ALL {
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::json!(status.as_str())
            );
        }
    }
}
