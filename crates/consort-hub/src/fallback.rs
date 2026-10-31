//! The fallback path: which model answers when the model of a channel
//! cannot. Pure functions with no I/O. The event code, the coordinator code
//! and the task code call [`Walk::run`] with a snapshot of the workers and
//! the holds, and act on the answer.
//!
//! The order of a walk: the first choice (the model of the channel or the
//! task), then the built-in row "Other accounts with the same model", then
//! the steps of the user. A walk skips an entry, and records why, when
//!
//! - no account has the label, or the account does not offer the model,
//! - the account is signed out or its sign-in expired,
//! - the account cannot keep the approval mode (a step never lowers it),
//! - the account is held: it reached a usage limit, or its provider was
//!   down or overloaded for three turns.
//!
//! A worker is never a reason. The accounts of every paired worker count,
//! also when the worker is offline or paused. Placement then looks for a
//! worker that can run the chosen account, as for any other selection.

use std::collections::HashMap;

use consort_proto::model::{
    ApprovalMode, AuthState, FallbackCause, FallbackModel, FallbackNotice, FallbackPath,
    FallbackScope, HarnessAccount, ModelSelection, OUTAGE_TRIES, OptionValue, PathLine,
    PathOutcome, PathPlace, SkipReason, Worker, harness_company,
};
use consort_proto::time::Timestamp;

use crate::store::machine::AccountHold;

/// How long a limit holds an account when the provider gives no reset time.
/// See DECISIONS 753.
pub(crate) const LIMIT_WITHOUT_RESET_MS: i64 = 60 * 60 * 1000;

/// How long a provider outage holds the accounts of the harness. The hub has
/// no time from the provider, so the hold is a fixed time. See DECISIONS 760.
pub(crate) const OUTAGE_HOLD_MS: i64 = 10 * 60 * 1000;

/// A hold on an account label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hold {
    pub until: Timestamp,
    pub cause: FallbackCause,
    /// True when the provider gave `until`.
    pub exact: bool,
}

/// The holds by account label. Two workers with the same label share one
/// login, so a hold on one of them holds the label.
pub(crate) type Holds = HashMap<String, Hold>;

/// Turns the held account IDs into holds by label. An account that no worker
/// has any more gives no hold.
pub(crate) fn holds_by_label(workers: &[Worker], rows: &[AccountHold]) -> Holds {
    let mut holds = Holds::new();
    for row in rows {
        let label = workers
            .iter()
            .filter_map(|w| w.inventory.as_ref())
            .find_map(|inventory| inventory.account(row.account_id))
            .map(|account| account.label.clone());
        let Some(label) = label else { continue };
        let hold = Hold {
            until: row.until,
            cause: row.cause,
            exact: row.exact,
        };
        holds
            .entry(label)
            .and_modify(|old| {
                if hold.until > old.until {
                    *old = hold;
                }
            })
            .or_insert(hold);
    }
    holds
}

/// The accounts of every paired worker, in pairing order.
fn accounts(workers: &[Worker]) -> impl Iterator<Item = &HarnessAccount> {
    workers
        .iter()
        .filter(|w| w.revoked_at.is_none())
        .filter_map(|w| w.inventory.as_ref())
        .flat_map(|inventory| inventory.accounts.iter())
}

/// Whether the sign-in is gone: signed out, or an expired login that the hub
/// saw in a failed turn.
fn signed_out(account: &HarnessAccount) -> bool {
    match account.auth_state {
        AuthState::SignedOut => true,
        AuthState::Error => account
            .auth_detail
            .as_deref()
            .is_some_and(|d| d.to_ascii_lowercase().contains("expired")),
        AuthState::SignedIn | AuthState::Unknown => false,
    }
}

/// Whether the account runs the mode as asked. A step never lowers the
/// approval mode: an account that can only run in a lower mode is skipped.
/// An account with no list of modes did not say, so it is accepted, as in
/// placement.
pub(crate) fn keeps_mode(account: &HarnessAccount, mode: ApprovalMode) -> bool {
    account.capabilities.approval_modes.is_empty()
        || account.capabilities.approval_modes.contains(&mode)
}

/// The harness of the first account with this label, if any worker has one.
pub(crate) fn harness_of(workers: &[Worker], label: &str) -> Option<String> {
    accounts(workers)
        .find(|a| a.label == label)
        .map(|a| a.harness.clone())
}

