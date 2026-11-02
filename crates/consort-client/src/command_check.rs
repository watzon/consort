//! The API endpoints of Settings > Providers and the command check of
//! Settings > Models, as the app shows them. The keys stay on the hub.

use consort_proto::model::{
    ApiEndpoint, CommandCheckEndpoint, CommandCheckKind, CommandCheckOutcome, CommandCheckSetting,
    CommandCheckVerdict, DEFAULT_CONFIDENCE_LIMIT, FavoriteModel, HarnessAccount,
};

use crate::view::{ModelGroupView, SelectionView};

/// The confidence limits that the select offers, in percent.
pub const CONFIDENCE_LIMITS: [u8; 4] = [70, 80, 90, 95];

/// The boundary around a session on the session host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryView {
    MacSandbox,
    Bubblewrap,
}

/// The warning of Settings > Hub. The Mac sandbox is for development. With
/// no boundary, no session starts.
pub fn boundary_warning(host: Option<&crate::view::WorkerView>) -> Option<String> {
    let host = host.filter(|h| h.online)?;
    match host.boundary {
        Some(BoundaryView::Bubblewrap) => None,
        Some(BoundaryView::MacSandbox) => Some(
            "This hub runs on a Mac, so the agents run in a development sandbox. Keychain items are not separate for each profile. Use a Linux server for real work."
                .to_owned(),
        ),
        None => Some(
            "The session host found no boundary, so no session can start. Install bubblewrap on the hub machine, then restart the session host."
                .to_owned(),
        ),
    }
}

/// An API endpoint row. `in_use` means that the command check uses it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EndpointView {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub key_last_four: String,
    pub in_use: bool,
}

/// The kind of source of the command check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckSourceKind {
    #[default]
    Account,
    Decision,
    Chat,
}

impl CheckSourceKind {
    pub const ALL: [CheckSourceKind; 3] = [Self::Account, Self::Decision, Self::Chat];

    /// The words in the select.
    pub fn label(self) -> &'static str {
        match self {
            Self::Account => "A model of an account",
            Self::Decision => "Decision model",
            Self::Chat => "Chat model",
        }
    }

    /// The line under a choice of the select.
    pub fn description(self) -> &'static str {
        match self {
            Self::Account => "Uses a small model of an account that is signed in.",
            Self::Decision => "Asks named questions on an API endpoint. It gives no reason.",
            Self::Chat => "Asks a model on an API endpoint. It costs money for each check.",
        }
    }
}

/// A model name on an endpoint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckEndpointChoice {
    pub endpoint_id: String,
    pub model: String,
    pub confidence_limit: u8,
}

/// What the user chose. A change of kind keeps the choices of the other
/// kinds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandCheckChoice {
    pub kind: CheckSourceKind,
    /// The model of an account. `None` means that Consort chooses.
    pub account_model: Option<SelectionView>,
    pub decision: Option<CheckEndpointChoice>,
    pub chat: Option<CheckEndpointChoice>,
}

/// The command check section of Settings > Models.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandCheckView {
    pub choice: CommandCheckChoice,
    /// The model that Consort chooses when the user chose none.
    pub chosen_for_you: Option<SelectionView>,
}

impl CommandCheckView {
    /// The account model that runs. `None` shows "No model yet".
    pub fn account_model(&self) -> Option<&SelectionView> {
        self.choice
            .account_model
            .as_ref()
            .or(self.chosen_for_you.as_ref())
    }

    /// What the source still needs before a test can run, in words for the
    /// user. `model` is the text in the Model name field now, which the hub
    /// may not have yet. `any_endpoint` is true when an endpoint exists. A new
    /// choice takes the first endpoint.
    pub fn test_blocked(&self, model: &str, any_endpoint: bool) -> Option<&'static str> {
        let chosen = match self.choice.kind {
            CheckSourceKind::Account => {
                return self
                    .account_model()
                    .is_none()
                    .then_some("Choose a model for the command check, then test.");
            }
            CheckSourceKind::Decision => self.choice.decision.as_ref(),
            CheckSourceKind::Chat => self.choice.chat.as_ref(),
        };
        if !chosen.is_some_and(|c| !c.endpoint_id.is_empty()) && !any_endpoint {
            return Some("Add an endpoint in Settings > Providers, then test.");
        }
        model
            .trim()
            .is_empty()
            .then_some("Type a model name, then test.")
    }
}

