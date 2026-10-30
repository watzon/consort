//! Things that wait for the user: approvals and decisions.
//!
//! An approval comes from a harness (`request.opened`), or the hub makes it
//! itself as a **hub card**: a connector approval, a management approval
//! card, a connect card, a sign-in card or an install card. A hub card uses
//! the same entity, the same card message and the same `approval.answer`
//! path. Its `request_id` starts with [`HUB_CARD_REQUEST_PREFIX`], and the
//! hub never sends `session.respond` for it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::account::SignInMethod;
use super::common::ValidationError;
use super::connector::{
    ApprovalRule, DisplayField, ToolApprovals, ToolKind, UseIn, canonical_json,
};
use super::skill::{SkillAudit, SkillSource};
use super::voice::{CommitmentFlag, OutgoingKind, VoiceUse};
use crate::ids::{
    AccountId, ApprovalId, ChannelId, ClientId, ConnectionId, DecisionId, MessageId, RequestId,
    SessionId, SignInId, SkillId, TaskId, WorkerId,
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
    /// Run a connector tool (a hub card).
    Connector,
    /// A change by a management tool (the management approval card).
    Management,
    /// Connect a service (the connect card).
    ConnectService,
    /// Sign in to a harness account (the sign-in card).
    SignIn,
    /// Install a skill (the install card).
    SkillInstall,
}

impl ApprovalKind {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::FileChange => "file_change",
            Self::Tool => "tool",
            Self::Network => "network",
            Self::Other => "other",
            Self::Connector => "connector",
            Self::Management => "management",
            Self::ConnectService => "connect_service",
            Self::SignIn => "sign_in",
            Self::SkillInstall => "skill_install",
        }
    }

    /// Whether the hub makes approvals of this kind itself.
    pub fn is_hub_card(self) -> bool {
        matches!(
            self,
            Self::Connector
                | Self::Management
                | Self::ConnectService
                | Self::SignIn
                | Self::SkillInstall
        )
    }
}

/// The start of the `request_id` of every hub card: `hub:` and the approval
/// ID. The hub answers a hub card itself and never sends `session.respond`.
pub const HUB_CARD_REQUEST_PREFIX: &str = "hub:";
/// The option ID that allows a hub card.
pub const HUB_CARD_ALLOW: &str = "allow";
/// The option ID that denies a hub card.
pub const HUB_CARD_DENY: &str = "deny";

/// The `request_id` of a hub card.
pub fn hub_card_request_id(approval_id: ApprovalId) -> RequestId {
    RequestId::new(format!("{HUB_CARD_REQUEST_PREFIX}{approval_id}"))
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
    /// A connector tool call whose approval rule asks the user. The card
    /// shows `fields`. Allow runs the call. Don't refuses it.
    Connector(Box<ConnectorCard>),
    /// A management change that asks first. Allow makes the change. Don't
    /// leaves everything as it is.
    Management(Box<ManagementCard>),
    /// The connect card. Continue in browser connects with these settings.
    /// The app can change them first ("Change settings") and then send
    /// `connection.connect` with this approval's ID.
    ConnectService(Box<ConnectServiceCard>),
    /// The sign-in card. The app shows the account's sign-in flow inline,
    /// with the states of the sign-in dialog.
    SignIn(Box<SignInCard>),
    /// The install card, with the safety facts. Add installs the skill. Not
    /// now drops it.
    SkillInstall(Box<SkillInstallCard>),
}