/// The name to show for a model, from the first account that has it.
pub(crate) fn model_name(workers: &[Worker], label: &str, model: &str) -> Option<String> {
    accounts(workers)
        .filter(|a| a.label == label)
        .flat_map(|a| a.models.iter())
        .find(|m| m.id == model)
        .map(|m| m.name.clone())
}

/// The effort of a selection, as the text of the option `effort`.
pub(crate) fn effort_of(selection: &ModelSelection) -> Option<String> {
    match selection.options.get("effort") {
        Some(OptionValue::Text(value)) => Some(value.clone()),
        _ => None,
    }
}

/// What the check of one account and model found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    /// A signed-in account can run the model in the mode, and is not held.
    Usable,
    /// The entry is skipped.
    Skip(SkipReason, Option<Hold>),
}

/// The result of a walk.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Outcome {
    /// Where the work goes. `None` when no entry can take it.
    pub chosen: Option<Chosen>,
    /// One line for the first choice and for each entry that the walk
    /// reached, up to the chosen one.
    pub lines: Vec<PathLine>,
}

/// The entry that takes the work.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Chosen {
    pub selection: ModelSelection,
    pub place: PathPlace,
    /// The harness of the account.
    pub harness: String,
}

/// A walk over the path.
pub(crate) struct Walk<'a> {
    pub path: &'a FallbackPath,
    pub workers: &'a [Worker],
    pub holds: &'a Holds,
    pub mode: ApprovalMode,
}