/// The result of a test, in one line and with details to copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestResultView {
    pub ok: bool,
    pub message: String,
    pub details: Option<String>,
}

/// What the command check said about one command, for the Details of its
/// item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckDetailView {
    pub blocked: bool,
    /// "Allowed by the command check, Haiku 5.5 of Claude Work."
    pub check: String,
    pub reason: Option<String>,
    /// "Is this command part of what the user asked? No, 91 percent. The
    /// limit is 80 percent."
    pub rule: Option<String>,
    pub error: Option<String>,
}

/// The rows of Details for one outcome.
pub fn check_detail(outcome: &CommandCheckOutcome) -> CheckDetailView {
    let source = &outcome.source;
    let check = match outcome.verdict {
        CommandCheckVerdict::Allowed if outcome.command_rule.is_some() => {
            let what = outcome.command_rule.as_deref().unwrap_or_default();
            format!("Allowed by a command rule: {what}.")
        }
        CommandCheckVerdict::Allowed => format!("Allowed by the command check, {source}."),
        CommandCheckVerdict::Blocked => format!("Blocked by the command check, {source}."),
        CommandCheckVerdict::Asked => format!("The command check could not decide, {source}."),
    };
    CheckDetailView {
        blocked: outcome.verdict == CommandCheckVerdict::Blocked,
        check,
        reason: outcome.reason.clone(),
        rule: outcome.rule.as_ref().map(|rule| {
            format!(
                "{} {}, {} percent. The limit is {} percent.",
                rule.question, rule.answer, rule.confidence, rule.limit
            )
        }),
        error: outcome.error.clone(),
    }
}

/// The endpoints with their use.
pub(crate) fn endpoints_view(
    endpoints: &[ApiEndpoint],
    setting: &CommandCheckSetting,
) -> Vec<EndpointView> {
    let active = match setting.kind {
        CommandCheckKind::Account => None,
        CommandCheckKind::Decision => setting.decision.as_ref(),
        CommandCheckKind::Chat => setting.chat.as_ref(),
    };
    endpoints
        .iter()
        .map(|e| EndpointView {
            id: e.id.clone(),
            name: e.name.clone(),
            base_url: e.base_url.clone(),
            key_last_four: e.key_last_four.clone(),
            in_use: active.is_some_and(|c| c.endpoint_id == e.id),
        })
        .collect()
}

fn selection_of(groups: &[ModelGroupView], account_label: &str, model_id: &str) -> SelectionView {
    let model_name = groups
        .iter()
        .find(|g| g.account_label == account_label)
        .and_then(|g| g.models.iter().find(|m| m.id == model_id))
        .map_or_else(|| model_id.to_owned(), |m| m.name.clone());
    SelectionView {
        account_label: account_label.to_owned(),
        model_id: model_id.to_owned(),
        model_name,
        options: Vec::new(),
    }
}

fn endpoint_choice(c: &CommandCheckEndpoint) -> CheckEndpointChoice {
    CheckEndpointChoice {
        endpoint_id: c.endpoint_id.clone(),
        model: c.model.clone(),
        confidence_limit: c.confidence_limit,
    }
}

