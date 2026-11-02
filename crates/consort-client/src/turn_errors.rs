//! Error cards: how the client explains a turn that failed.
//!
//! The hub posts a `turn_failed` notice (see protocol.md, "turn_failed
//! notice"). The notice has the kind of the failure, the reset time of a
//! limit, the name of the account and the raw text of the worker, all of
//! them optional. An old notice has only a text, and it reads as `Unknown`.
//! The view turns that into a title, an explanation with the next step,
//! the actions that fit, and the raw details.

use consort_proto::model::TurnErrorKind;
use serde::Deserialize;

use crate::format;

/// The kind of a failed turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TurnErrorKindView {
    /// The account reached a usage limit or a rate limit.
    RateLimit,
    /// The sign-in of the account ended.
    ExpiredSignIn,
    /// The provider was down or overloaded for three tries.
    ProviderOutage,
    /// The worker restarted during the turn.
    WorkerRestarted,
    /// Anything else, and every notice from before the kinds existed.
    #[default]
    Unknown,
}

impl From<TurnErrorKind> for TurnErrorKindView {
    fn from(kind: TurnErrorKind) -> Self {
        match kind {
            TurnErrorKind::RateLimit => Self::RateLimit,
            TurnErrorKind::ExpiredSignIn => Self::ExpiredSignIn,
            TurnErrorKind::ProviderOutage => Self::ProviderOutage,
            TurnErrorKind::WorkerRestarted => Self::WorkerRestarted,
            TurnErrorKind::Unknown => Self::Unknown,
        }
    }
}

/// What the user can do about a failed turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorAction {
    /// Open the model picker. For a task, it changes the model of the task
    /// (`Client::set_task_model`).
    ChooseAnotherModel,
    /// Open the account of the failed turn, to sign in again.
    SignInAgain,
    /// Open Settings > Models, where the user edits the fallback path.
    EditPath,
    /// `Client::try_again` with the target of the card.
    TryAgain,
}

impl ErrorAction {
    /// The button text.
    pub fn label(self) -> &'static str {
        match self {
            Self::ChooseAnotherModel => "Choose another model",
            Self::SignInAgain => "Sign in again",
            Self::EditPath => "Edit path",
            Self::TryAgain => "Try again",
        }
    }
}

/// What `Client::try_again` does for a card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TryAgainTarget {
    /// Retry the task. The hub starts a new session from the brief.
    Task { task_id: String },
    /// Send the last message of the user again, as a new message in the
    /// same channel. The channel assistant then answers it again. The first
    /// message stays in the timeline, so it shows twice. This works only for
    /// a failure that a message of the user started: a failed routine run or
    /// check-in has no message to send.
    ResendMessage { channel_id: String, body: String },
    /// The first turn of the guided start failed before the assistant spoke,
    /// so there is no message to send. The hub queues the turn again.
    Greeting,
}

/// The words and the actions for one failed turn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ErrorCardView {
    pub kind: TurnErrorKindView,
    /// For example "Claude Work reached its weekly limit".
    pub title: String,
    /// What happened and what to do next, in plain words.
    pub explanation: String,
    /// The buttons, in order. `TryAgain` is there only when `try_again` is
    /// set.
    pub actions: Vec<ErrorAction>,
    /// The raw text of the worker, for a details section. Empty when the
    /// worker gave none.
    pub details: String,
    /// The name of the account of the turn, when the hub knew it.
    pub account: Option<String>,
    /// When a limit resets, in Unix milliseconds.
    pub resets_at: Option<i64>,
    /// One line for each step that the path tried or skipped. Empty when the
    /// hub sent no path. The card then reads as an error of one account.
    pub path: Vec<crate::fallback::PathLineView>,
    /// What `Client::try_again` does. `None` when nothing can be tried
    /// again.
    pub try_again: Option<TryAgainTarget>,
    /// A newer message of the user, or a retry, came after this failure.
    /// The app then shows the one line `summary` and no buttons.
    pub quiet: bool,
    /// The quiet line: the title.
    pub summary: String,
}

/// The fields of a `turn_failed` notice body.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnFailed {
    pub kind: TurnErrorKindView,
    pub resets_at: Option<i64>,
    pub account: Option<String>,
    /// The raw text of the worker, or the notice text of an old notice.
    pub raw: String,
    /// The lines of the path report, as the hub sent them.
    pub path: Vec<consort_proto::model::PathLine>,
}