impl Walk<'_> {
    /// Checks one account label with one model. The order of the reasons:
    /// no account, signed out, cannot keep the mode, held.
    pub(crate) fn check(&self, label: &str, model: &str) -> Check {
        let enabled: Vec<&HarnessAccount> = accounts(self.workers)
            .filter(|a| a.label == label && a.enabled)
            .collect();
        if enabled.is_empty() {
            return Check::Skip(SkipReason::Unavailable, None);
        }
        let signed_in: Vec<&&HarnessAccount> = enabled.iter().filter(|a| !signed_out(a)).collect();
        if signed_in.is_empty() {
            return Check::Skip(SkipReason::SignedOut, None);
        }
        let with_model: Vec<&&&HarnessAccount> = signed_in
            .iter()
            .filter(|a| a.can_run(model) || a.models.iter().any(|m| m.id == model))
            .collect();
        if with_model.is_empty() {
            return Check::Skip(SkipReason::Unavailable, None);
        }
        if !with_model.iter().any(|a| keeps_mode(a, self.mode)) {
            return Check::Skip(SkipReason::Mode, None);
        }
        if let Some(hold) = self.holds.get(label) {
            return Check::Skip(SkipReason::Limited, Some(*hold));
        }
        Check::Usable
    }

    fn line(
        &self,
        place: PathPlace,
        step: Option<u32>,
        selection: &ModelSelection,
        outcome: PathOutcome,
        check: Check,
    ) -> PathLine {
        let (reason, hold) = match check {
            Check::Skip(reason, hold) => (Some(reason), hold),
            Check::Usable => (None, None),
        };
        let harness = harness_of(self.workers, &selection.account_label);
        PathLine {
            place,
            step,
            account_label: selection.account_label.clone(),
            model: selection.model.clone(),
            model_name: model_name(self.workers, &selection.account_label, &selection.model),
            effort: effort_of(selection),
            outcome,
            cause: hold.map(|h| h.cause),
            reason: if outcome == PathOutcome::Skipped {
                reason
            } else {
                None
            },
            until: hold.filter(|h| h.exact).map(|h| h.until),
            provider: match (outcome, hold.map(|h| h.cause)) {
                (_, Some(FallbackCause::Outage)) => harness.as_deref().map(harness_company),
                _ => None,
            },
        }
    }

    /// Walks the path from the first choice. `trigger` is what just made
    /// the first choice fail, so its line says why even when no hold is
    /// stored yet.
    pub(crate) fn run(&self, first: &ModelSelection, trigger: Option<FallbackCause>) -> Outcome {
        let mut lines = Vec::new();
        let first_hold = self.holds.get(&first.account_label).copied();
        let mut first_line = self.line(
            PathPlace::First,
            None,
            first,
            PathOutcome::Failed,
            first_hold.map_or(Check::Usable, |h| Check::Skip(SkipReason::Limited, Some(h))),
        );
        if first_line.cause.is_none() {
            first_line.cause = trigger;
        }
        lines.push(first_line);

        let first_harness = harness_of(self.workers, &first.account_label);
        let mut tried: Vec<(String, String)> =
            vec![(first.account_label.clone(), first.model.clone())];

        // The built-in row: other accounts of the same harness that have the
        // same model, in the order that the workers report them.
        if self.path.other_accounts
            && let Some(harness) = first_harness
        {
            let mut labels: Vec<String> = Vec::new();
            for account in accounts(self.workers) {
                if account.harness == harness
                    && account.label != first.account_label
                    && account.models.iter().any(|m| m.id == first.model)
                    && !labels.contains(&account.label)
                {
                    labels.push(account.label.clone());
                }
            }
            for label in labels {
                let candidate = ModelSelection {
                    account_label: label,
                    model: first.model.clone(),
                    options: first.options.clone(),
                };
                tried.push((candidate.account_label.clone(), candidate.model.clone()));
                if let Some(outcome) =
                    self.consider(PathPlace::OtherAccount, None, candidate, &mut lines)
                {
                    return outcome;
                }
            }
        }

        for (index, step) in self.path.steps.iter().enumerate() {
            if tried
                .iter()
                .any(|(label, model)| *label == step.account_label && *model == step.model)
            {
                continue;
            }
            let number = u32::try_from(index + 1).ok();
            if let Some(outcome) = self.consider(PathPlace::Step, number, step.clone(), &mut lines)
            {
                return outcome;
            }
        }
        Outcome {
            chosen: None,
            lines,
        }
    }

    /// Checks one entry. Returns the outcome when the entry takes the work.
    /// Otherwise the entry is a skipped line.
    fn consider(
        &self,
        place: PathPlace,
        step: Option<u32>,
        selection: ModelSelection,
        lines: &mut Vec<PathLine>,
    ) -> Option<Outcome> {
        let check = self.check(&selection.account_label, &selection.model);
        match check {
            Check::Usable => {
                lines.push(self.line(place, step, &selection, PathOutcome::Chosen, check));
                let harness = harness_of(self.workers, &selection.account_label)
                    .unwrap_or_else(|| "unknown".to_owned());
                Some(Outcome {
                    chosen: Some(Chosen {
                        selection,
                        place,
                        harness,
                    }),
                    lines: std::mem::take(lines),
                })
            }
            Check::Skip(..) => {
                lines.push(self.line(place, step, &selection, PathOutcome::Skipped, check));
                None
            }
        }
    }

    /// Whether the first choice can answer now: it is usable, in the mode,
    /// and not held. The assistant goes back only then.
    pub(crate) fn first_is_usable(&self, first: &ModelSelection) -> bool {
        self.check(&first.account_label, &first.model) == Check::Usable
    }
}

/// The time when a fallback ends, for the notice: the exact end of the hold
/// on the first choice. `None` when the provider gave no time.
pub(crate) fn returns_at(lines: &[PathLine]) -> Option<Timestamp> {
    lines
        .iter()
        .find(|l| l.place == PathPlace::First)
        .and_then(|l| l.until)
}

/// A move that the walk made.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Moved {
    /// The first choice, where the assistant goes back to.
    pub first: ModelSelection,
    /// The model that the work left: the first choice, or the fallback that
    /// failed after an earlier move.
    pub left: ModelSelection,
    pub chosen: Chosen,
    pub cause: FallbackCause,
    pub lines: Vec<PathLine>,
}

/// The selection that a new session starts with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StartChoice {
    /// What placement gets. The first choice, unless the path moved it.
    pub selection: Option<ModelSelection>,
    /// Set when the first choice is held and the path chose another entry.
    pub moved: Option<Moved>,
}