/// The section of Settings > Models. `accounts` are the accounts of the
/// session host. The hub picks the same model when the user chose none.
pub(crate) fn command_check_view<'a>(
    setting: &CommandCheckSetting,
    accounts: impl Iterator<Item = &'a HarnessAccount>,
    groups: &[ModelGroupView],
) -> CommandCheckView {
    let chosen_for_you = accounts
        .filter(|a| {
            a.enabled
                && a.blocked_reason.is_none()
                && a.auth_state == consort_proto::model::AuthState::SignedIn
        })
        .find_map(|a| a.smallest_model().map(|m| (a, m)))
        .map(|(a, m)| SelectionView {
            account_label: a.label.clone(),
            model_id: m.id.clone(),
            model_name: m.name.clone(),
            options: Vec::new(),
        });
    CommandCheckView {
        choice: CommandCheckChoice {
            kind: match setting.kind {
                CommandCheckKind::Account => CheckSourceKind::Account,
                CommandCheckKind::Decision => CheckSourceKind::Decision,
                CommandCheckKind::Chat => CheckSourceKind::Chat,
            },
            account_model: setting
                .account_model
                .as_ref()
                .map(|m| selection_of(groups, &m.account_label, &m.model)),
            decision: setting.decision.as_ref().map(endpoint_choice),
            chat: setting.chat.as_ref().map(endpoint_choice),
        },
        chosen_for_you,
    }
}