/// A connector tool call whose approval rule asks the user. The card
/// shows `fields`. Allow runs the call. Don't refuses it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorCard {
    /// The connection.
    pub connection_id: ConnectionId,
    /// The connection name, for example "Gmail Work".
    pub connection_name: String,
    /// The service name, for example "Gmail".
    pub service_name: String,
    /// The tool slug, for example `GMAIL_SEND_EMAIL`.
    pub tool: String,
    /// The tool kind.
    pub tool_kind: ToolKind,
    /// The arguments in a form to read, for example To, Subject and
    /// Body.
    #[serde(default)]
    pub fields: Vec<DisplayField>,
    /// The exact arguments that run when the user allows the call. After
    /// an edit, they hold the user's text.
    #[serde(default)]
    pub arguments: Value,
    /// Set when the tool drafts or sends text in the user's name. Only such
    /// a card has Edit, the voice line and commitment flags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outgoing: Option<OutgoingKind>,
    /// The argument keys that the user can edit on the card, such as
    /// `subject` and `body`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub editable: Vec<String>,
    /// The voice that wrote the text. `None` on a draft card is "Neutral
    /// style".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<VoiceUse>,
    /// Phrases that the user must check. While there are flags, an allow
    /// answer must say `checked`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<CommitmentFlag>,
    /// The arguments that the assistant wrote, kept after the user edited
    /// the card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_arguments: Option<Value>,
    /// The user said "Not how I write".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub not_my_voice: bool,
    /// The rule that made the hub ask, set when the card is posted. Only
    /// "Ask once per task" saves an allow for later calls, also when the
    /// answer comes after the call stopped waiting. `None` on a card from
    /// an older hub saves nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_rule: Option<ApprovalRule>,
}

impl ConnectorCard {
    /// Whether the user changed the text of the card.
    pub fn edited(&self) -> bool {
        self.draft_arguments
            .as_ref()
            .is_some_and(|draft| draft != &self.arguments)
    }

    /// The hash of what the user checks on a card with flags: the
    /// arguments and the flags. "I checked the marked lines" holds only for
    /// this hash, so a new scan or an edit from another client clears it.
    pub fn review_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let flags = serde_json::to_value(&self.flags).unwrap_or(Value::Null);
        let mut hasher = Sha256::new();
        hasher.update(canonical_json(&self.arguments).as_bytes());
        hasher.update(b"\n");
        hasher.update(canonical_json(&flags).as_bytes());
        hasher
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut out, b| {
                use std::fmt::Write as _;
                let _ = write!(out, "{b:02x}");
                out
            })
    }
}

/// A management change that asks first. Allow makes the change. Don't
/// leaves everything as it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagementCard {
    /// The management tool, for example `update_connection`.
    pub tool: String,
    /// What changes, for example "Use GitHub Work in channels with the
    /// tag work".
    pub change: String,
    /// Its effect, for example "GitHub Work becomes available in 4 more
    /// channels".
    pub effect: String,
    /// The arguments of the tool call, which the hub runs on Allow.
    #[serde(default)]
    pub arguments: Value,
}

/// The connect card. Continue in browser connects with these settings.
/// The app can change them first ("Change settings") and then send
/// `connection.connect` with this approval's ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectServiceCard {
    /// The Composio toolkit slug.
    pub service: String,
    /// The service name.
    pub service_name: String,
    /// The service logo.
    #[serde(default)]
    pub logo_url: Option<String>,
    /// The connection name.
    pub name: String,
    /// Where the connection can be used.
    pub use_in: UseIn,
    /// The approval rules.
    pub approvals: ToolApprovals,
    /// The connection, after the user continued.
    #[serde(default)]
    pub connection_id: Option<ConnectionId>,
}

/// The sign-in card. The app shows the account's sign-in flow inline,
/// with the states of the sign-in dialog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignInCard {
    /// The worker.
    pub worker_id: WorkerId,
    /// The account.
    pub account_id: AccountId,
    /// The account name.
    pub account_label: String,
    /// The harness.
    pub harness: String,
    /// The method that the assistant asked for. Empty lets the user
    /// choose.
    #[serde(default)]
    pub method: Option<SignInMethod>,
    /// The sign-in, after the user started it.
    #[serde(default)]
    pub sign_in_id: Option<SignInId>,
}

