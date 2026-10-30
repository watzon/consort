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

/// The result of a run.
///
/// A run is `running` until its work ends: the coordinator turn that got the
/// routine's prompt, or the task that the run started (until the task is
/// Ready for review, Resolved or Failed). A run that the hub did not start is
/// `skipped`: the last run of the same routine still ran, or the hub was
/// offline at the run time for 1 hour or more.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineRunStatus {
    /// The run worked.
    #[default]
    Ok,
    /// The run failed.
    Error,
    /// The hub did not start the run. `error` says why.
    Skipped,
    /// The run started and its work did not end yet.
    Running,
}

impl RoutineRunStatus {
    /// The text that the wire uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Skipped => "skipped",
            Self::Running => "running",
        }
    }

    /// Reads the wire text. Unknown text is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "ok" => Self::Ok,
            "error" => Self::Error,
            "skipped" => Self::Skipped,
            "running" => Self::Running,
            _ => return None,
        })
    }
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

/// One run of a routine. It is a feed entity, so a client sees a run start
/// and end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineRun {
    /// The run number. It is unique in the hub and grows with each run.
    #[serde(default)]
    pub id: u64,
    /// The routine.
    pub routine_id: RoutineId,
    /// The channel of the routine when the run started.
    pub channel_id: ChannelId,
    /// When the run started. For a skipped run, the time when it was due.
    pub started_at: Timestamp,
    /// When the work of the run ended. Empty while it runs.
    #[serde(default)]
    pub ended_at: Option<Timestamp>,
    /// The result.
    #[serde(default)]
    pub status: RoutineRunStatus,
    /// True for a run that the user started with Run now.
    #[serde(default)]
    pub manual: bool,
    /// The first message that the assistant posted in the timeline in the
    /// run's turn, for a coordinator turn.
    pub message_id: Option<MessageId>,
    /// The task that the run created, for a `start_task` action.
    pub task_id: Option<TaskId>,
    /// Why the run failed or was skipped, with the next step.
    pub error: Option<String>,
}

impl RoutineRun {
    /// The feed `entity_id` of this run: its number.
    pub fn entity_id(&self) -> String {
        self.id.to_string()
    }

    /// Whether the work of the run has not ended yet.
    pub fn is_running(&self) -> bool {
        self.status == RoutineRunStatus::Running
    }
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
    fn run_shapes() {
        let routine: RoutineId = "rtn_00000000-0000-0000-0000-000000000005".parse().unwrap();
        let channel: ChannelId = "chn_00000000-0000-0000-0000-000000000001".parse().unwrap();
        let run = RoutineRun {
            id: 7,
            routine_id: routine,
            channel_id: channel,
            started_at: Timestamp::from_millis(1_000),
            ended_at: None,
            status: RoutineRunStatus::Skipped,
            manual: true,
            message_id: None,
            task_id: None,
            error: Some("The last run was still going.".into()),
        };
        let value = serde_json::to_value(&run).unwrap();
        assert_eq!(value["status"], "skipped");
        assert_eq!(value["manual"], true);
        assert_eq!(serde_json::from_value::<RoutineRun>(value).unwrap(), run);
        // A run from an older hub has no status, number or end.
        let old: RoutineRun = serde_json::from_value(json!({
            "routine_id": routine, "channel_id": channel, "started_at": "2026-11-01T08:00:00Z",
            "message_id": null, "task_id": null, "error": null
        }))
        .unwrap();
        assert_eq!(
            (old.id, old.status, old.manual),
            (0, RoutineRunStatus::Ok, false)
        );
        for status in [
            RoutineRunStatus::Ok,
            RoutineRunStatus::Error,
            RoutineRunStatus::Skipped,
            RoutineRunStatus::Running,
        ] {
            assert_eq!(RoutineRunStatus::parse(status.as_str()), Some(status));
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                json!(status.as_str())
            );
        }
        assert_eq!(RoutineRunStatus::parse("done"), None);
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
