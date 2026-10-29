//! Things that wait for the user: approvals and decisions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::common::ValidationError;
use crate::ids::{
    ApprovalId, ChannelId, ClientId, DecisionId, MessageId, RequestId, SessionId, TaskId,
};
use crate::time::Timestamp;

/// The kind of action that an approval asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    /// Run a command.
    Command,
    /// Change files.
    FileChange,
    /// Call a tool.
    Tool,
    /// Reach the network.
    Network,
    /// Anything else.
    Other,
}

/// What an approval asks for. Serialized with a `kind` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalDetails {
    /// Run a command.
    Command {
        /// The command line.
        command: String,
        /// The folder it runs in.
        cwd: Option<String>,
    },
    /// Change files.
    FileChange {
        /// The paths that change.
        #[serde(default)]
        paths: Vec<String>,
    },
    /// Call a tool.
    Tool {
        /// The MCP server name, when the tool comes from one.
        server: Option<String>,
        /// The tool name.
        tool: String,
        /// The arguments.
        #[serde(default)]
        arguments: Value,
    },
    /// Reach the network.
    Network {
        /// The host or URL.
        target: String,
    },
    /// Anything else.
    Other {
        /// What the harness says about the action.
        text: Option<String>,
    },
}

impl ApprovalDetails {
    /// The approval kind.
    pub fn kind(&self) -> ApprovalKind {
        match self {
            Self::Command { .. } => ApprovalKind::Command,
            Self::FileChange { .. } => ApprovalKind::FileChange,
            Self::Tool { .. } => ApprovalKind::Tool,
            Self::Network { .. } => ApprovalKind::Network,
            Self::Other { .. } => ApprovalKind::Other,
        }
    }
}

/// What choosing a request option means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOptionKind {
    /// Allow this action once.
    AllowOnce,
    /// Allow this action for the rest of the session.
    AllowAlways,
    /// Deny this action once.
    DenyOnce,
    /// Deny this action for the rest of the session.
    DenyAlways,
    /// One answer to a question.
    Choice,
}

impl RequestOptionKind {
    /// Whether choosing this option allows the action.
    pub fn is_allow(self) -> bool {
        matches!(self, Self::AllowOnce | Self::AllowAlways)
    }

    /// Whether choosing this option denies the action.
    pub fn is_deny(self) -> bool {
        matches!(self, Self::DenyOnce | Self::DenyAlways)
    }
}

/// One option of a harness request, as the harness gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestOption {
    /// The harness's option ID. `session.respond` sends it back.
    pub id: String,
    /// The button text.
    pub label: String,
    /// What the option means.
    pub kind: RequestOptionKind,
    /// More text about the option, when the harness gives it.
    pub description: Option<String>,
}

/// The status of an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Waiting for the user.
    Pending,
    /// The user allowed it.
    Allowed,
    /// The user denied it.
    Denied,
    /// Nobody answered in time.
    Expired,
    /// The harness withdrew it, for example after an interrupt.
    Canceled,
}

/// A harness request for permission to do one action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Approval {
    /// The ID.
    pub id: ApprovalId,
    /// The channel.
    pub channel_id: ChannelId,
    /// The session that asked.
    pub session_id: SessionId,
    /// The task, when a task session asked.
    pub task_id: Option<TaskId>,
    /// The approval card.
    pub message_id: MessageId,
    /// The harness request ID. `session.respond` sends it back.
    pub request_id: RequestId,
    /// One line for the card and the push notification.
    pub summary: String,
    /// What the approval asks for. Has a `kind` field.
    pub details: ApprovalDetails,
    /// The options from the harness.
    pub options: Vec<RequestOption>,
    /// The status.
    pub status: ApprovalStatus,
    /// The option that the user chose.
    pub answer_option_id: Option<String>,
    /// When the user answered.
    pub answered_at: Option<Timestamp>,
    /// The client that answered.
    pub answered_by_client_id: Option<ClientId>,
    /// When the approval opened.
    pub created_at: Timestamp,
}

impl Approval {
    /// The option with this ID.
    pub fn option(&self, option_id: &str) -> Option<&RequestOption> {
        self.options.iter().find(|o| o.id == option_id)
    }
}

/// The most characters in a decision question.
pub const DECISION_QUESTION_MAX_CHARS: usize = 1_200;
/// The most characters in a decision option label.
pub const DECISION_LABEL_MAX_CHARS: usize = 40;
/// The most characters in a decision option consequence.
pub const DECISION_CONSEQUENCE_MAX_CHARS: usize = 800;
/// The fewest options in a decision.
pub const DECISION_MIN_OPTIONS: usize = 2;
/// The most options in a decision.
pub const DECISION_MAX_OPTIONS: usize = 4;

