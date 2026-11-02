//! The fallback path as the clients show it: the page in Settings > Models,
//! the marker on the model chip, the notices of a move, and the lines of an
//! error card. Every function is pure, so each client shows the same words
//! from the same state. See "The fallback path" in architecture.md.

use consort_proto::model::{
    AuthState, CoordinatorFallback, FallbackCause, FallbackNotice, FallbackPath, FallbackScope,
    OUTAGE_TRIES, PathLine, PathOutcome, PathPlace, SkipReason, harness_company,
};
use serde::Deserialize;

use crate::cache::Cache;
use crate::format;
use crate::picker::level_label;
use crate::reduce::selection_view;
use crate::turn_errors::reset_label;
use crate::view::{ModelGroupView, ProviderLogo, SelectionView};

/// The most steps that the path can have.
pub use consort_proto::model::FALLBACK_STEPS_MAX;

/// The notice kind of a move.
pub const MOVED_KIND: &str = "account_fallback";
/// The notice kind of an assistant that went back to its first choice.
pub const RETURNED_KIND: &str = "fallback_returned";
/// The notice kind of a move that skipped a signed-out account.
pub const SIGNED_OUT_KIND: &str = "fallback_signed_out";

/// Why a step cannot run now. The page shows a ring and one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StepSkip {
    /// The account has no sign-in now.
    SignedOut,
    /// No signed-in account has this label and this model.
    Unavailable,
}

impl StepSkip {
    /// The line under the step, such as "Skipped now. This account is signed
    /// out."
    pub fn label(self) -> &'static str {
        match self {
            Self::SignedOut => "Skipped now. This account is signed out.",
            Self::Unavailable => "Skipped now. No signed-in account has this model.",
        }
    }
}

/// One step of the path, or the first choice, as the page shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FallbackStepView {
    /// The account, the model and the options (the effort).
    pub selection: SelectionView,
    /// The provider name, such as "Codex".
    pub provider: String,
    pub logo: ProviderLogo,
    /// The company that gets the work, such as "OpenAI".
    pub company: String,
    /// The effort in words, such as "High effort". `None` when the
    /// selection has no effort.
    pub effort_label: Option<String>,
    /// The step is on another harness than the first choice. The work and
    /// its context then go to another company.
    pub other_harness: bool,
    /// Why the path skips the step now. `None` when it can run.
    pub skipped: Option<StepSkip>,
}

/// The fallback path for Settings > Models.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FallbackPathView {
    /// The built-in row "Other accounts with the same model" is on.
    pub other_accounts: bool,
    /// The first choice that the page shows: the model of Tutti. `None`
    /// when no channel has a model yet.
    pub first: Option<FallbackStepView>,
    /// The labels of the other signed-in accounts of the same harness that
    /// have the model of the first choice, for the note of the built-in row.
    pub other_account_labels: Vec<String>,
    /// The steps that the user added, in order.
    pub steps: Vec<FallbackStepView>,
}

impl FallbackPathView {
    /// The step with this account label and model ID, as a number from 1.
    /// The model picker marks such a model with "Step N".
    pub fn step_number(&self, account_label: &str, model_id: &str) -> Option<usize> {
        self.steps
            .iter()
            .position(|s| {
                s.selection.account_label == account_label && s.selection.model_id == model_id
            })
            .map(|i| i + 1)
    }

    /// The step on another harness comes first among the steps. Adding it
    /// needs the confirm dialog when no step is on another harness yet.
    pub fn has_other_harness_step(&self) -> bool {
        self.steps.iter().any(|s| s.other_harness)
    }

    /// The words of the channel row: "Other accounts, then 2 steps".
    pub fn summary(&self) -> String {
        let steps = match self.steps.len() {
            0 => String::new(),
            1 => "1 step".to_owned(),
            n => format!("{n} steps"),
        };
        match (self.other_accounts, steps.is_empty()) {
            (true, true) => "Other accounts".to_owned(),
            (true, false) => format!("Other accounts, then {steps}"),
            (false, true) => "No steps".to_owned(),
            (false, false) => steps,
        }
    }
}

/// The company behind a provider of the model picker, such as "Codex" or
/// "Claude Code". Another provider shows its own name.
pub fn company_of_provider(provider: &str) -> String {
    match provider {
        "Claude Code" => "Anthropic".to_owned(),
        "Codex" => "OpenAI".to_owned(),
        "Gemini CLI" => "Google".to_owned(),
        other => other.to_owned(),
    }
}

