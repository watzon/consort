//! The fallback path: the models that Consort tries, in order, when the model
//! of a channel cannot answer. See "The fallback path" in architecture.md.

use serde::{Deserialize, Serialize};

use super::common::{ModelSelection, ValidationError};
use crate::time::Timestamp;

/// The most steps that the user can add to the path.
pub const FALLBACK_STEPS_MAX: usize = 10;

/// The most characters in the account label or the model ID of a step.
pub const FALLBACK_FIELD_MAX_CHARS: usize = 200;

/// How many turns of one session must fail with a provider outage before the
/// path moves the work. The first failure and two retries.
pub const OUTAGE_TRIES: u32 = 3;

/// The one global fallback path. The first step is the model of the channel.
/// It is not stored: it changes with the channel. Next comes the built-in row
/// "Other accounts with the same model", then the steps of the user. The last
/// row, "Stop and show the error", is fixed and not stored either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FallbackPath {
    /// The built-in row "Other accounts with the same model". On by default.
    /// It moves the work to another signed-in account of the same harness
    /// that has the same model.
    pub other_accounts: bool,
    /// The steps that the user added, in order. An account label, a model
    /// and the options (the effort) of each.
    pub steps: Vec<ModelSelection>,
}

impl Default for FallbackPath {
    fn default() -> Self {
        Self {
            other_accounts: true,
            steps: Vec::new(),
        }
    }
}

impl FallbackPath {
    /// The place of a step with this account label and model, if the path
    /// has one. The options do not count: the same account and model twice
    /// is one step.
    pub fn position(&self, account_label: &str, model: &str) -> Option<usize> {
        self.steps
            .iter()
            .position(|s| s.account_label == account_label && s.model == model)
    }

    /// Checks the steps: at most [`FALLBACK_STEPS_MAX`], each with an account
    /// label and a model of reasonable length, and no account and model
    /// twice.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.steps.len() > FALLBACK_STEPS_MAX {
            return Err(ValidationError::new(
                "steps",
                format!("has more than {FALLBACK_STEPS_MAX} steps"),
            ));
        }
        for (index, step) in self.steps.iter().enumerate() {
            for (field, value) in [
                ("account_label", &step.account_label),
                ("model", &step.model),
            ] {
                if value.trim().is_empty() {
                    return Err(ValidationError::new(
                        format!("steps[{index}].{field}"),
                        "is empty",
                    ));
                }
                if value.chars().count() > FALLBACK_FIELD_MAX_CHARS {
                    return Err(ValidationError::new(
                        format!("steps[{index}].{field}"),
                        format!("is longer than {FALLBACK_FIELD_MAX_CHARS} characters"),
                    ));
                }
            }
            if self.position(&step.account_label, &step.model) != Some(index) {
                return Err(ValidationError::new(
                    format!("steps[{index}]"),
                    "repeats an account and model that an earlier step has",
                ));
            }
        }
        Ok(())
    }
}

/// What made the path move the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackCause {
    /// The account reached a usage limit.
    UsageLimit,
    /// The provider was down or overloaded for [`OUTAGE_TRIES`] turns.
    Outage,
}

/// Why the path skipped a step. A worker is never a reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// The account is at a usage limit, or its provider is down. The line
    /// has the cause and the time when the hold ends.
    Limited,
    /// The account is signed out, or its sign-in expired.
    SignedOut,
    /// No account has this label, or the account does not offer the model.
    Unavailable,
    /// The account cannot keep the approval mode. A step never lowers it.
    Mode,
}

/// Where a line of the path report belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathPlace {
    /// The first choice: the model of the channel or the task.
    First,
    /// The built-in row "Other accounts with the same model".
    OtherAccount,
    /// A step that the user added. The line has its number.
    Step,
}

/// What happened to one line of the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathOutcome {
    /// The turn failed on it. It is a filled dot.
    Failed,
    /// Consort did not try it, for the reason in the line. It is a ring.
    Skipped,
    /// The work moved to it.
    Chosen,
}

/// One line of the path report: what Consort tried or skipped, and why.
/// The details of a move and the error card of a turn that no step could
/// answer both show these lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathLine {
    /// Where the line belongs.
    pub place: PathPlace,
    /// The number of the step, from 1. Set for [`PathPlace::Step`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    /// The account label.
    pub account_label: String,
    /// The model ID.
    pub model: String,
    /// The name to show for the model, when the hub knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    /// The effort option, when the selection has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// What happened.
    pub outcome: PathOutcome,
    /// The cause of the hold, for a [`SkipReason::Limited`] skip or a failed
    /// line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<FallbackCause>,
    /// Why the line was skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<SkipReason>,
    /// When the hold on the account ends. Left out when the provider gave no
    /// time and the hub holds the account for a fixed time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Timestamp>,
    /// The name of the provider, for a failed line of an outage, such as
    /// "OpenAI".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// Whose work moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackScope {
    /// The assistant of a channel. It goes back on its own.
    Assistant,
    /// A task. It stays where it moved.
    Task,
}