/// The install card, with the safety facts. Add installs the skill. Not
/// now drops it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillInstallCard {
    /// The skill name.
    pub name: String,
    /// What it does.
    pub description: String,
    /// Where it comes from.
    pub source: SkillSource,
    /// The commit that would be pinned.
    #[serde(default)]
    pub commit: Option<String>,
    /// The license.
    #[serde(default)]
    pub license: Option<String>,
    /// The files that can run.
    #[serde(default)]
    pub scripts: Vec<String>,
    /// The audit results.
    #[serde(default)]
    pub audits: Vec<SkillAudit>,
    /// Where it can be used.
    pub use_in: UseIn,
    /// Why the hub would block it. A blocked skill cannot be added.
    #[serde(default)]
    pub blocked_reason: Option<String>,
    /// The skill, after the user added it.
    #[serde(default)]
    pub skill_id: Option<SkillId>,
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
            Self::Connector(_) => ApprovalKind::Connector,
            Self::Management(_) => ApprovalKind::Management,
            Self::ConnectService(_) => ApprovalKind::ConnectService,
            Self::SignIn(_) => ApprovalKind::SignIn,
            Self::SkillInstall(_) => ApprovalKind::SkillInstall,
        }
    }

    /// Whether the hub makes this approval itself (a hub card).
    pub fn is_hub_card(&self) -> bool {
        self.kind().is_hub_card()
    }

    /// The two options of a hub card, with the button text of the design:
    /// Allow and Don't for a connector or management card, Continue in
    /// browser and Not now for a connect card, Sign in and Not now for a
    /// sign-in card, and Add and Not now for an install card. `None` for an
    /// approval from a harness.
    pub fn hub_card_options(&self) -> Option<Vec<RequestOption>> {
        let (allow, deny) = match self.kind() {
            ApprovalKind::Connector | ApprovalKind::Management => ("Allow", "Don't"),
            ApprovalKind::ConnectService => ("Continue in browser", "Not now"),
            ApprovalKind::SignIn => ("Sign in", "Not now"),
            ApprovalKind::SkillInstall => ("Add", "Not now"),
            _ => return None,
        };
        Some(vec![
            RequestOption {
                id: HUB_CARD_ALLOW.to_owned(),
                label: allow.to_owned(),
                kind: RequestOptionKind::AllowOnce,
                description: None,
            },
            RequestOption {
                id: HUB_CARD_DENY.to_owned(),
                label: deny.to_owned(),
                kind: RequestOptionKind::DenyOnce,
                description: None,
            },
        ])
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

    /// Whether the hub made this approval itself. The hub then carries out
    /// the answer, and sends no `session.respond`.
    pub fn is_hub_card(&self) -> bool {
        self.details.is_hub_card()
            && self
                .request_id
                .as_str()
                .starts_with(HUB_CARD_REQUEST_PREFIX)
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

    fn hub_cards() -> Vec<ApprovalDetails> {
        vec![
            ApprovalDetails::Connector(Box::new(ConnectorCard {
                connection_id: ConnectionId::new(),
                connection_name: "Gmail Work".into(),
                service_name: "Gmail".into(),
                tool: "GMAIL_SEND_EMAIL".into(),
                tool_kind: ToolKind::Change,
                fields: vec![
                    DisplayField {
                        label: "To".into(),
                        value: "kevin@example.com".into(),
                    },
                    DisplayField {
                        label: "Subject".into(),
                        value: "Friday".into(),
                    },
                ],
                arguments: json!({"recipient_email": "kevin@example.com", "subject": "Friday"}),
                outgoing: None,
                editable: Vec::new(),
                voice: None,
                flags: Vec::new(),
                draft_arguments: None,
                not_my_voice: false,
                ask_rule: None,
            })),
            ApprovalDetails::Management(Box::new(ManagementCard {
                tool: "update_connection".into(),
                change: "Use GitHub Work in every channel".into(),
                effect: "GitHub Work becomes available in 4 more channels".into(),
                arguments: json!({"connection": "github_work", "use_in": {"kind": "every_channel"}}),
            })),
            ApprovalDetails::ConnectService(Box::new(ConnectServiceCard {
                service: "gmail".into(),
                service_name: "Gmail".into(),
                logo_url: None,
                name: "Gmail Work".into(),
                use_in: UseIn::Tags {
                    tags: vec!["work".into()],
                },
                approvals: ToolApprovals::default(),
                connection_id: None,
            })),
            ApprovalDetails::SignIn(Box::new(SignInCard {
                worker_id: WorkerId::new(),
                account_id: AccountId::new(),
                account_label: "Claude Work".into(),
                harness: "claude".into(),
                method: Some(SignInMethod::ClaudePlan),
                sign_in_id: Some(SignInId::new()),
            })),
            ApprovalDetails::SkillInstall(Box::new(SkillInstallCard {
                name: "pdf-forms".into(),
                description: "Fill PDF forms.".into(),
                source: SkillSource::Git {
                    url: "https://github.com/anthropics/skills".into(),
                    path: Some("pdf".into()),
                    git_ref: None,
                },
                commit: Some("abc123".into()),
                license: Some("MIT".into()),
                scripts: vec!["scripts/fill.py".into()],
                audits: vec![],
                use_in: UseIn::EveryChannel,
                blocked_reason: None,
                skill_id: None,
            })),
        ]
    }

    #[test]
    fn hub_card_details_round_trip() {
        let kinds = [
            "connector",
            "management",
            "connect_service",
            "sign_in",
            "skill_install",
        ];
        for (details, kind) in hub_cards().into_iter().zip(kinds) {
            let value = serde_json::to_value(&details).unwrap();
            assert_eq!(value["kind"], kind);
            assert_eq!(details.kind().as_str(), kind);
            assert_eq!(serde_json::to_value(details.kind()).unwrap(), json!(kind));
            assert!(details.is_hub_card());
            let options = details.hub_card_options().unwrap();
            assert_eq!(options.len(), 2);
            assert_eq!(options[0].id, HUB_CARD_ALLOW);
            assert!(options[0].kind.is_allow());
            assert_eq!(options[1].id, HUB_CARD_DENY);
            assert!(options[1].kind.is_deny());
            assert_eq!(
                serde_json::from_value::<ApprovalDetails>(value).unwrap(),
                details
            );
        }
        let labels: Vec<String> = hub_cards()
            .iter()
            .map(|d| d.hub_card_options().unwrap()[0].label.clone())
            .collect();
        assert_eq!(
            labels,
            ["Allow", "Allow", "Continue in browser", "Sign in", "Add"]
        );
        let harness = ApprovalDetails::Network {
            target: "github.com".into(),
        };
        assert!(!harness.is_hub_card());
        assert_eq!(harness.hub_card_options(), None);
    }

    #[test]
    fn a_hub_card_approval_needs_the_hub_request_id() {
        let id = ApprovalId::new();
        let mut approval = Approval {
            id,
            channel_id: ChannelId::new(),
            session_id: SessionId::new(),
            task_id: None,
            message_id: MessageId::new(),
            request_id: hub_card_request_id(id),
            summary: "Send an email with Gmail Work".into(),
            details: hub_cards().remove(0),
            options: hub_cards()[0].hub_card_options().unwrap(),
            status: ApprovalStatus::Pending,
            answer_option_id: None,
            answered_at: None,
            answered_by_client_id: None,
            created_at: crate::time::Timestamp::from_millis(1),
        };
        assert!(approval.request_id.as_str().starts_with("hub:apr_"));
        assert!(approval.is_hub_card());
        let back: Approval =
            serde_json::from_value(serde_json::to_value(&approval).unwrap()).unwrap();
        assert_eq!(back, approval);
        // A harness cannot make a hub card by naming the kind.
        approval.request_id = RequestId::new("toolu_1");
        assert!(!approval.is_hub_card());
    }

    #[test]
    fn option_kinds() {
        assert!(RequestOptionKind::AllowAlways.is_allow());
        assert!(RequestOptionKind::DenyOnce.is_deny());
        assert!(!RequestOptionKind::Choice.is_allow());
        assert!(!RequestOptionKind::Choice.is_deny());
    }
}