/// One labelled option of a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionOption {
    /// The button text. At most 40 characters.
    pub label: String,
    /// What happens if the user picks this option. At most 800 characters.
    pub consequence: String,
    /// The harness's option ID, when the decision comes from a harness
    /// question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub id: Option<String>,
}

/// The status of a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    /// Waiting for the user.
    Pending,
    /// The user answered.
    Answered,
    /// Nobody answered in time.
    Expired,
    /// The assistant or the hub withdrew it.
    Canceled,
}

/// The user's answer to a decision: an option, free text, or both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionAnswer {
    /// The index of the chosen option.
    pub option_index: Option<u8>,
    /// Free text, when the decision allows it.
    pub text: Option<String>,
}

/// A question from the assistant with two to four labelled options.
///
/// A decision comes from the `ask_decision` hub tool, or from a harness
/// question (`request.opened` with kind `question`). For a harness question,
/// `request_id` is set and the hub sends the answer with `session.respond`.
/// Otherwise the answer arrives as input to a later turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// The ID.
    pub id: DecisionId,
    /// The channel.
    pub channel_id: ChannelId,
    /// The task, when a task session asked.
    pub task_id: Option<TaskId>,
    /// The session that asked.
    pub session_id: SessionId,
    /// The decision card.
    pub message_id: MessageId,
    /// The harness request, when the decision comes from a harness question.
    pub request_id: Option<RequestId>,
    /// The question. At most 1,200 characters.
    pub question: String,
    /// More context, in markdown.
    pub context: Option<String>,
    /// Two to four options.
    pub options: Vec<DecisionOption>,
    /// The option that the assistant recommends.
    pub recommended_index: Option<u8>,
    /// Whether the user may answer with free text.
    pub allow_free_text: bool,
    /// The status.
    pub status: DecisionStatus,
    /// The answer.
    pub answer: Option<DecisionAnswer>,
    /// When the user answered.
    pub answered_at: Option<Timestamp>,
    /// When the decision opened.
    pub created_at: Timestamp,
}

/// Checks the limits of a decision: the question length, two to four
/// options, the label and consequence lengths, and the recommended index.
pub fn validate_decision(
    question: &str,
    options: &[DecisionOption],
    recommended_index: Option<u8>,
) -> Result<(), ValidationError> {
    if question.trim().is_empty() {
        return Err(ValidationError::new("question", "is empty"));
    }
    if question.chars().count() > DECISION_QUESTION_MAX_CHARS {
        return Err(ValidationError::new(
            "question",
            format!("is longer than {DECISION_QUESTION_MAX_CHARS} characters"),
        ));
    }
    if !(DECISION_MIN_OPTIONS..=DECISION_MAX_OPTIONS).contains(&options.len()) {
        return Err(ValidationError::new(
            "options",
            format!(
                "must have {DECISION_MIN_OPTIONS} to {DECISION_MAX_OPTIONS} items, not {}",
                options.len()
            ),
        ));
    }
    for (i, option) in options.iter().enumerate() {
        if option.label.trim().is_empty() {
            return Err(ValidationError::new(
                format!("options[{i}].label"),
                "is empty",
            ));
        }
        if option.label.chars().count() > DECISION_LABEL_MAX_CHARS {
            return Err(ValidationError::new(
                format!("options[{i}].label"),
                format!("is longer than {DECISION_LABEL_MAX_CHARS} characters"),
            ));
        }
        if option.consequence.chars().count() > DECISION_CONSEQUENCE_MAX_CHARS {
            return Err(ValidationError::new(
                format!("options[{i}].consequence"),
                format!("is longer than {DECISION_CONSEQUENCE_MAX_CHARS} characters"),
            ));
        }
    }
    if let Some(index) = recommended_index
        && usize::from(index) >= options.len()
    {
        return Err(ValidationError::new(
            "recommended_index",
            "does not point at an option",
        ));
    }
    Ok(())
}

