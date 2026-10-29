//! Routines: prompts that the hub runs on a schedule in one channel.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::{ModelSelection, ValidationError};
use super::work::PlacementRequest;
use crate::ids::{ChannelId, MessageId, RoutineId, TaskId};
use crate::time::Timestamp;

/// The shortest interval for an `interval` schedule, in seconds.
pub const ROUTINE_MIN_INTERVAL_SECONDS: u64 = 60;

/// When a routine runs. Serialized with a `kind` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoutineSchedule {
    /// A cron expression in a time zone.
    Cron {
        /// A five-field cron expression, for example `0 8 * * 1-5`.
        expression: String,
        /// An IANA time zone, for example `America/Chicago`.
        time_zone: String,
    },
    /// A fixed interval.
    Interval {
        /// Seconds between runs. At least 60.
        seconds: u64,
    },
    /// One run at a time.
    Once {
        /// When to run.
        at: Timestamp,
    },
}

impl RoutineSchedule {
    /// Checks the schedule. The hub checks the cron syntax and the time zone
    /// itself.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Cron {
                expression,
                time_zone,
            } => {
                if expression.split_whitespace().count() != 5 {
                    return Err(ValidationError::new(
                        "schedule.expression",
                        "must have five fields",
                    ));
                }
                if time_zone.trim().is_empty() {
                    return Err(ValidationError::new("schedule.time_zone", "is empty"));
                }
                Ok(())
            }
            Self::Interval { seconds } if *seconds < ROUTINE_MIN_INTERVAL_SECONDS => {
                Err(ValidationError::new(
                    "schedule.seconds",
                    format!("must be at least {ROUTINE_MIN_INTERVAL_SECONDS}"),
                ))
            }
            Self::Interval { .. } | Self::Once { .. } => Ok(()),
        }
    }
}

/// What a routine does when it runs. Serialized with a `kind` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoutineAction {
    /// Wake the channel's coordinator with the prompt.
    CoordinatorTurn,
    /// Start a task with the prompt as its brief.
    StartTask {
        /// The task title.
        title: String,
        /// The model selection. Empty means the channel's default.
        #[serde(default)]
        model_selection: Option<ModelSelection>,
        /// Where the task may run.
        #[serde(default)]
        placement: PlacementRequest,
    },
}

/// The result of the last run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineRunStatus {
    /// The run worked.
    Ok,
    /// The run failed.
    Error,
}

/// A prompt that the hub runs on a schedule in one channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routine {
    /// The ID.
    pub id: RoutineId,
    /// The channel.
    pub channel_id: ChannelId,
    /// The name, for example "Morning brief".
    pub name: String,
    /// When it runs.
    pub schedule: RoutineSchedule,
    /// The prompt, in markdown.
    pub prompt: String,
    /// What it does.
    pub action: RoutineAction,
    /// Whether it runs.
    pub enabled: bool,
    /// The next run.
    pub next_run_at: Option<Timestamp>,
    /// The last run.
    pub last_run_at: Option<Timestamp>,
    /// The result of the last run.
    pub last_run_status: Option<RoutineRunStatus>,
}

/// One run of a routine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineRun {
    /// The routine.
    pub routine_id: RoutineId,
    /// When the run started.
    pub started_at: Timestamp,
    /// The message that the run caused, for a coordinator turn.
    pub message_id: Option<MessageId>,
    /// The task that the run created, for a `start_task` action.
    pub task_id: Option<TaskId>,
    /// The error, when the run failed.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schedule_shapes() {
        let cron = RoutineSchedule::Cron {
            expression: "0 8 * * 1-5".into(),
            time_zone: "America/Chicago".into(),
        };
        assert_eq!(
            serde_json::to_value(&cron).unwrap(),
            json!({"kind": "cron", "expression": "0 8 * * 1-5", "time_zone": "America/Chicago"})
        );
        let once: RoutineSchedule =
            serde_json::from_value(json!({"kind": "once", "at": "2026-11-01T08:00:00Z"})).unwrap();
        assert!(matches!(once, RoutineSchedule::Once { .. }));
        assert!(cron.validate().is_ok());
    }

    #[test]
    fn schedule_validation() {
        assert!(
            RoutineSchedule::Interval { seconds: 59 }
                .validate()
                .is_err()
        );
        assert!(RoutineSchedule::Interval { seconds: 60 }.validate().is_ok());
        assert!(
            RoutineSchedule::Cron {
                expression: "* * *".into(),
                time_zone: "UTC".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            RoutineSchedule::Cron {
                expression: "* * * * *".into(),
                time_zone: " ".into()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn action_shapes() {
        assert_eq!(
            serde_json::to_value(RoutineAction::CoordinatorTurn).unwrap(),
            json!({"kind": "coordinator_turn"})
        );
        let start: RoutineAction =
            serde_json::from_value(json!({"kind": "start_task", "title": "Nightly"})).unwrap();
        assert_eq!(
            start,
            RoutineAction::StartTask {
                title: "Nightly".into(),
                model_selection: None,
                placement: PlacementRequest::default(),
            }
        );
    }
}