/// Reads a notice body. `None` when the body is not a `turn_failed`
/// notice. A body without a kind reads as `Unknown`, so a notice from an
/// older hub still gets a card.
pub fn parse_turn_failed(body: &str) -> Option<TurnFailed> {
    #[derive(Deserialize)]
    struct Parsed {
        #[serde(default)]
        kind: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        error_kind: Option<String>,
        #[serde(default)]
        resets_at: Option<consort_proto::time::Timestamp>,
        #[serde(default)]
        account: Option<String>,
        #[serde(default)]
        raw: Option<String>,
        #[serde(default)]
        path: Vec<consort_proto::model::PathLine>,
    }
    let parsed: Parsed = serde_json::from_str(body).ok()?;
    if parsed.kind != "turn_failed" {
        return None;
    }
    Some(TurnFailed {
        kind: parsed
            .error_kind
            .as_deref()
            .map(TurnErrorKind::from_name)
            .map(TurnErrorKindView::from)
            .unwrap_or_default(),
        resets_at: parsed.resets_at.map(|t| t.as_millis()),
        account: parsed.account.filter(|a| !a.trim().is_empty()),
        raw: parsed
            .raw
            .filter(|r| !r.trim().is_empty())
            .unwrap_or(parsed.text),
        path: parsed.path,
    })
}

/// The title of a card.
pub fn title(kind: TurnErrorKindView, account: Option<&str>) -> String {
    match (kind, account) {
        (TurnErrorKindView::ProviderOutage, _) => "The model provider did not answer".to_owned(),
        (TurnErrorKindView::RateLimit, Some(a)) => format!("{a} reached its weekly limit"),
        (TurnErrorKindView::RateLimit, None) => "The account reached its weekly limit".to_owned(),
        (TurnErrorKindView::ExpiredSignIn, Some(a)) => format!("{a} needs a new sign-in"),
        (TurnErrorKindView::ExpiredSignIn, None) => "The account needs a new sign-in".to_owned(),
        (TurnErrorKindView::WorkerRestarted, _) => {
            "The worker restarted during this turn".to_owned()
        }
        (TurnErrorKindView::Unknown, _) => "The assistant stopped with an error".to_owned(),
    }
}

/// When a limit resets, such as "Friday at 9:00 AM".
pub fn reset_label(resets_at: i64, now: i64) -> String {
    format!(
        "{} at {}",
        format::day_label(resets_at, now).to_lowercase_first(),
        format::time_label(resets_at)
    )
}

trait LowercaseFirst {
    fn to_lowercase_first(&self) -> String;
}

impl LowercaseFirst for String {
    /// "Today" reads as "today" inside a sentence. A weekday or a date
    /// keeps its capital.
    fn to_lowercase_first(&self) -> String {
        match self.as_str() {
            "Today" => "today".to_owned(),
            "Yesterday" => "yesterday".to_owned(),
            _ => self.clone(),
        }
    }
}

/// What happened and what to do next.
pub fn explanation(
    kind: TurnErrorKindView,
    account: Option<&str>,
    resets_at: Option<i64>,
    now: i64,
) -> String {
    let who = account.unwrap_or("The account");
    match kind {
        TurnErrorKindView::RateLimit => {
            let until = match resets_at {
                Some(at) => format!("until {}", reset_label(at, now)),
                None => "until its limit resets".to_owned(),
            };
            format!(
                "{who} cannot run more turns {until}. Choose another model to go on now, or try again after the reset."
            )
        }
        TurnErrorKindView::ExpiredSignIn => format!(
            "The sign-in of {} ended. Sign in again, then try again. You can also choose another model.",
            account.unwrap_or("the account")
        ),
        TurnErrorKindView::ProviderOutage => format!(
            "The provider of {} was down or overloaded for {} tries. Choose another model to go on now, or try again later.",
            account.unwrap_or("the account"),
            consort_proto::model::OUTAGE_TRIES
        ),
        TurnErrorKindView::WorkerRestarted => {
            "The worker stopped and started again, so this turn did not finish. Try again."
                .to_owned()
        }
        TurnErrorKindView::Unknown => {
            "The assistant stopped and gave an error. Try again. The details show what it said."
                .to_owned()
        }
    }
}