/// Chooses the selection of a new session. When the first choice is held (a
/// usage limit or an outage that another turn found), the walk runs and its
/// answer replaces the first choice. When nothing in the path can take the
/// work, the first choice stays and the turn shows the error. `first` is
/// `None` when the channel has no choice: the default rule then gives it.
pub(crate) fn choose_start(walk: &Walk<'_>, first: Option<&ModelSelection>) -> StartChoice {
    let resolved = first
        .cloned()
        .or_else(|| crate::placement::default_selection(walk.workers, walk.mode));
    let Some(resolved) = resolved else {
        return StartChoice {
            selection: None,
            moved: None,
        };
    };
    let unchanged = StartChoice {
        selection: first.cloned(),
        moved: None,
    };
    let Some(hold) = walk.holds.get(&resolved.account_label) else {
        return unchanged;
    };
    let outcome = walk.run(&resolved, Some(hold.cause));
    match outcome.chosen {
        Some(chosen) => StartChoice {
            selection: Some(chosen.selection.clone()),
            moved: Some(Moved {
                left: resolved.clone(),
                first: resolved,
                chosen,
                cause: hold.cause,
                lines: outcome.lines,
            }),
        },
        None => unchanged,
    }
}

fn notice_model(workers: &[Worker], selection: &ModelSelection) -> FallbackModel {
    FallbackModel {
        account_label: selection.account_label.clone(),
        model: selection.model.clone(),
        model_name: model_name(workers, &selection.account_label, &selection.model),
    }
}

/// How a model reads in a sentence: "Codex Work, GPT-5.5".
fn model_words(model: &FallbackModel) -> String {
    format!(
        "{}, {}",
        model.account_label,
        model.model_name.as_deref().unwrap_or(&model.model)
    )
}

/// The notice for a move, and the text that older clients show. The text
/// has no time: a client writes "It returns at 4:10 PM" from `returns_at`
/// in the time zone of the user.
pub(crate) fn moved_notice(
    scope: FallbackScope,
    workers: &[Worker],
    moved: &Moved,
) -> (String, FallbackNotice) {
    let from = notice_model(workers, &moved.left);
    let to = notice_model(workers, &moved.chosen.selection);
    let from_harness = harness_of(workers, &moved.left.account_label);
    let provider = (from_harness.as_deref() != Some(moved.chosen.harness.as_str()))
        .then(|| harness_company(&moved.chosen.harness));
    let returns_at = if scope == FallbackScope::Assistant {
        returns_at(&moved.lines)
    } else {
        None
    };
    let why = match moved.cause {
        FallbackCause::UsageLimit => format!("{} reached its usage limit", from.account_label),
        FallbackCause::Outage => format!(
            "{} did not answer after {OUTAGE_TRIES} tries",
            harness_company(from_harness.as_deref().unwrap_or("provider"))
        ),
    };
    let mut text = match scope {
        FallbackScope::Assistant => {
            format!("{why}, so the assistant moved to {}.", model_words(&to))
        }
        FallbackScope::Task => format!(
            "{why}, so this task moved to {}. It stays there until the task ends.",
            model_words(&to)
        ),
    };
    if let Some(provider) = &provider {
        text.push_str(&format!(" The work and its context go to {provider}."));
    }
    if scope == FallbackScope::Assistant {
        text.push_str(if returns_at.is_some() {
            " It goes back when the limit resets."
        } else {
            " It goes back on its own."
        });
    }
    let notice = FallbackNotice {
        scope: Some(scope),
        cause: Some(moved.cause),
        from: Some(from),
        to: Some(to),
        provider,
        returns_at,
        lines: moved.lines.clone(),
        account_label: None,
    };
    (text, notice)
}

/// The notice for an assistant that went back to its first choice.
pub(crate) fn returned_notice(
    workers: &[Worker],
    first: &ModelSelection,
    from: &ModelSelection,
    cause: FallbackCause,
) -> (String, FallbackNotice) {
    let to = notice_model(workers, first);
    let name = to.model_name.clone().unwrap_or_else(|| to.model.clone());
    let text = match cause {
        FallbackCause::UsageLimit => format!(
            "The limit of {} has reset, so the assistant went back to {name}.",
            to.account_label
        ),
        FallbackCause::Outage => format!(
            "{} can answer again, so the assistant went back to {name}.",
            to.account_label
        ),
    };
    let notice = FallbackNotice {
        scope: Some(FallbackScope::Assistant),
        cause: Some(cause),
        from: Some(notice_model(workers, from)),
        to: Some(to),
        ..FallbackNotice::default()
    };
    (text, notice)
}