impl Decision {
    /// Checks the decision's own limits.
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_decision(&self.question, &self.options, self.recommended_index)
    }

    /// Checks that an answer fits this decision: it names an existing option
    /// or gives text, and gives text only when free text is allowed.
    pub fn check_answer(&self, answer: &DecisionAnswer) -> Result<(), ValidationError> {
        let text = answer
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        if answer.option_index.is_none() && text.is_none() {
            return Err(ValidationError::new(
                "option_index",
                "is missing, and no text was given",
            ));
        }
        if let Some(index) = answer.option_index
            && usize::from(index) >= self.options.len()
        {
            return Err(ValidationError::new(
                "option_index",
                "does not point at an option",
            ));
        }
        if text.is_some() && !self.allow_free_text {
            return Err(ValidationError::new(
                "text",
                "is not allowed for this decision",
            ));
        }
        Ok(())
    }

    /// The label of the chosen option, if any.
    pub fn answer_label(&self) -> Option<&str> {
        let index = self.answer.as_ref()?.option_index?;
        self.options
            .get(usize::from(index))
            .map(|o| o.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn option(label: &str) -> DecisionOption {
        DecisionOption {
            label: label.into(),
            consequence: "Something happens.".into(),
            id: None,
        }
    }

    #[test]
    fn decision_limits() {
        let two = [option("Yes"), option("No")];
        assert!(validate_decision("Ship it?", &two, Some(0)).is_ok());
        assert!(validate_decision("Ship it?", &two[..1], None).is_err());
        let five = [
            option("a"),
            option("b"),
            option("c"),
            option("d"),
            option("e"),
        ];
        assert!(validate_decision("Ship it?", &five, None).is_err());
        assert!(validate_decision("Ship it?", &five[..4], None).is_ok());
        assert!(validate_decision("", &two, None).is_err());
        assert!(validate_decision(&"q".repeat(1_201), &two, None).is_err());
        assert!(validate_decision(&"q".repeat(1_200), &two, None).is_ok());
        let long_label = [option(&"x".repeat(41)), option("No")];
        let err = validate_decision("Ship?", &long_label, None).unwrap_err();
        assert_eq!(err.field, "options[0].label");
        let ok_label = [option(&"é".repeat(40)), option("No")];
        assert!(validate_decision("Ship?", &ok_label, None).is_ok());
        let mut long_consequence = option("Yes");
        long_consequence.consequence = "c".repeat(801);
        assert!(validate_decision("Ship?", &[long_consequence, option("No")], None).is_err());
        assert!(validate_decision("Ship?", &two, Some(2)).is_err());
    }

    fn decision(allow_free_text: bool) -> Decision {
        Decision {
            id: DecisionId::new(),
            channel_id: ChannelId::new(),
            task_id: None,
            session_id: SessionId::new(),
            message_id: MessageId::new(),
            request_id: None,
            question: "Ship it?".into(),
            context: None,
            options: vec![option("Yes"), option("Not now")],
            recommended_index: None,
            allow_free_text,
            status: DecisionStatus::Pending,
            answer: None,
            answered_at: None,
            created_at: Timestamp::from_millis(0),
        }
    }

    #[test]
    fn decision_answers() {
        let d = decision(false);
        let pick = DecisionAnswer {
            option_index: Some(1),
            text: None,
        };
        assert!(d.check_answer(&pick).is_ok());
        assert!(
            d.check_answer(&DecisionAnswer {
                option_index: Some(2),
                text: None
            })
            .is_err()
        );
        assert!(
            d.check_answer(&DecisionAnswer {
                option_index: None,
                text: Some("maybe".into())
            })
            .is_err()
        );
        assert!(
            d.check_answer(&DecisionAnswer {
                option_index: None,
                text: Some("  ".into())
            })
            .is_err()
        );
        let free = decision(true);
        assert!(
            free.check_answer(&DecisionAnswer {
                option_index: None,
                text: Some("maybe".into())
            })
            .is_ok()
        );
        let mut answered = free.clone();
        answered.answer = Some(pick);
        assert_eq!(answered.answer_label(), Some("Not now"));
    }

    #[test]
    fn approval_details_have_kind() {
        let details = ApprovalDetails::Command {
            command: "rm -rf target".into(),
            cwd: Some("/w".into()),
        };
        assert_eq!(
            serde_json::to_value(&details).unwrap(),
            json!({"kind": "command", "command": "rm -rf target", "cwd": "/w"})
        );
        assert_eq!(details.kind(), ApprovalKind::Command);
        let other: ApprovalDetails = serde_json::from_value(json!({"kind": "other"})).unwrap();
        assert_eq!(other, ApprovalDetails::Other { text: None });
    }

    #[test]
    fn option_kinds() {
        assert!(RequestOptionKind::AllowAlways.is_allow());
        assert!(RequestOptionKind::DenyOnce.is_deny());
        assert!(!RequestOptionKind::Choice.is_allow());
        assert!(!RequestOptionKind::Choice.is_deny());
    }
}