/// The harness of the first account that has this label.
pub fn harness_of(cache: &Cache, account_label: &str) -> Option<String> {
    cache
        .workers
        .values()
        .filter_map(|w| w.inventory.as_ref())
        .flat_map(|i| i.accounts.iter())
        .find(|a| a.label == account_label)
        .map(|a| a.harness.clone())
}

/// The effort of a selection in words, such as "High effort".
fn effort_label(selection: &SelectionView) -> Option<String> {
    selection
        .effort()
        .map(|value| format!("{} effort", level_label(value, value)))
}

fn step_view(
    cache: &Cache,
    groups: &[ModelGroupView],
    selection: SelectionView,
    first_harness: Option<&str>,
) -> FallbackStepView {
    let harness = harness_of(cache, &selection.account_label);
    let (provider, logo) = match &harness {
        Some(h) => crate::reduce::provider_logo_of(cache, h),
        None => ("Provider".to_owned(), ProviderLogo::Generic),
    };
    let signed_in = groups
        .iter()
        .find(|g| g.account_label == selection.account_label);
    let skipped = match signed_in {
        Some(group) if group.models.iter().any(|m| m.id == selection.model_id) => None,
        Some(_) => Some(StepSkip::Unavailable),
        None => {
            let known = cache
                .workers
                .values()
                .filter_map(|w| w.inventory.as_ref())
                .flat_map(|i| i.accounts.iter())
                .any(|a| a.label == selection.account_label && a.auth_state != AuthState::SignedIn);
            Some(if known {
                StepSkip::SignedOut
            } else {
                StepSkip::Unavailable
            })
        }
    };
    FallbackStepView {
        effort_label: effort_label(&selection),
        company: harness_company(harness.as_deref().unwrap_or("provider")),
        other_harness: first_harness.is_some_and(|f| Some(f) != harness.as_deref()),
        provider,
        logo,
        skipped,
        selection,
    }
}

/// A step from the picker groups alone, for the mock data. A step whose
/// account is not in `groups` reads as unavailable. The company comes from
/// the logo of the group.
pub fn step_from_groups(
    groups: &[ModelGroupView],
    selection: SelectionView,
    first: Option<&SelectionView>,
) -> FallbackStepView {
    let group = groups
        .iter()
        .find(|g| g.account_label == selection.account_label);
    let first_provider = first.and_then(|f| {
        groups
            .iter()
            .find(|g| g.account_label == f.account_label)
            .map(|g| g.provider.clone())
    });
    let (provider, logo) = group
        .map(|g| (g.provider.clone(), g.logo.clone()))
        .unwrap_or_else(|| ("Provider".to_owned(), ProviderLogo::Generic));
    let skipped = match group {
        Some(g) if g.models.iter().any(|m| m.id == selection.model_id) => None,
        _ => Some(StepSkip::Unavailable),
    };
    FallbackStepView {
        effort_label: effort_label(&selection),
        company: company_of_provider(&provider),
        other_harness: first_provider.is_some_and(|f| f != provider),
        provider,
        logo,
        skipped,
        selection,
    }
}

/// The path for the page. `first` is the model of Tutti.
pub fn path_view(
    cache: &Cache,
    groups: &[ModelGroupView],
    path: &FallbackPath,
    first: Option<&SelectionView>,
) -> FallbackPathView {
    let first_harness = first.and_then(|f| harness_of(cache, &f.account_label));
    let first_step = first
        .cloned()
        .map(|s| step_view(cache, groups, s, first_harness.as_deref()));
    let other_account_labels = first
        .map(|f| {
            groups
                .iter()
                .filter(|g| g.account_label != f.account_label)
                .filter(|g| harness_of(cache, &g.account_label) == first_harness)
                .filter(|g| g.models.iter().any(|m| m.id == f.model_id))
                .map(|g| g.account_label.clone())
                .collect()
        })
        .unwrap_or_default();
    FallbackPathView {
        other_accounts: path.other_accounts,
        first: first_step,
        other_account_labels,
        steps: path
            .steps
            .iter()
            .map(|s| {
                step_view(
                    cache,
                    groups,
                    selection_view(cache, s),
                    first_harness.as_deref(),
                )
            })
            .collect(),
    }
}