/// The notices for accounts that a move skipped because they are signed
/// out. One for each account.
pub(crate) fn signed_out_notices(lines: &[PathLine]) -> Vec<(String, FallbackNotice)> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for line in lines {
        if line.reason == Some(SkipReason::SignedOut)
            && !seen.contains(&line.account_label.as_str())
        {
            seen.push(&line.account_label);
            out.push((
                format!(
                    "{} is signed out, so the path skipped it. Sign in to use it again.",
                    line.account_label
                ),
                FallbackNotice {
                    account_label: Some(line.account_label.clone()),
                    lines: vec![line.clone()],
                    ..FallbackNotice::default()
                },
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::auth::PublicKey;
    use consort_proto::ids::{AccountId, WorkerId};
    use consort_proto::model::{
        Capabilities, Inventory, ModelInfo, Os, WorkerLoad, WorkerPolicy, WorkerState,
    };

    fn model(id: &str, name: &str) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            name: name.into(),
            options: Vec::new(),
        }
    }

    pub(crate) fn account(label: &str, harness: &str, models: &[&str]) -> HarnessAccount {
        HarnessAccount {
            blocked_reason: None,
            account_id: AccountId::new(),
            harness: harness.into(),
            label: label.into(),
            harness_version: None,
            auth_state: AuthState::SignedIn,
            auth_detail: None,
            models: models.iter().map(|m| model(m, m)).collect(),
            capabilities: Capabilities::default(),
            enabled: true,
            profile: true,
            email: None,
            plan: None,
            secret_names: Vec::new(),
            sign_in_methods: Vec::new(),
        }
    }

    fn worker(name: &str, accounts: Vec<HarnessAccount>) -> Worker {
        Worker {
            kind: Default::default(),
            id: WorkerId::new(),
            name: name.to_owned(),
            hostname: format!("{name}.local"),
            os: Os::Macos,
            arch: "aarch64".to_owned(),
            public_key: PublicKey::from_bytes([1; 32]),
            version: "0.1.0".to_owned(),
            protocol_version: 1,
            tags: Vec::new(),
            paused: false,
            state: WorkerState::Online,
            last_seen_at: None,
            inventory: Some(Inventory {
                boundary: None,
                hash: String::new(),
                hostname: format!("{name}.local"),
                os: Os::Macos,
                arch: "aarch64".to_owned(),
                tags: Vec::new(),
                accounts,
                local_mcp_servers: Vec::new(),
                checkouts: Vec::new(),
                policy: WorkerPolicy::default(),
                load: WorkerLoad::default(),
                tools: Vec::new(),
            }),
            paired_at: Timestamp::from_millis(0),
            revoked_at: None,
        }
    }

    fn sel(label: &str, model: &str) -> ModelSelection {
        ModelSelection {
            account_label: label.into(),
            model: model.into(),
            options: Default::default(),
        }
    }

    fn hold(until: i64, cause: FallbackCause, exact: bool) -> Hold {
        Hold {
            until: Timestamp::from_millis(until),
            cause,
            exact,
        }
    }

    /// The accounts of the design: two Claude accounts, a signed-out Codex
    /// account and a Codex account.
    fn fleet() -> Vec<Worker> {
        let mut codex_personal = account("Codex Personal", "codex", &["gpt-5.5-mini"]);
        codex_personal.auth_state = AuthState::SignedOut;
        vec![worker(
            "hub",
            vec![
                account("Claude Personal", "claude", &["opus", "sonnet"]),
                account("Claude Work", "claude", &["opus", "sonnet"]),
                account("Codex Work", "codex", &["gpt-5.5", "gpt-5.5-mini"]),
                codex_personal,
            ],
        )]
    }

    fn path(other: bool, steps: &[(&str, &str)]) -> FallbackPath {
        FallbackPath {
            other_accounts: other,
            steps: steps.iter().map(|(l, m)| sel(l, m)).collect(),
        }
    }

    fn walk<'a>(
        path: &'a FallbackPath,
        workers: &'a [Worker],
        holds: &'a Holds,
        mode: ApprovalMode,
    ) -> Walk<'a> {
        Walk {
            path,
            workers,
            holds,
            mode,
        }
    }

    fn chosen_label(outcome: &Outcome) -> Option<&str> {
        outcome
            .chosen
            .as_ref()
            .map(|c| c.selection.account_label.as_str())
    }

    #[test]
    fn the_built_in_row_comes_before_the_steps() {
        let workers = fleet();
        let path = path(true, &[("Codex Work", "gpt-5.5")]);
        let holds: Holds = [(
            "Claude Personal".to_owned(),
            hold(9, FallbackCause::UsageLimit, true),
        )]
        .into();
        let out = walk(&path, &workers, &holds, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        assert_eq!(chosen_label(&out), Some("Claude Work"));
        assert_eq!(out.chosen.as_ref().unwrap().place, PathPlace::OtherAccount);
        // The first choice and the chosen entry are the only lines.
        assert_eq!(out.lines.len(), 2);
        assert_eq!(out.lines[0].place, PathPlace::First);
        assert_eq!(out.lines[0].outcome, PathOutcome::Failed);
        assert_eq!(out.lines[1].outcome, PathOutcome::Chosen);
    }

    #[test]
    fn with_the_built_in_row_off_the_first_step_is_next() {
        let workers = fleet();
        let path = path(false, &[("Codex Work", "gpt-5.5")]);
        let holds: Holds = [(
            "Claude Personal".to_owned(),
            hold(9, FallbackCause::UsageLimit, true),
        )]
        .into();
        let out = walk(&path, &workers, &holds, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        assert_eq!(chosen_label(&out), Some("Codex Work"));
        assert_eq!(out.chosen.as_ref().unwrap().place, PathPlace::Step);
        assert_eq!(out.lines[1].step, Some(1));
        assert_eq!(out.chosen.as_ref().unwrap().harness, "codex");
    }

    #[test]
    fn the_steps_run_in_their_order_and_a_held_one_is_skipped_with_its_reason() {
        let workers = fleet();
        let path = path(
            false,
            &[
                ("Claude Work", "sonnet"),
                ("Codex Work", "gpt-5.5"),
                ("Codex Personal", "gpt-5.5-mini"),
            ],
        );
        let holds: Holds = [
            (
                "Claude Personal".to_owned(),
                hold(5, FallbackCause::UsageLimit, true),
            ),
            (
                "Claude Work".to_owned(),
                hold(7, FallbackCause::UsageLimit, true),
            ),
        ]
        .into();
        let out = walk(&path, &workers, &holds, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        assert_eq!(chosen_label(&out), Some("Codex Work"));
        let skipped = &out.lines[1];
        assert_eq!(skipped.outcome, PathOutcome::Skipped);
        assert_eq!(skipped.reason, Some(SkipReason::Limited));
        assert_eq!(skipped.cause, Some(FallbackCause::UsageLimit));
        assert_eq!(skipped.until, Some(Timestamp::from_millis(7)));
        assert_eq!(skipped.step, Some(1));
    }

    #[test]
    fn a_signed_out_account_is_skipped_and_the_walk_goes_on() {
        let workers = fleet();
        let path = path(
            false,
            &[
                ("Codex Personal", "gpt-5.5-mini"),
                ("Codex Work", "gpt-5.5"),
            ],
        );
        let holds: Holds = [(
            "Claude Personal".to_owned(),
            hold(5, FallbackCause::UsageLimit, true),
        )]
        .into();
        let out = walk(&path, &workers, &holds, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        assert_eq!(chosen_label(&out), Some("Codex Work"));
        assert_eq!(out.lines[1].reason, Some(SkipReason::SignedOut));
        assert_eq!(out.lines[1].account_label, "Codex Personal");
    }

    #[test]
    fn an_expired_login_counts_as_signed_out() {
        let mut workers = fleet();
        let inventory = workers[0].inventory.as_mut().unwrap();
        inventory.accounts[2].auth_state = AuthState::Error;
        inventory.accounts[2].auth_detail = Some("The login expired. Sign in again.".into());
        let path = path(false, &[("Codex Work", "gpt-5.5")]);
        let holds = Holds::new();
        let w = walk(&path, &workers, &holds, ApprovalMode::Auto);
        assert_eq!(
            w.check("Codex Work", "gpt-5.5"),
            Check::Skip(SkipReason::SignedOut, None)
        );
    }

    #[test]
    fn a_removed_account_or_a_missing_model_is_unavailable() {
        let workers = fleet();
        let path = path(false, &[]);
        let holds = Holds::new();
        let w = walk(&path, &workers, &holds, ApprovalMode::Auto);
        assert_eq!(
            w.check("Gone", "opus"),
            Check::Skip(SkipReason::Unavailable, None)
        );
        assert_eq!(
            w.check("Claude Work", "haiku"),
            Check::Skip(SkipReason::Unavailable, None)
        );
        let mut disabled = fleet();
        disabled[0].inventory.as_mut().unwrap().accounts[1].enabled = false;
        let w = walk(&path, &disabled, &holds, ApprovalMode::Auto);
        assert_eq!(
            w.check("Claude Work", "opus"),
            Check::Skip(SkipReason::Unavailable, None)
        );
    }

    /// A step never lowers the approval mode: an account that can only run
    /// in a lower mode is skipped, and the walk goes on.
    #[test]
    fn a_step_that_would_lower_the_approval_mode_is_skipped() {
        let mut workers = fleet();
        let inventory = workers[0].inventory.as_mut().unwrap();
        inventory.accounts[2].capabilities.approval_modes =
            vec![ApprovalMode::Ask, ApprovalMode::AutoEdits];
        inventory.accounts[1].capabilities.approval_modes =
            vec![ApprovalMode::Ask, ApprovalMode::Auto];
        let path = path(
            false,
            &[("Codex Work", "gpt-5.5"), ("Claude Work", "sonnet")],
        );
        let holds: Holds = [(
            "Claude Personal".to_owned(),
            hold(5, FallbackCause::UsageLimit, true),
        )]
        .into();
        let out = walk(&path, &workers, &holds, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        assert_eq!(out.lines[1].account_label, "Codex Work");
        assert_eq!(out.lines[1].reason, Some(SkipReason::Mode));
        assert_eq!(chosen_label(&out), Some("Claude Work"));
        // In Ask mode, the same account is fine.
        let out = walk(&path, &workers, &holds, ApprovalMode::Ask)
            .run(&sel("Claude Personal", "opus"), None);
        assert_eq!(chosen_label(&out), Some("Codex Work"));
    }

    #[test]
    fn the_built_in_row_skips_an_account_that_cannot_keep_the_mode() {
        let mut workers = fleet();
        workers[0].inventory.as_mut().unwrap().accounts[1]
            .capabilities
            .approval_modes = vec![ApprovalMode::Ask];
        let path = path(true, &[]);
        let holds: Holds = [(
            "Claude Personal".to_owned(),
            hold(5, FallbackCause::UsageLimit, true),
        )]
        .into();
        let out = walk(&path, &workers, &holds, ApprovalMode::FullAccess)
            .run(&sel("Claude Personal", "opus"), None);
        assert!(out.chosen.is_none());
        assert_eq!(out.lines[1].place, PathPlace::OtherAccount);
        assert_eq!(out.lines[1].reason, Some(SkipReason::Mode));
    }

    /// A worker is never a reason: an offline, paused or empty worker does
    /// not change the walk.
    #[test]
    fn a_worker_is_never_a_reason_to_skip() {
        let mut workers = fleet();
        workers[0].state = WorkerState::Offline;
        workers[0].paused = true;
        let path = path(false, &[("Codex Work", "gpt-5.5")]);
        let holds: Holds = [(
            "Claude Personal".to_owned(),
            hold(5, FallbackCause::UsageLimit, true),
        )]
        .into();
        let out = walk(&path, &workers, &holds, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        assert_eq!(chosen_label(&out), Some("Codex Work"));
        // The account is on another worker than the first choice.
        let mut split = fleet();
        let codex: Vec<_> = split[0].inventory.as_mut().unwrap().accounts.split_off(2);
        let mut other = worker("bigboi", codex);
        other.id = WorkerId::new();
        split.push(other);
        let out = walk(&path, &split, &holds, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        assert_eq!(chosen_label(&out), Some("Codex Work"));
    }

    #[test]
    fn nothing_usable_ends_the_walk_with_a_line_for_each_entry() {
        let workers = fleet();
        let path = path(
            true,
            &[
                ("Codex Work", "gpt-5.5"),
                ("Codex Personal", "gpt-5.5-mini"),
            ],
        );
        let holds: Holds = [
            (
                "Claude Personal".to_owned(),
                hold(5, FallbackCause::UsageLimit, true),
            ),
            (
                "Claude Work".to_owned(),
                hold(7, FallbackCause::UsageLimit, true),
            ),
            (
                "Codex Work".to_owned(),
                hold(9, FallbackCause::Outage, false),
            ),
        ]
        .into();
        let out = walk(&path, &workers, &holds, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        assert!(out.chosen.is_none());
        let places: Vec<_> = out.lines.iter().map(|l| l.place).collect();
        assert_eq!(
            places,
            [
                PathPlace::First,
                PathPlace::OtherAccount,
                PathPlace::Step,
                PathPlace::Step
            ]
        );
        assert_eq!(out.lines[2].cause, Some(FallbackCause::Outage));
        assert_eq!(out.lines[2].until, None, "an outage hold has no exact time");
        assert_eq!(out.lines[2].provider.as_deref(), Some("OpenAI"));
        assert_eq!(out.lines[3].reason, Some(SkipReason::SignedOut));
    }

    #[test]
    fn a_step_that_repeats_the_first_choice_or_the_built_in_row_is_left_out() {
        let workers = fleet();
        let path = path(
            true,
            &[
                ("Claude Personal", "opus"),
                ("Claude Work", "opus"),
                ("Codex Work", "gpt-5.5"),
            ],
        );
        let holds: Holds = [
            (
                "Claude Personal".to_owned(),
                hold(5, FallbackCause::UsageLimit, true),
            ),
            (
                "Claude Work".to_owned(),
                hold(7, FallbackCause::UsageLimit, true),
            ),
        ]
        .into();
        let out = walk(&path, &workers, &holds, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        let labels: Vec<_> = out.lines.iter().map(|l| l.account_label.as_str()).collect();
        assert_eq!(labels, ["Claude Personal", "Claude Work", "Codex Work"]);
        // The step number is its place in the stored list.
        assert_eq!(out.lines[2].step, Some(3));
    }

    #[test]
    fn the_first_line_has_the_trigger_when_no_hold_is_stored() {
        let workers = fleet();
        let path = path(false, &[("Codex Work", "gpt-5.5")]);
        let holds = Holds::new();
        let out = walk(&path, &workers, &holds, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), Some(FallbackCause::Outage));
        assert_eq!(out.lines[0].cause, Some(FallbackCause::Outage));
        assert_eq!(returns_at(&out.lines), None);
    }

    #[test]
    fn returns_at_is_the_exact_end_of_the_hold_on_the_first_choice() {
        let workers = fleet();
        let path = path(false, &[("Codex Work", "gpt-5.5")]);
        let exact: Holds = [(
            "Claude Personal".to_owned(),
            hold(5_000, FallbackCause::UsageLimit, true),
        )]
        .into();
        let out = walk(&path, &workers, &exact, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        assert_eq!(returns_at(&out.lines), Some(Timestamp::from_millis(5_000)));
        // The provider gave no time: the notice says nothing about it.
        let guess: Holds = [(
            "Claude Personal".to_owned(),
            hold(5_000, FallbackCause::UsageLimit, false),
        )]
        .into();
        let out = walk(&path, &workers, &guess, ApprovalMode::Auto)
            .run(&sel("Claude Personal", "opus"), None);
        assert_eq!(returns_at(&out.lines), None);
    }

    #[test]
    fn holds_are_by_label_and_the_later_end_wins() {
        let workers = fleet();
        let id = workers[0].inventory.as_ref().unwrap().accounts[0].account_id;
        let rows = [
            AccountHold {
                account_id: id,
                until: Timestamp::from_millis(10),
                cause: FallbackCause::UsageLimit,
                exact: true,
            },
            AccountHold {
                account_id: AccountId::new(),
                until: Timestamp::from_millis(99),
                cause: FallbackCause::UsageLimit,
                exact: true,
            },
        ];
        let holds = holds_by_label(&workers, &rows);
        assert_eq!(
            holds.len(),
            1,
            "an account that no worker has gives no hold"
        );
        assert_eq!(holds["Claude Personal"].until, Timestamp::from_millis(10));
    }

    #[test]
    fn effort_and_names_come_from_the_selection_and_the_inventory() {
        let workers = fleet();
        let mut selection = sel("Claude Work", "opus");
        selection
            .options
            .insert("effort".into(), OptionValue::Text("high".into()));
        assert_eq!(effort_of(&selection).as_deref(), Some("high"));
        assert_eq!(
            model_name(&workers, "Claude Work", "opus").as_deref(),
            Some("opus")
        );
        assert_eq!(model_name(&workers, "Gone", "opus"), None);
    }
}