/// The wire form of what the user chose.
pub(crate) fn setting_of(choice: &CommandCheckChoice) -> CommandCheckSetting {
    let endpoint = |c: &CheckEndpointChoice| CommandCheckEndpoint {
        endpoint_id: c.endpoint_id.clone(),
        model: c.model.clone(),
        confidence_limit: if c.confidence_limit == 0 {
            DEFAULT_CONFIDENCE_LIMIT
        } else {
            c.confidence_limit
        },
    };
    CommandCheckSetting {
        kind: match choice.kind {
            CheckSourceKind::Account => CommandCheckKind::Account,
            CheckSourceKind::Decision => CommandCheckKind::Decision,
            CheckSourceKind::Chat => CommandCheckKind::Chat,
        },
        account_model: choice.account_model.as_ref().map(|m| FavoriteModel {
            account_label: m.account_label.clone(),
            model: m.model_id.clone(),
        }),
        decision: choice.decision.as_ref().map(endpoint),
        chat: choice.chat.as_ref().map(endpoint),
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::model::{CommandCheckRule, ModelInfo};

    use super::*;

    fn account(label: &str, models: &[&str]) -> HarnessAccount {
        serde_json::from_value(serde_json::json!({
            "account_id": consort_proto::ids::AccountId::new(),
            "label": label,
            "harness": "claude",
            "enabled": true,
            "auth_state": "signed_in",
            "models": models
                .iter()
                .map(|id| ModelInfo {
                    id: (*id).to_owned(),
                    name: (*id).to_owned(),
                    options: Vec::new(),
                })
                .collect::<Vec<_>>(),
        }))
        .expect("an account")
    }

    #[test]
    fn the_warning_follows_the_boundary() {
        let mut host = crate::view::WorkerView {
            online: true,
            ..Default::default()
        };
        assert!(boundary_warning(None).is_none());
        assert!(
            boundary_warning(Some(&host))
                .unwrap()
                .contains("no boundary")
        );
        host.boundary = Some(BoundaryView::MacSandbox);
        assert!(boundary_warning(Some(&host)).unwrap().contains("Keychain"));
        host.boundary = Some(BoundaryView::Bubblewrap);
        assert!(boundary_warning(Some(&host)).is_none());
        host.online = false;
        host.boundary = None;
        assert!(boundary_warning(Some(&host)).is_none());
    }

    #[test]
    fn consort_chooses_the_smallest_model_of_a_signed_in_account() {
        let accounts = [
            account("Empty", &[]),
            account("Claude Work", &["opus", "haiku", "sonnet"]),
        ];
        let view = command_check_view(&CommandCheckSetting::default(), accounts.iter(), &[]);
        let chosen = view.account_model().expect("a model");
        assert_eq!(chosen.account_label, "Claude Work");
        assert_eq!(chosen.model_id, "haiku");
        assert_eq!(view.test_blocked("", false), None);
    }

    #[test]
    fn no_account_means_no_model_and_no_test() {
        let view = command_check_view(&CommandCheckSetting::default(), [].iter(), &[]);
        assert!(view.account_model().is_none());
        assert!(view.test_blocked("", false).is_some());
    }

    #[test]
    fn an_endpoint_source_needs_an_endpoint_and_a_model_name() {
        let mut view = CommandCheckView::default();
        view.choice.kind = CheckSourceKind::Decision;
        assert!(view.test_blocked("m", false).unwrap().contains("endpoint"));
        // A new choice takes the first endpoint.
        assert_eq!(view.test_blocked("m", true), None);
        view.choice.decision = Some(CheckEndpointChoice {
            endpoint_id: "e".to_owned(),
            model: " ".to_owned(),
            confidence_limit: 80,
        });
        assert!(
            view.test_blocked(" ", false)
                .unwrap()
                .contains("model name")
        );
        // The text in the field counts, before the hub has it.
        assert_eq!(view.test_blocked("m", false), None);
    }

    #[test]
    fn an_endpoint_is_in_use_only_for_the_kind_that_runs() {
        let endpoint = ApiEndpoint {
            id: "e".to_owned(),
            name: "OpenRouter".to_owned(),
            base_url: "https://x".to_owned(),
            key_last_four: "4f2a".to_owned(),
        };
        let choice = CommandCheckEndpoint {
            endpoint_id: "e".to_owned(),
            model: "m".to_owned(),
            confidence_limit: 80,
        };
        let mut setting = CommandCheckSetting {
            decision: Some(choice),
            ..CommandCheckSetting::default()
        };
        assert!(!endpoints_view(std::slice::from_ref(&endpoint), &setting)[0].in_use);
        setting.kind = CommandCheckKind::Decision;
        assert!(endpoints_view(&[endpoint], &setting)[0].in_use);
    }

    #[test]
    fn details_name_the_source_and_the_rule() {
        let detail = check_detail(&CommandCheckOutcome {
            command_rule: None,
            verdict: CommandCheckVerdict::Blocked,
            source: "jev on OpenRouter".to_owned(),
            reason: None,
            rule: Some(CommandCheckRule {
                question: "Did the user ask for this?".to_owned(),
                answer: "no".to_owned(),
                confidence: 91,
                limit: 80,
            }),
            error: None,
        });
        assert!(detail.blocked);
        assert_eq!(
            detail.check,
            "Blocked by the command check, jev on OpenRouter."
        );
        assert_eq!(
            detail.rule.as_deref(),
            Some("Did the user ask for this? no, 91 percent. The limit is 80 percent.")
        );
    }

    #[test]
    fn the_wire_form_keeps_the_choices_of_each_kind() {
        let choice = CommandCheckChoice {
            kind: CheckSourceKind::Chat,
            account_model: Some(SelectionView {
                account_label: "Claude Work".to_owned(),
                model_id: "haiku".to_owned(),
                ..SelectionView::default()
            }),
            decision: Some(CheckEndpointChoice {
                endpoint_id: "e".to_owned(),
                model: "jev".to_owned(),
                confidence_limit: 0,
            }),
            chat: None,
        };
        let setting = setting_of(&choice);
        assert_eq!(setting.kind, CommandCheckKind::Chat);
        assert_eq!(setting.decision.expect("kept").confidence_limit, 80);
        assert_eq!(setting.account_model.expect("kept").model, "haiku");
    }

    #[test]
    fn details_name_the_command_rule_that_allowed_it() {
        let detail = check_detail(&CommandCheckOutcome {
            command_rule: Some("Push to a branch".to_owned()),
            verdict: CommandCheckVerdict::Allowed,
            source: String::new(),
            reason: None,
            rule: None,
            error: None,
        });
        assert_eq!(detail.check, "Allowed by a command rule: Push to a branch.");
    }
}