/// An assistant that runs on a fallback, for the model chip.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelFallbackView {
    /// The first choice that the assistant goes back to.
    pub from: SelectionView,
    /// What the assistant runs on now.
    pub to: SelectionView,
    pub cause: FallbackCauseView,
    /// When the assistant can go back, in Unix milliseconds.
    pub returns_at: Option<i64>,
    /// "Available at 4:10 PM" before the reset. `None` when the assistant
    /// can go back now, or when the provider gave no time.
    pub available_label: Option<String>,
}

/// What made the path move the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FallbackCauseView {
    UsageLimit,
    Outage,
}

impl From<FallbackCause> for FallbackCauseView {
    fn from(cause: FallbackCause) -> Self {
        match cause {
            FallbackCause::UsageLimit => Self::UsageLimit,
            FallbackCause::Outage => Self::Outage,
        }
    }
}

/// The marker state of a channel chip. `now` is Unix milliseconds.
pub fn channel_fallback_view(
    cache: &Cache,
    fallback: &CoordinatorFallback,
    now: i64,
) -> ChannelFallbackView {
    let returns_at = fallback.returns_at.map(|t| t.as_millis());
    ChannelFallbackView {
        from: selection_view(cache, &fallback.from),
        to: selection_view(cache, &fallback.to),
        cause: fallback.cause.into(),
        returns_at,
        available_label: returns_at
            .filter(|at| *at > now)
            .map(|at| format!("Available at {}", at_label(at, now))),
    }
}

/// "4:10 PM" for today, else "Friday at 9:00 AM".
fn at_label(at: i64, now: i64) -> String {
    if format::day_label(at, now) == "Today" {
        format::time_label(at)
    } else {
        reset_label(at, now)
    }
}

/// What a move notice is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FallbackNoticeKind {
    /// The work moved.
    Moved,
    /// The assistant went back to its first choice.
    Returned,
    /// A move skipped an account that is signed out.
    SignedOut,
}

/// Whose work moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FallbackScopeView {
    Assistant,
    Task,
}

/// One line of the details: what the path tried or skipped.
#[derive(Clone, Debug, PartialEq)]
pub struct PathLineView {
    /// "First choice", "Other accounts with the same model" or "Step 2".
    pub place: String,
    /// The model name, or the model ID when the name is not known.
    pub model: String,
    pub account: String,
    /// "High effort", when the line has an effort.
    pub effort: Option<String>,
    pub outcome: PathOutcomeView,
    /// What happened, such as "Reached its usage limit. Resets at 4:10 PM."
    pub detail: String,
}

/// A filled dot for a failure, a ring for a skip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PathOutcomeView {
    Failed,
    Skipped,
    Chosen,
}

/// The fields of a move notice that the timeline shows.
#[derive(Clone, Debug, PartialEq)]
pub struct FallbackNoticeView {
    pub kind: FallbackNoticeKind,
    /// The sentence to show. For a move of the assistant with a reset time,
    /// it ends with "It returns at 4:10 PM."
    pub text: String,
    pub scope: FallbackScopeView,
    /// "Claude Personal, Opus 5.5".
    pub from: Option<String>,
    pub to: Option<String>,
    /// The company that now gets the work, when it is another harness.
    pub provider: Option<String>,
    /// "It returns at 4:10 PM". `None` for a task, and when the provider
    /// gave no reset time.
    pub returns_label: Option<String>,
    pub returns_at: Option<i64>,
    /// For a signed-out notice: the account to sign in to.
    pub account_label: Option<String>,
    pub lines: Vec<PathLineView>,
    /// The notice offers "Go back now": the assistant still runs on the
    /// fallback of this notice, and the reset has passed. The reducer sets it.
    pub go_back: bool,
}

fn model_words(model: &consort_proto::model::FallbackModel) -> String {
    format!(
        "{}, {}",
        model.account_label,
        model.model_name.as_deref().unwrap_or(&model.model)
    )
}