/// The buttons for a kind, in order. `can_try_again` is false when there is
/// nothing to try again.
pub fn actions(kind: TurnErrorKindView, can_try_again: bool) -> Vec<ErrorAction> {
    let mut actions = match kind {
        TurnErrorKindView::RateLimit | TurnErrorKindView::ProviderOutage => {
            vec![ErrorAction::ChooseAnotherModel, ErrorAction::TryAgain]
        }
        TurnErrorKindView::ExpiredSignIn => vec![
            ErrorAction::SignInAgain,
            ErrorAction::ChooseAnotherModel,
            ErrorAction::TryAgain,
        ],
        TurnErrorKindView::WorkerRestarted | TurnErrorKindView::Unknown => {
            vec![ErrorAction::TryAgain]
        }
    };
    if !can_try_again {
        actions.retain(|a| *a != ErrorAction::TryAgain);
    }
    actions
}

/// The card for one failed turn. `quiet` is true when the failure is old:
/// see [`ErrorCardView::quiet`]. `now` is Unix milliseconds.
pub fn error_card(
    failed: &TurnFailed,
    try_again: Option<TryAgainTarget>,
    quiet: bool,
    now: i64,
) -> ErrorCardView {
    // A turn that no step of the path could answer has its own words.
    let path_failed = !failed.path.is_empty()
        && matches!(
            failed.kind,
            TurnErrorKindView::RateLimit | TurnErrorKindView::ProviderOutage
        );
    let title = if path_failed {
        "No model in the path could answer".to_owned()
    } else {
        title(failed.kind, failed.account.as_deref())
    };
    let explanation = if path_failed {
        let who = if matches!(try_again, Some(TryAgainTarget::Task { .. })) {
            "The task stopped"
        } else {
            "Your message was not answered"
        };
        format!("{who}. Consort tried every step. Choose a model to go on now, or edit the path.")
    } else {
        explanation(
            failed.kind,
            failed.account.as_deref(),
            failed.resets_at,
            now,
        )
    };
    let mut actions = if quiet {
        Vec::new()
    } else {
        actions(failed.kind, try_again.is_some())
    };
    if path_failed
        && !quiet
        && let Some(at) = actions
            .iter()
            .position(|a| *a == ErrorAction::ChooseAnotherModel)
    {
        actions.insert(at + 1, ErrorAction::EditPath);
    }
    ErrorCardView {
        kind: failed.kind,
        path: crate::fallback::path_line_views(
            &failed.path,
            now,
            matches!(try_again, Some(TryAgainTarget::Task { .. })),
        ),
        explanation,
        actions,
        details: failed.raw.clone(),
        account: failed.account.clone(),
        resets_at: failed.resets_at,
        try_again,
        quiet,
        summary: title.clone(),
        title,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(kind: TurnErrorKindView) -> TurnFailed {
        TurnFailed {
            kind,
            resets_at: None,
            account: Some("Claude Work".to_owned()),
            raw: "raw text".to_owned(),
            path: Vec::new(),
        }
    }

    #[test]
    fn the_four_titles_are_exact() {
        let name = Some("Claude Work");
        assert_eq!(
            title(TurnErrorKindView::RateLimit, name),
            "Claude Work reached its weekly limit"
        );
        assert_eq!(
            title(TurnErrorKindView::ExpiredSignIn, name),
            "Claude Work needs a new sign-in"
        );
        assert_eq!(
            title(TurnErrorKindView::WorkerRestarted, name),
            "The worker restarted during this turn"
        );
        assert_eq!(
            title(TurnErrorKindView::Unknown, name),
            "The assistant stopped with an error"
        );
    }

    #[test]
    fn a_title_without_an_account_name_says_the_account() {
        assert_eq!(
            title(TurnErrorKindView::RateLimit, None),
            "The account reached its weekly limit"
        );
        assert_eq!(
            title(TurnErrorKindView::ExpiredSignIn, None),
            "The account needs a new sign-in"
        );
    }

    #[test]
    fn a_provider_outage_has_its_own_words_and_actions() {
        let body = r#"{"kind":"turn_failed","text":"x","error_kind":"provider_outage","account":"Codex Work"}"#;
        let failed = parse_turn_failed(body).unwrap();
        assert_eq!(failed.kind, TurnErrorKindView::ProviderOutage);
        let card = error_card(&failed, None, false, 0);
        assert_eq!(card.title, "The model provider did not answer");
        assert!(card.explanation.contains("3 tries"));
        assert_eq!(card.actions, [ErrorAction::ChooseAnotherModel]);
        assert!(card.path.is_empty());
        assert_eq!(ErrorAction::EditPath.label(), "Edit path");
    }

    #[test]
    fn the_actions_fit_the_kind() {
        use ErrorAction::*;
        assert_eq!(
            actions(TurnErrorKindView::RateLimit, true),
            [ChooseAnotherModel, TryAgain]
        );
        assert_eq!(
            actions(TurnErrorKindView::ExpiredSignIn, true),
            [SignInAgain, ChooseAnotherModel, TryAgain]
        );
        assert_eq!(
            actions(TurnErrorKindView::WorkerRestarted, true),
            [TryAgain]
        );
        assert_eq!(actions(TurnErrorKindView::Unknown, true), [TryAgain]);
        // Nothing to try again: the button is not offered.
        assert_eq!(
            actions(TurnErrorKindView::RateLimit, false),
            [ChooseAnotherModel]
        );
        assert!(actions(TurnErrorKindView::Unknown, false).is_empty());
        assert_eq!(TryAgain.label(), "Try again");
        assert_eq!(ChooseAnotherModel.label(), "Choose another model");
        assert_eq!(SignInAgain.label(), "Sign in again");
    }

    #[test]
    fn an_old_notice_reads_as_unknown_with_its_text_as_the_details() {
        let body = r#"{"kind":"turn_failed","text":"boom"}"#;
        let failed = parse_turn_failed(body).unwrap();
        assert_eq!(failed.kind, TurnErrorKindView::Unknown);
        assert_eq!(failed.raw, "boom");
        assert_eq!(failed.account, None);
        assert_eq!(failed.resets_at, None);
        // A kind from a newer hub reads as unknown too.
        let body = r#"{"kind":"turn_failed","text":"x","error_kind":"from_the_future"}"#;
        assert_eq!(
            parse_turn_failed(body).unwrap().kind,
            TurnErrorKindView::Unknown
        );
        // Other notices are not errors.
        assert!(parse_turn_failed(r#"{"kind":"assistant_unavailable","text":"x"}"#).is_none());
        assert!(parse_turn_failed("not json").is_none());
    }

    #[test]
    fn a_new_notice_gives_kind_reset_account_and_raw_text() {
        let body = r#"{"kind":"turn_failed","text":"You hit your limit.","error_kind":"rate_limit","resets_at":1900000000000,"account":"Claude Work","raw":"limit reached|1900000000"}"#;
        let failed = parse_turn_failed(body).unwrap();
        assert_eq!(failed.kind, TurnErrorKindView::RateLimit);
        assert_eq!(failed.resets_at, Some(1_900_000_000_000));
        assert_eq!(failed.account.as_deref(), Some("Claude Work"));
        assert_eq!(failed.raw, "limit reached|1900000000");
    }

    #[test]
    fn the_card_holds_title_explanation_actions_and_details() {
        let target = TryAgainTarget::Task {
            task_id: "tsk_1".into(),
        };
        let card = error_card(
            &failed(TurnErrorKindView::RateLimit),
            Some(target.clone()),
            false,
            1_900_000_000_000,
        );
        assert_eq!(card.title, "Claude Work reached its weekly limit");
        assert!(card.explanation.contains("Choose another model"));
        assert!(card.explanation.contains("until its limit resets"));
        assert_eq!(card.details, "raw text");
        assert_eq!(card.try_again, Some(target));
        assert_eq!(
            card.actions,
            [ErrorAction::ChooseAnotherModel, ErrorAction::TryAgain]
        );
        assert!(!card.quiet);
    }

    #[test]
    fn a_quiet_card_keeps_the_title_as_one_line_and_offers_no_action() {
        let card = error_card(&failed(TurnErrorKindView::ExpiredSignIn), None, true, 0);
        assert!(card.quiet);
        assert!(card.actions.is_empty());
        assert_eq!(card.summary, "Claude Work needs a new sign-in");
    }

    #[test]
    fn a_known_reset_time_is_in_the_explanation() {
        let at = 1_900_000_000_000;
        let text = explanation(
            TurnErrorKindView::RateLimit,
            Some("Claude Work"),
            Some(at),
            at,
        );
        assert!(text.contains("until today at "), "{text}");
        assert!(!text.contains("its limit resets"), "{text}");
    }

    #[test]
    fn no_text_has_a_dash_as_a_sentence_dash() {
        for kind in [
            TurnErrorKindView::RateLimit,
            TurnErrorKindView::ExpiredSignIn,
            TurnErrorKindView::WorkerRestarted,
            TurnErrorKindView::Unknown,
        ] {
            let text = format!(
                "{} {}",
                title(kind, Some("A")),
                explanation(kind, Some("A"), None, 0)
            );
            assert!(
                !text.contains('\u{2014}') && !text.contains('\u{2013}'),
                "{text}"
            );
        }
    }
}