/// A model with the words to show for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackModel {
    /// The account label.
    pub account_label: String,
    /// The model ID.
    pub model: String,
    /// The name to show for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
}

/// The extra fields of the notices `account_fallback` (the work moved),
/// `fallback_returned` (the assistant went back) and `fallback_signed_out`
/// (a move skipped an account that is signed out). The body of the notice
/// also has `kind` and `text`. The text is a full sentence for a client that
/// does not read the other fields. Every field is optional on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FallbackNotice {
    /// Whose work moved.
    pub scope: Option<FallbackScope>,
    /// What made the path move the work.
    pub cause: Option<FallbackCause>,
    /// The model that the work left. For `fallback_returned`, the model that
    /// the assistant left when it went back.
    pub from: Option<FallbackModel>,
    /// The model that the work moved to. For `fallback_returned`, the first
    /// choice that the assistant went back to.
    pub to: Option<FallbackModel>,
    /// The provider that now receives the work, set when the move goes to
    /// another harness than the first choice, such as "OpenAI".
    pub provider: Option<String>,
    /// When the assistant goes back. Left out when the provider gave no
    /// time.
    pub returns_at: Option<Timestamp>,
    /// One line for each step that Consort tried or skipped.
    pub lines: Vec<PathLine>,
    /// For `fallback_signed_out`: the label of the account that is signed
    /// out.
    pub account_label: Option<String>,
}

/// The state of an assistant that runs on a fallback now. The channel holds
/// it until the assistant goes back. The composer shows a marker on the model
/// chip while it is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorFallback {
    /// The first choice that the assistant returns to.
    pub from: ModelSelection,
    /// What the assistant runs on now.
    pub to: ModelSelection,
    /// What made the path move the assistant.
    pub cause: FallbackCause,
    /// When the assistant can go back. `None` when the provider gave no
    /// time: the hub then holds the account for a fixed time.
    pub returns_at: Option<Timestamp>,
    /// When the assistant moved.
    pub moved_at: Timestamp,
}

/// The company behind a harness, for the words "The work and its context go
/// to OpenAI." The names are the ones that the model picker shows for the
/// harness families that Consort knows. Another harness shows its own name.
pub fn harness_company(harness: &str) -> String {
    let lower = harness.to_ascii_lowercase();
    if lower.starts_with("claude") {
        "Anthropic".to_owned()
    } else if lower == "codex" {
        "OpenAI".to_owned()
    } else if lower.contains("gemini") {
        "Google".to_owned()
    } else if lower == "opencode" {
        "OpenCode".to_owned()
    } else if lower == "pi" {
        "Pi".to_owned()
    } else {
        lower.strip_prefix("acp:").unwrap_or(&lower).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(label: &str, model: &str) -> ModelSelection {
        ModelSelection {
            account_label: label.into(),
            model: model.into(),
            options: Default::default(),
        }
    }

    #[test]
    fn the_default_path_has_the_built_in_row_on_and_no_steps() {
        let path = FallbackPath::default();
        assert!(path.other_accounts);
        assert!(path.steps.is_empty());
        assert!(path.validate().is_ok());
        // An empty record reads as the default.
        let read: FallbackPath = serde_json::from_str("{}").unwrap();
        assert_eq!(read, path);
    }

    #[test]
    fn validate_refuses_an_empty_field_a_repeat_and_too_many_steps() {
        let mut path = FallbackPath {
            other_accounts: true,
            steps: vec![step("A", "m"), step("A", "n")],
        };
        assert!(path.validate().is_ok());
        path.steps.push(step("A", "m"));
        assert!(path.validate().is_err(), "a repeat");
        path.steps = vec![step(" ", "m")];
        assert!(path.validate().is_err(), "an empty label");
        path.steps = vec![step("A", "")];
        assert!(path.validate().is_err(), "an empty model");
        path.steps = (0..=FALLBACK_STEPS_MAX)
            .map(|i| step("A", &format!("m{i}")))
            .collect();
        assert!(path.validate().is_err(), "too many");
        path.steps = vec![step(&"x".repeat(FALLBACK_FIELD_MAX_CHARS + 1), "m")];
        assert!(path.validate().is_err(), "too long");
    }

    #[test]
    fn a_notice_reads_with_missing_fields() {
        let notice: FallbackNotice =
            serde_json::from_str(r#"{"kind":"account_fallback","text":"x"}"#).unwrap();
        assert_eq!(notice, FallbackNotice::default());
    }

    #[test]
    fn the_company_names_the_provider_of_a_harness() {
        assert_eq!(harness_company("claude"), "Anthropic");
        assert_eq!(harness_company("codex"), "OpenAI");
        assert_eq!(harness_company("acp:gemini"), "Google");
        assert_eq!(harness_company("acp:goose"), "goose");
    }
}