/// The detail of a path line, in words. `now` is Unix milliseconds. `task`
/// is true for the details of a task, where a chosen step starts a new
/// session.
fn line_detail(line: &PathLine, now: i64, task: bool) -> String {
    let until = line
        .until
        .map(|t| format!(" until {}", at_label(t.as_millis(), now)))
        .unwrap_or_default();
    let provider = line.provider.as_deref().unwrap_or("The provider");
    match (line.outcome, line.reason, line.cause) {
        (PathOutcome::Chosen, ..) if task => {
            "Chosen. This starts a new session in the same folder.".to_owned()
        }
        (PathOutcome::Chosen, ..) => "Chosen.".to_owned(),
        (PathOutcome::Failed, _, Some(FallbackCause::Outage)) => {
            format!("{provider} did not answer after {OUTAGE_TRIES} tries.")
        }
        (PathOutcome::Failed, ..) => format!("At its usage limit{until}."),
        (PathOutcome::Skipped, Some(SkipReason::Limited), Some(FallbackCause::Outage)) => {
            format!("Skipped, {provider} is down or overloaded{until}.")
        }
        (PathOutcome::Skipped, Some(SkipReason::Limited), _) => {
            format!("Skipped, at its usage limit{until}.")
        }
        (PathOutcome::Skipped, Some(SkipReason::SignedOut), _) => {
            "Skipped, the account is signed out.".to_owned()
        }
        (PathOutcome::Skipped, Some(SkipReason::Unavailable), _) => {
            "Skipped, no signed-in account has this model.".to_owned()
        }
        (PathOutcome::Skipped, Some(SkipReason::Mode), _) => {
            "Skipped, the account cannot keep the approval mode.".to_owned()
        }
        (PathOutcome::Skipped, None, _) => "Skipped.".to_owned(),
    }
}

/// The lines of a path report as the views show them.
pub fn path_line_views(lines: &[PathLine], now: i64, task: bool) -> Vec<PathLineView> {
    lines
        .iter()
        .map(|line| PathLineView {
            place: match (line.place, line.step) {
                (PathPlace::First, _) => "First choice".to_owned(),
                (PathPlace::OtherAccount, _) => "Other accounts with the same model".to_owned(),
                (PathPlace::Step, Some(n)) => format!("Step {n}"),
                (PathPlace::Step, None) => "Step".to_owned(),
            },
            model: line
                .model_name
                .clone()
                .unwrap_or_else(|| line.model.clone()),
            account: line.account_label.clone(),
            effort: line
                .effort
                .as_deref()
                .map(|e| format!("{} effort", level_label(e, e))),
            outcome: match line.outcome {
                PathOutcome::Failed => PathOutcomeView::Failed,
                PathOutcome::Skipped => PathOutcomeView::Skipped,
                PathOutcome::Chosen => PathOutcomeView::Chosen,
            },
            detail: line_detail(line, now, task),
        })
        .collect()
}

/// Reads the body of a fallback notice. `None` when the kind is not one of
/// the three or the body is not the JSON of a notice. `now` is Unix
/// milliseconds. The `go_back` flag stays false: the reducer sets it.
pub fn parse_notice(kind: &str, body: &str, now: i64) -> Option<FallbackNoticeView> {
    let kind = match kind {
        MOVED_KIND => FallbackNoticeKind::Moved,
        RETURNED_KIND => FallbackNoticeKind::Returned,
        SIGNED_OUT_KIND => FallbackNoticeKind::SignedOut,
        _ => return None,
    };
    #[derive(Deserialize)]
    struct Parsed {
        #[serde(default)]
        text: String,
        #[serde(flatten)]
        notice: FallbackNotice,
    }
    let Parsed { text, notice } = serde_json::from_str(body).ok()?;
    let returns_at = notice.returns_at.map(|t| t.as_millis());
    let returns_label = returns_at.map(|at| format!("It returns at {}", at_label(at, now)));
    // The hub names no time in its sentence, because the time zone is the
    // client's. With a reset time, the client says when.
    let text = match &returns_label {
        Some(label) => {
            let base = text
                .strip_suffix(" It goes back when the limit resets.")
                .unwrap_or(&text);
            format!("{base} {label}.")
        }
        None => text,
    };
    Some(FallbackNoticeView {
        text,
        kind,
        scope: match notice.scope {
            Some(FallbackScope::Task) => FallbackScopeView::Task,
            _ => FallbackScopeView::Assistant,
        },
        from: notice.from.as_ref().map(model_words),
        to: notice.to.as_ref().map(model_words),
        provider: notice.provider.clone(),
        returns_label,
        returns_at,
        account_label: notice.account_label.clone(),
        lines: path_line_views(
            &notice.lines,
            now,
            notice.scope == Some(FallbackScope::Task),
        ),
        go_back: false,
    })
}

/// The `path` lines of a `turn_failed` notice. Empty when the notice has no
/// path.
pub fn parse_failed_path(body: &str, now: i64) -> Vec<PathLineView> {
    #[derive(Deserialize)]
    struct Parsed {
        #[serde(default)]
        path: Vec<PathLine>,
    }
    serde_json::from_str::<Parsed>(body)
        .map(|p| path_line_views(&p.path, now, false))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::model::AuthState;

    #[test]
    fn the_summary_names_the_built_in_row_and_the_steps() {
        let mut view = FallbackPathView {
            other_accounts: true,
            ..Default::default()
        };
        assert_eq!(view.summary(), "Other accounts");
        view.steps = vec![step("A", "m"), step("A", "n")];
        assert_eq!(view.summary(), "Other accounts, then 2 steps");
        view.other_accounts = false;
        assert_eq!(view.summary(), "2 steps");
        view.steps.truncate(1);
        assert_eq!(view.summary(), "1 step");
        view.steps.clear();
        assert_eq!(view.summary(), "No steps");
    }

    fn step(label: &str, model: &str) -> FallbackStepView {
        FallbackStepView {
            selection: SelectionView {
                account_label: label.into(),
                model_id: model.into(),
                model_name: model.into(),
                options: Vec::new(),
            },
            provider: "Codex".into(),
            logo: ProviderLogo::Codex,
            company: "OpenAI".into(),
            effort_label: None,
            other_harness: false,
            skipped: None,
        }
    }

    #[test]
    fn a_step_number_finds_the_account_and_model() {
        let view = FallbackPathView {
            steps: vec![step("A", "m"), step("B", "n")],
            ..Default::default()
        };
        assert_eq!(view.step_number("B", "n"), Some(2));
        assert_eq!(view.step_number("B", "m"), None);
    }

    #[test]
    fn a_moved_notice_reads_its_fields_and_omits_a_missing_reset() {
        let body = r#"{"kind":"account_fallback","text":"x","scope":"assistant","cause":"usage_limit","from":{"account_label":"Claude Personal","model":"opus","model_name":"Opus 5.5"},"to":{"account_label":"Codex Work","model":"gpt","model_name":"GPT-5.5"},"provider":"OpenAI","lines":[{"place":"first","account_label":"Claude Personal","model":"opus","outcome":"failed","cause":"usage_limit"},{"place":"step","step":1,"account_label":"Claude Work","model":"sonnet","outcome":"skipped","reason":"signed_out"}]}"#;
        let now = 1_900_000_000_000;
        let view = parse_notice(MOVED_KIND, body, now).unwrap();
        assert_eq!(view.kind, FallbackNoticeKind::Moved);
        assert_eq!(view.from.as_deref(), Some("Claude Personal, Opus 5.5"));
        assert_eq!(view.to.as_deref(), Some("Codex Work, GPT-5.5"));
        assert_eq!(view.provider.as_deref(), Some("OpenAI"));
        assert_eq!(view.returns_label, None);
        assert_eq!(view.lines.len(), 2);
        assert_eq!(view.lines[0].place, "First choice");
        assert_eq!(view.lines[0].outcome, PathOutcomeView::Failed);
        assert_eq!(view.lines[1].place, "Step 1");
        assert_eq!(view.lines[1].outcome, PathOutcomeView::Skipped);
        assert_eq!(view.lines[1].detail, "Skipped, the account is signed out.");
        // Another kind is not a fallback notice.
        assert!(parse_notice("turn_failed", body, now).is_none());
        assert!(parse_notice(MOVED_KIND, "not json", now).is_none());
    }

    #[test]
    fn a_reset_time_gives_the_returns_line() {
        let now = 1_900_000_000_000;
        let body = format!(
            r#"{{"kind":"account_fallback","text":"The assistant moved. It goes back when the limit resets.","scope":"assistant","returns_at":{}}}"#,
            now + 3_600_000
        );
        let view = parse_notice(MOVED_KIND, &body, now).unwrap();
        assert!(view.text.starts_with("The assistant moved. It returns at "));
        assert!(!view.text.contains("goes back"));
        assert!(view.returns_label.unwrap().starts_with("It returns at "));
        assert_eq!(view.returns_at, Some(now + 3_600_000));
    }

    #[test]
    fn the_failed_path_of_an_error_card_reads_its_lines() {
        let body = r#"{"kind":"turn_failed","text":"x","path":[{"place":"first","account_label":"A","model":"m","outcome":"failed","cause":"outage","provider":"OpenAI"},{"place":"other_account","account_label":"B","model":"m","outcome":"skipped","reason":"mode"}]}"#;
        let lines = parse_failed_path(body, 0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].detail, "OpenAI did not answer after 3 tries.");
        assert_eq!(lines[1].place, "Other accounts with the same model");
        assert!(
            lines[1]
                .detail
                .starts_with("Skipped, the account cannot keep the approval mode")
        );
        assert!(parse_failed_path(r#"{"kind":"turn_failed"}"#, 0).is_empty());
    }

    fn account(
        label: &str,
        harness: &str,
        models: &[&str],
        signed_in: bool,
    ) -> consort_proto::model::HarnessAccount {
        use consort_proto::model::{Capabilities, HarnessAccount, ModelInfo};
        HarnessAccount {
            blocked_reason: None,
            account_id: consort_proto::ids::AccountId::new(),
            harness: harness.into(),
            label: label.into(),
            harness_version: None,
            auth_state: if signed_in {
                AuthState::SignedIn
            } else {
                AuthState::SignedOut
            },
            auth_detail: None,
            models: models
                .iter()
                .map(|m| ModelInfo {
                    id: (*m).into(),
                    name: (*m).into(),
                    options: Vec::new(),
                })
                .collect(),
            capabilities: Capabilities::default(),
            enabled: true,
            profile: true,
            email: None,
            plan: None,
            secret_names: Vec::new(),
            sign_in_methods: Vec::new(),
        }
    }

    #[test]
    fn the_page_marks_a_signed_out_step_an_other_harness_step_and_the_other_accounts() {
        use consort_proto::model::{
            Inventory, ModelSelection, Os, Worker, WorkerPolicy, WorkerState,
        };
        let mut cache = Cache::default();
        let worker = Worker {
            kind: Default::default(),
            id: consort_proto::ids::WorkerId::new(),
            name: "hub".into(),
            hostname: "hub".into(),
            os: Os::Linux,
            arch: "x64".into(),
            public_key: consort_proto::auth::PublicKey::from_bytes([1; 32]),
            version: "0.1.0".into(),
            protocol_version: 1,
            tags: Vec::new(),
            paused: false,
            state: WorkerState::Online,
            last_seen_at: None,
            inventory: Some(Inventory {
                boundary: None,
                hash: String::new(),
                hostname: "hub".into(),
                os: Os::Linux,
                arch: "x64".into(),
                tags: Vec::new(),
                accounts: vec![
                    account("Claude Personal", "claude", &["opus", "sonnet"], true),
                    account("Claude Work", "claude", &["opus", "sonnet"], true),
                    account("Codex Work", "codex", &["gpt"], true),
                    account("Codex Personal", "codex", &["gpt"], false),
                ],
                local_mcp_servers: Vec::new(),
                checkouts: Vec::new(),
                policy: WorkerPolicy::default(),
                load: Default::default(),
                tools: Vec::new(),
                update: None,
            }),
            paired_at: consort_proto::time::Timestamp::from_millis(0),
            revoked_at: None,
        };
        cache.workers.insert(worker.id.to_string(), worker);
        let groups = crate::picker::model_groups(
            cache.workers.values().filter_map(|w| {
                w.inventory
                    .as_ref()
                    .map(|i| (w.name.as_str(), i.accounts.as_slice()))
            }),
            |h| crate::reduce::provider_logo_of(&cache, h),
        );
        let pick = |label: &str, model: &str| ModelSelection {
            account_label: label.into(),
            model: model.into(),
            options: Default::default(),
        };
        let path = FallbackPath {
            other_accounts: true,
            steps: vec![
                pick("Claude Work", "sonnet"),
                pick("Codex Work", "gpt"),
                pick("Codex Personal", "gpt"),
                pick("Gone", "m"),
            ],
        };
        let first = selection_view(&cache, &pick("Claude Personal", "opus"));
        let view = path_view(&cache, &groups, &path, Some(&first));
        assert_eq!(view.other_account_labels, vec!["Claude Work".to_owned()]);
        let [same, other, signed_out, gone] = &view.steps[..] else {
            panic!("four steps");
        };
        assert!(!same.other_harness && same.skipped.is_none());
        assert!(other.other_harness);
        assert_eq!(other.company, "OpenAI");
        assert_eq!(signed_out.skipped, Some(StepSkip::SignedOut));
        assert_eq!(gone.skipped, Some(StepSkip::Unavailable));
        assert!(view.has_other_harness_step());
        assert_eq!(view.step_number("Codex Work", "gpt"), Some(2));
    }
}
