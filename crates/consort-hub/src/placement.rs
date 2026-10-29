//! Placement: pure functions.
//!
//! Placement turns a model selection and a placement request into one worker
//! and one harness account. It does no I/O and reads no store. The task code
//! and the coordinator code call [`place`] with a snapshot of the workers.
//!
//! The steps, in order:
//!
//! 1. Ignore revoked workers.
//! 2. A request that names a worker uses only that worker. The result is a
//!    placement, a wait, or a failure that says what to change.
//! 3. Otherwise the candidates are the online workers that are not paused.
//!    When the channel has tags, only workers that have every tag count.
//! 4. Each candidate must pass three checks: a usable harness account, the
//!    `require` constraints (with the channel tags added), and the worker
//!    policy.
//! 5. The remaining candidates are ranked. The first one wins.
//!
//! When no candidate remains, the result is a wait. The reason names the
//! first check that removed the last candidate.

use std::cmp::Reverse;
use std::collections::HashMap;

use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::ids::{AccountId, RepoId, WorkerId};
use consort_proto::model::{
    ApprovalMode, AuthState, HarnessAccount, Inventory, Isolation, ModelOptions, ModelSelection,
    Os, PlacementRequest, PlacementRequire, Repo, Worker, WorkerState,
};

use crate::store::conversation::repo_url_key;

/// What the placement is for. It changes the ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Subject {
    /// A coordinator session for a channel.
    Coordinator,
    /// A task session.
    Task,
}

/// Everything that placement reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlacementInput<'a> {
    pub subject: Subject,
    /// Every worker the hub knows, revoked ones included, in pairing order.
    pub workers: &'a [Worker],
    /// Open sessions by worker, counted by the hub.
    pub open_sessions: &'a HashMap<WorkerId, u32>,
    /// The model selection that the channel or the task chose. `None` means
    /// the default rule: the best harness account and its first model.
    pub selection: Option<&'a ModelSelection>,
    pub request: &'a PlacementRequest,
    pub approval_mode: ApprovalMode,
    /// The channel's preferred workers, in order.
    pub preferred_workers: &'a [WorkerId],
    /// The tags of the channel. A worker qualifies only if it has every one
    /// of them, on top of any `require.tags`.
    pub channel_tags: &'a [String],
    /// The repos of the session (already defaulted from the channel). Empty
    /// for coordinators.
    pub repos: &'a [Repo],
    pub isolation: Isolation,
}

/// One repo of the session and where it lives on the chosen worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoCheckout {
    pub repo_id: RepoId,
    pub url: String,
    /// The checkout path on the chosen worker, if the worker has one.
    pub checkout_path: Option<String>,
}

/// The chosen worker, account and model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placement {
    pub worker_id: WorkerId,
    pub worker_name: String,
    pub account_id: AccountId,
    pub account_label: String,
    pub harness: String,
    pub model: String,
    pub options: ModelOptions,
    /// One entry per input repo, with the checkout path on the chosen worker
    /// if it has one.
    pub checkouts: Vec<RepoCheckout>,
}

/// Why placement did not give a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlacementError {
    /// Nothing can start now. The task stays in Up next with this reason.
    /// Placement runs again when a worker connects or changes.
    Wait(String),
    /// The request can never be met as written. The task fails with this
    /// error.
    Fail(ErrorBody),
}

impl PlacementError {
    /// The human message: the wait reason or the error message.
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::Wait(reason) => reason.clone(),
            Self::Fail(body) => body.message.clone(),
        }
    }
}

/// Picks the worker and the harness account.
pub(crate) fn place(input: &PlacementInput<'_>) -> Result<Placement, PlacementError> {
    let live: Vec<(usize, &Worker)> = input
        .workers
        .iter()
        .enumerate()
        .filter(|(_, worker)| worker.revoked_at.is_none())
        .collect();
    match input.request.worker_id {
        Some(id) => place_on_named_worker(input, &live, id),
        None => place_on_best_worker(input, &live),
    }
}

// Named worker ---------------------------------------------------------------

fn place_on_named_worker(
    input: &PlacementInput<'_>,
    live: &[(usize, &Worker)],
    id: WorkerId,
) -> Result<Placement, PlacementError> {
    let Some(&(index, worker)) = live.iter().find(|(_, worker)| worker.id == id) else {
        return Err(PlacementError::Fail(ErrorBody::new(
            ErrorCode::WorkerUnknown,
            format!(
                "The worker {id} is not known or the user removed it. \
                 Choose another worker, or leave the worker empty."
            ),
        )));
    };
    let name = &worker.name;
    if worker.paused {
        return Err(PlacementError::Wait(format!("{name} is paused.")));
    }
    if worker.state != WorkerState::Online {
        return Err(PlacementError::Wait(format!(
            "Waiting for {name} to come online."
        )));
    }
    let Some(inventory) = &worker.inventory else {
        return Err(PlacementError::Wait(format!(
            "Waiting for {name} to send its inventory."
        )));
    };
    match evaluate(input, index, worker, inventory) {
        Ok(candidate) => Ok(candidate.into_placement()),
        Err(rejection) => Err(PlacementError::Fail(
            rejection.named_worker_error(input, worker, inventory),
        )),
    }
}

// Any worker -----------------------------------------------------------------

fn place_on_best_worker(
    input: &PlacementInput<'_>,
    live: &[(usize, &Worker)],
) -> Result<Placement, PlacementError> {
    if let Some(reason) = channel_tag_wait(input, live) {
        return Err(PlacementError::Wait(reason));
    }
    let online: Vec<&(usize, &Worker)> = live
        .iter()
        .filter(|(_, worker)| worker.state == WorkerState::Online)
        .collect();
    let ready: Vec<&(usize, &Worker)> = online
        .iter()
        .copied()
        .filter(|(_, worker)| !worker.paused)
        .collect();
    if ready.is_empty() {
        let all_paused = !live.is_empty() && live.iter().all(|(_, worker)| worker.paused);
        return Err(PlacementError::Wait(
            if all_paused {
                "All workers are paused."
            } else {
                "Waiting for a worker to come online."
            }
            .to_owned(),
        ));
    }

    let mut candidates = Vec::new();
    let mut rejections = Vec::new();
    for &&(index, worker) in &ready {
        // A worker that has not sent its inventory cannot be used yet.
        let Some(inventory) = &worker.inventory else {
            continue;
        };
        match evaluate(input, index, worker, inventory) {
            Ok(candidate) => candidates.push(candidate),
            Err(rejection) => rejections.push(rejection),
        }
    }

    let best = match input.subject {
        Subject::Coordinator => candidates.into_iter().min_by_key(|c| {
            (
                !c.worker.all_tags().contains(&"hub"),
                preferred_rank(input, c.worker.id),
                session_count(input, c),
                c.index,
            )
        }),
        Subject::Task => candidates.into_iter().min_by_key(|c| {
            (
                Reverse(c.checkout_count()),
                preferred_rank(input, c.worker.id),
                c.inventory.policy.max_approval_mode < input.approval_mode,
                session_count(input, c),
                c.index,
            )
        }),
    };
    match best {
        Some(candidate) => Ok(candidate.into_placement()),
        None => Err(PlacementError::Wait(wait_reason(input, &rejections))),
    }
}

/// The position in the channel's preferred workers. Unlisted workers rank
/// after listed ones.
fn preferred_rank(input: &PlacementInput<'_>, id: WorkerId) -> usize {
    input
        .preferred_workers
        .iter()
        .position(|preferred| *preferred == id)
        .unwrap_or(usize::MAX)
}

/// The larger of the hub's count and the worker's own count.
fn session_count(input: &PlacementInput<'_>, candidate: &Candidate<'_>) -> u32 {
    input
        .open_sessions
        .get(&candidate.worker.id)
        .copied()
        .unwrap_or(0)
        .max(candidate.inventory.load.active_sessions)
}

// Checks ---------------------------------------------------------------------

/// A worker that passed every check.
struct Candidate<'a> {
    /// The place in pairing order.
    index: usize,
    worker: &'a Worker,
    inventory: &'a Inventory,
    account: &'a HarnessAccount,
    model: String,
    options: ModelOptions,
    checkouts: Vec<RepoCheckout>,
}

impl Candidate<'_> {
    fn checkout_count(&self) -> usize {
        self.checkouts
            .iter()
            .filter(|checkout| checkout.checkout_path.is_some())
            .count()
    }

    fn into_placement(self) -> Placement {
        Placement {
            worker_id: self.worker.id,
            worker_name: self.worker.name.clone(),
            account_id: self.account.account_id,
            account_label: self.account.label.clone(),
            harness: self.account.harness.clone(),
            model: self.model,
            options: self.options,
            checkouts: self.checkouts,
        }
    }
}

/// Why a worker did not pass. The variants are in the order of the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rejection {
    /// No usable harness account.
    Account(AccountProblem),
    /// The `require` constraints do not hold.
    Require,
    /// The worker policy does not allow an in-place run in this folder.
    Policy { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AccountProblem {
    /// No enabled, signed-in account matches.
    Missing,
    /// An account matches, but it cannot enforce an approval mode at or
    /// below the requested one.
    Mode { label: String },
}

/// Runs the three checks on one worker, in order.
fn evaluate<'a>(
    input: &PlacementInput<'_>,
    index: usize,
    worker: &'a Worker,
    inventory: &'a Inventory,
) -> Result<Candidate<'a>, Rejection> {
    let (account, model, options) = choose_account(inventory, input.selection, input.approval_mode)
        .map_err(Rejection::Account)?;
    if let Some(require) = &effective_require(input)
        && !meets_require(require, worker, inventory)
    {
        return Err(Rejection::Require);
    }
    let checkouts = match_checkouts(worker, inventory, input.repos, input.isolation)
        .map_err(|path| Rejection::Policy { path })?;
    Ok(Candidate {
        index,
        worker,
        inventory,
        account,
        model,
        options,
        checkouts,
    })
}

/// Chooses the account, the model and the model options on one worker.
fn choose_account<'a>(
    inventory: &'a Inventory,
    selection: Option<&ModelSelection>,
    approval_mode: ApprovalMode,
) -> Result<(&'a HarnessAccount, String, ModelOptions), AccountProblem> {
    match selection {
        Some(selection) => {
            // The same test as `Inventory::account_for`, but it goes on to
            // the next account of that label when the first cannot enforce
            // the mode.
            let mut matching = inventory
                .accounts
                .iter()
                .filter(|a| a.label == selection.account_label && a.can_run(&selection.model))
                .peekable();
            let first_label = matching.peek().map(|a| a.label.clone());
            match matching.find(|a| enforces_mode(a, approval_mode)) {
                Some(account) => Ok((account, selection.model.clone(), selection.options.clone())),
                None => {
                    Err(
                        first_label.map_or(AccountProblem::Missing, |label| AccountProblem::Mode {
                            label,
                        }),
                    )
                }
            }
        }
        None => {
            let usable: Vec<&HarnessAccount> = inventory
                .accounts
                .iter()
                .filter(|a| {
                    a.enabled && a.auth_state == AuthState::SignedIn && !a.models.is_empty()
                })
                .collect();
            let Some(first) = usable.first() else {
                return Err(AccountProblem::Missing);
            };
            // `min_by_key` keeps the first of equal ranks, so a tie goes to
            // the account that comes first in the inventory.
            let best = usable
                .iter()
                .copied()
                .filter(|a| enforces_mode(a, approval_mode))
                .min_by_key(|a| harness_rank(a));
            match best {
                Some(account) => match account.models.first() {
                    Some(model) => Ok((account, model.id.clone(), ModelOptions::new())),
                    None => Err(AccountProblem::Missing),
                },
                None => Err(AccountProblem::Mode {
                    label: first.label.clone(),
                }),
            }
        }
    }
}

/// Whether the account can enforce a mode that is at most the requested one.
/// An empty list means the worker did not say, so the account is accepted.
fn enforces_mode(account: &HarnessAccount, requested: ApprovalMode) -> bool {
    account.capabilities.approval_modes.is_empty()
        || account.capabilities.best_mode_up_to(requested).is_some()
}

/// Labels in the order of the default choice. Rank 0 is the best.
const LABEL_ORDER: [&str; 5] = ["Claude Code", "Codex", "OpenCode", "Gemini CLI", "Pi"];

/// The rank of an account for the default choice. A lower rank wins.
fn harness_rank(account: &HarnessAccount) -> usize {
    let label = account.label.trim();
    if let Some(rank) = LABEL_ORDER
        .iter()
        .position(|known| known.eq_ignore_ascii_case(label))
    {
        return rank;
    }
    let harness = account.harness.trim().to_ascii_lowercase();
    if harness.starts_with("claude") {
        0
    } else if harness == "codex" {
        1
    } else if harness == "opencode" {
        2
    } else if harness.contains("gemini") {
        3
    } else if harness == "pi" {
        4
    } else {
        LABEL_ORDER.len()
    }
}

/// The constraints of the request with the channel tags added. `None` when
/// the request has none and the channel has no tags.
fn effective_require(input: &PlacementInput<'_>) -> Option<PlacementRequire> {
    if input.channel_tags.is_empty() {
        return input.request.require.clone();
    }
    let mut require = input.request.require.clone().unwrap_or_default();
    for tag in input.channel_tags {
        if !require.tags.contains(tag) {
            require.tags.push(tag.clone());
        }
    }
    Some(require)
}

/// The reason to wait when the channel has tags and no online worker that
/// is not paused has every one of them. `None` when such a worker exists.
///
/// If no worker at all has the tags, the reason says so and points to the
/// place where the user adds a tag to a worker.
fn channel_tag_wait(input: &PlacementInput<'_>, live: &[(usize, &Worker)]) -> Option<String> {
    if input.channel_tags.is_empty() {
        return None;
    }
    let tagged: Vec<&Worker> = live
        .iter()
        .map(|(_, worker)| *worker)
        .filter(|worker| worker.has_tags(input.channel_tags))
        .collect();
    if tagged
        .iter()
        .any(|worker| worker.state == WorkerState::Online && !worker.paused)
    {
        return None;
    }
    let noun = if input.channel_tags.len() == 1 {
        "tag"
    } else {
        "tags"
    };
    let wanted = join_list(input.channel_tags);
    Some(if tagged.is_empty() {
        format!(
            "Waiting for an online worker with the {noun} {wanted}. \
             Add the {noun} to a worker in Settings > Workers."
        )
    } else if tagged.iter().all(|worker| worker.paused) {
        format!(
            "Waiting for an online worker with the {noun} {wanted}. \
             Every worker with the {noun} is paused."
        )
    } else {
        format!("Waiting for an online worker with the {noun} {wanted}.")
    })
}

fn meets_require(require: &PlacementRequire, worker: &Worker, inventory: &Inventory) -> bool {
    require.is_satisfied_by(
        worker.os,
        worker.all_tags(),
        inventory
            .local_mcp_servers
            .iter()
            .map(|server| server.name.as_str()),
    )
}

/// Matches each repo to a checkout on the worker and applies the policy.
///
/// An in-place run is only allowed in the worker's `allowed_roots`, when the
/// worker lists any. If a repo has several checkouts, the first one inside
/// an allowed root wins. If a repo has checkouts and none is inside a root,
/// the error holds the path of the first one.
fn match_checkouts(
    worker: &Worker,
    inventory: &Inventory,
    repos: &[Repo],
    isolation: Isolation,
) -> Result<Vec<RepoCheckout>, String> {
    let roots = &inventory.policy.allowed_roots;
    let restricted = isolation == Isolation::InPlace && !roots.is_empty();
    let ignore_case = worker.os == Os::Windows;
    let keyed: Vec<(String, &str)> = inventory
        .checkouts
        .iter()
        .map(|checkout| (repo_url_key(&checkout.repo_url), checkout.path.as_str()))
        .collect();

    let mut result = Vec::with_capacity(repos.len());
    for repo in repos {
        let key = repo_url_key(&repo.url);
        let mut matching = keyed
            .iter()
            .filter(|(checkout_key, _)| *checkout_key == key)
            .map(|(_, path)| *path)
            .peekable();
        let first = matching.peek().copied();
        let path = if restricted {
            match first {
                None => None,
                Some(first) => Some(
                    matching
                        .find(|path| roots.iter().any(|root| is_under(path, root, ignore_case)))
                        .ok_or_else(|| first.to_owned())?,
                ),
            }
        } else {
            first
        };
        result.push(RepoCheckout {
            repo_id: repo.id,
            url: repo.url.clone(),
            checkout_path: path.map(str::to_owned),
        });
    }
    Ok(result)
}

/// Whether `path` is `root` or a folder below it. Both are compared as
/// normalized segments: `\` counts as `/`, `.` and `..` segments are
/// resolved, and case is ignored when `ignore_case` is set (Windows).
/// The comparison is by whole segments, so `/code2` is not under `/code`.
fn is_under(path: &str, root: &str, ignore_case: bool) -> bool {
    let (path_prefix, path_segments) = normalize_path(path, ignore_case);
    let (root_prefix, root_segments) = normalize_path(root, ignore_case);
    path_prefix == root_prefix
        && path_segments.len() >= root_segments.len()
        && path_segments[..root_segments.len()] == root_segments[..]
}

/// Returns the count of leading slashes (0, 1 or 2) and the path segments.
/// Two leading slashes mark a Windows network path.
fn normalize_path(path: &str, ignore_case: bool) -> (usize, Vec<String>) {
    let unified = path.replace('\\', "/");
    let prefix = unified.chars().take_while(|c| *c == '/').count().min(2);
    let mut segments: Vec<String> = Vec::new();
    for segment in unified.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|last| last != "..") {
                    segments.pop();
                } else if prefix == 0 {
                    segments.push("..".to_owned());
                }
            }
            other if ignore_case => segments.push(other.to_lowercase()),
            other => segments.push(other.to_owned()),
        }
    }
    (prefix, segments)
}

// Messages -------------------------------------------------------------------

/// The reason to wait, when no online worker passed the checks. It names the
/// check that removed the last worker.
fn wait_reason(input: &PlacementInput<'_>, rejections: &[Rejection]) -> String {
    let Some(last) = rejections.iter().map(Rejection::stage).max() else {
        // Every ready worker lacks an inventory.
        return "Waiting for the online workers to send their inventory.".to_owned();
    };
    match last {
        Stage::Account => {
            let mode_problem = rejections
                .iter()
                .any(|r| matches!(r, Rejection::Account(AccountProblem::Mode { .. })));
            if mode_problem {
                format!(
                    "No online worker has an account that can run in {} mode. \
                     Choose a mode with more access, or choose another account.",
                    mode_name(input.approval_mode)
                )
            } else if let Some(selection) = input.selection {
                format!(
                    "No online worker has the account {} with the model {}.",
                    selection.account_label, selection.model
                )
            } else {
                "No online worker has a signed-in harness account.".to_owned()
            }
        }
        Stage::Require => {
            // The account check removed some workers, so say who is left.
            let scope = if rejections.iter().any(|r| r.stage() == Stage::Account) {
                match input.selection {
                    Some(selection) => {
                        format!(
                            "No online worker with the account {}",
                            selection.account_label
                        )
                    }
                    None => "No online worker with a signed-in harness account".to_owned(),
                }
            } else {
                "No online worker".to_owned()
            };
            let wanted = effective_require(input)
                .as_ref()
                .map(describe_require)
                .unwrap_or_default();
            format!("{scope} {wanted}.")
        }
        Stage::Policy => "No online worker that fits the rest of the request allows an \
                          in-place run in the folders of these repos. Use a worktree or a \
                          clone, or add the folder to allowed_roots in the worker policy."
            .to_owned(),
    }
}

/// The order of the checks, for `wait_reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Account,
    Require,
    Policy,
}

impl Rejection {
    fn stage(&self) -> Stage {
        match self {
            Self::Account(_) => Stage::Account,
            Self::Require => Stage::Require,
            Self::Policy { .. } => Stage::Policy,
        }
    }

    /// The error for a request that names this worker. It says why and what
    /// to change.
    fn named_worker_error(
        &self,
        input: &PlacementInput<'_>,
        worker: &Worker,
        inventory: &Inventory,
    ) -> ErrorBody {
        let name = &worker.name;
        match self {
            Self::Account(AccountProblem::Missing) => ErrorBody::new(
                ErrorCode::NoMatchingAccount,
                explain_missing_account(name, inventory, input.selection),
            ),
            Self::Account(AccountProblem::Mode { label }) => ErrorBody::new(
                ErrorCode::NoMatchingAccount,
                format!(
                    "The account {label} on {name} cannot run in {} mode. \
                     Choose a mode with more access, or choose another account.",
                    mode_name(input.approval_mode)
                ),
            ),
            Self::Require => {
                let wanted = effective_require(input)
                    .as_ref()
                    .map(describe_require)
                    .unwrap_or_default();
                ErrorBody::new(
                    ErrorCode::Invalid,
                    format!(
                        "The request needs a worker that {wanted}, but {name} does not. \
                         Choose another worker, or change the requirement."
                    ),
                )
            }
            Self::Policy { path } => ErrorBody::new(
                ErrorCode::PolicyDenied,
                format!(
                    "{name} does not allow an in-place run in {path}. \
                     Use a worktree or a clone, or add the folder to allowed_roots \
                     in the worker policy of {name}."
                ),
            ),
        }
    }
}

/// Says which part of the account choice failed on one worker.
fn explain_missing_account(
    name: &str,
    inventory: &Inventory,
    selection: Option<&ModelSelection>,
) -> String {
    let Some(selection) = selection else {
        return format!(
            "{name} has no signed-in harness account with a model. \
             Sign in to a harness on that machine."
        );
    };
    let label = &selection.account_label;
    let same_label: Vec<&HarnessAccount> = inventory
        .accounts
        .iter()
        .filter(|a| a.label == *label)
        .collect();
    if same_label.is_empty() {
        format!(
            "{name} has no harness account named {label}. \
             Choose an account that {name} has, or leave the worker empty."
        )
    } else if !same_label.iter().any(|a| a.enabled) {
        format!(
            "The account {label} is turned off on {name}. \
             Turn it on in worker.toml, or choose another account."
        )
    } else if !same_label
        .iter()
        .any(|a| a.enabled && a.auth_state == AuthState::SignedIn)
    {
        format!(
            "The account {label} on {name} is not signed in. \
             Sign in to the harness on that machine, then try again."
        )
    } else {
        format!(
            "The account {label} on {name} does not offer the model {}. \
             Choose another model.",
            selection.model
        )
    }
}

/// Describes the `require` constraints, for example
/// `is a Windows machine and has the tag gpu`.
fn describe_require(require: &PlacementRequire) -> String {
    let mut parts = Vec::new();
    if let Some(os) = require.os {
        parts.push(format!("is {}", os_name(os)));
    }
    if !require.tags.is_empty() {
        let noun = if require.tags.len() == 1 {
            "tag"
        } else {
            "tags"
        };
        parts.push(format!("has the {noun} {}", join_list(&require.tags)));
    }
    if !require.local_mcp_servers.is_empty() {
        let noun = if require.local_mcp_servers.len() == 1 {
            "server"
        } else {
            "servers"
        };
        parts.push(format!(
            "has the local MCP {noun} {}",
            join_list(&require.local_mcp_servers)
        ));
    }
    parts.join(" and ")
}

/// Joins names as `a`, `a and b`, or `a, b and c`.
fn join_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

fn os_name(os: Os) -> &'static str {
    match os {
        Os::Macos => "a macOS machine",
        Os::Windows => "a Windows machine",
        Os::Linux => "a Linux machine",
    }
}

pub(crate) fn mode_name(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "Ask",
        ApprovalMode::AutoEdits => "Auto edits",
        ApprovalMode::Auto => "Auto",
        ApprovalMode::FullAccess => "Full access",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::auth::PublicKey;
    use consort_proto::model::{
        Capabilities, CheckoutInfo, LocalMcpServerInfo, ModelInfo, OptionValue, WorkerLoad,
        WorkerPolicy,
    };
    use consort_proto::time::Timestamp;

    // Builders ---------------------------------------------------------------

    fn account(label: &str, harness: &str, models: &[&str]) -> HarnessAccount {
        HarnessAccount {
            account_id: AccountId::new(),
            harness: harness.to_owned(),
            label: label.to_owned(),
            harness_version: None,
            auth_state: AuthState::SignedIn,
            auth_detail: None,
            models: models
                .iter()
                .map(|id| ModelInfo {
                    id: (*id).to_owned(),
                    name: (*id).to_owned(),
                    options: Vec::new(),
                })
                .collect(),
            capabilities: Capabilities::default(),
            enabled: true,
            profile: false,
            email: None,
            plan: None,
            secret_names: Vec::new(),
            sign_in_methods: Vec::new(),
        }
    }

    fn claude() -> HarnessAccount {
        account("Claude Code", "claude", &["opus-5.5", "sonnet-5.5"])
    }

    /// An online worker on macOS with one account and an empty policy.
    fn worker(name: &str, accounts: Vec<HarnessAccount>) -> Worker {
        Worker {
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

    fn inventory(worker: &mut Worker) -> &mut Inventory {
        worker
            .inventory
            .as_mut()
            .expect("the test worker has an inventory")
    }

    fn repo(url: &str) -> Repo {
        Repo {
            id: RepoId::new(),
            url: url.to_owned(),
            name: url.rsplit('/').next().unwrap_or(url).to_owned(),
            default_branch: "main".to_owned(),
        }
    }

    fn checkout(url: &str, path: &str) -> CheckoutInfo {
        CheckoutInfo {
            repo_url: url.to_owned(),
            path: path.to_owned(),
            managed: false,
        }
    }

    /// Owns the input of one placement call.
    struct Case {
        subject: Subject,
        workers: Vec<Worker>,
        open_sessions: HashMap<WorkerId, u32>,
        selection: Option<ModelSelection>,
        request: PlacementRequest,
        approval_mode: ApprovalMode,
        preferred: Vec<WorkerId>,
        channel_tags: Vec<String>,
        repos: Vec<Repo>,
        isolation: Isolation,
    }

    impl Case {
        fn task(workers: Vec<Worker>) -> Self {
            Self {
                subject: Subject::Task,
                workers,
                open_sessions: HashMap::new(),
                selection: None,
                request: PlacementRequest::default(),
                approval_mode: ApprovalMode::Ask,
                preferred: Vec::new(),
                channel_tags: Vec::new(),
                repos: Vec::new(),
                isolation: Isolation::Worktree,
            }
        }

        fn coordinator(workers: Vec<Worker>) -> Self {
            Self {
                subject: Subject::Coordinator,
                ..Self::task(workers)
            }
        }

        fn select(mut self, label: &str, model: &str) -> Self {
            self.selection = Some(ModelSelection {
                account_label: label.to_owned(),
                model: model.to_owned(),
                options: ModelOptions::new(),
            });
            self
        }

        fn run(&self) -> Result<Placement, PlacementError> {
            place(&PlacementInput {
                subject: self.subject,
                workers: &self.workers,
                open_sessions: &self.open_sessions,
                selection: self.selection.as_ref(),
                request: &self.request,
                approval_mode: self.approval_mode,
                preferred_workers: &self.preferred,
                channel_tags: &self.channel_tags,
                repos: &self.repos,
                isolation: self.isolation,
            })
        }

        /// The name of the chosen worker.
        fn chosen(&self) -> String {
            self.run().expect("placement should succeed").worker_name
        }

        fn wait_reason(&self) -> String {
            match self.run() {
                Err(PlacementError::Wait(reason)) => reason,
                other => panic!("expected a wait, got {other:?}"),
            }
        }

        fn fail(&self) -> ErrorBody {
            match self.run() {
                Err(PlacementError::Fail(body)) => body,
                other => panic!("expected a failure, got {other:?}"),
            }
        }
    }

    // Named worker -----------------------------------------------------------

    #[test]
    fn named_worker_that_is_offline_waits() {
        let mut a = worker("mini", vec![claude()]);
        a.state = WorkerState::Offline;
        let mut case = Case::task(vec![a.clone(), worker("studio", vec![claude()])]);
        case.request.worker_id = Some(a.id);
        let err = case.run().unwrap_err();
        assert_eq!(
            err,
            PlacementError::Wait("Waiting for mini to come online.".to_owned())
        );
        assert_eq!(err.reason(), "Waiting for mini to come online.");
    }

    #[test]
    fn named_worker_that_is_paused_waits() {
        let mut a = worker("mini", vec![claude()]);
        a.paused = true;
        let mut case = Case::task(vec![a.clone()]);
        case.request.worker_id = Some(a.id);
        assert_eq!(case.wait_reason(), "mini is paused.");
    }

    #[test]
    fn named_worker_without_inventory_waits() {
        let mut a = worker("mini", vec![claude()]);
        a.inventory = None;
        let mut case = Case::task(vec![a.clone()]);
        case.request.worker_id = Some(a.id);
        assert_eq!(
            case.wait_reason(),
            "Waiting for mini to send its inventory."
        );
    }

    #[test]
    fn named_worker_that_is_unknown_or_revoked_fails() {
        let mut revoked = worker("old", vec![claude()]);
        revoked.revoked_at = Some(Timestamp::from_millis(5));
        let mut case = Case::task(vec![revoked.clone(), worker("mini", vec![claude()])]);

        case.request.worker_id = Some(WorkerId::new());
        let err = case.fail();
        assert_eq!(err.code, ErrorCode::WorkerUnknown);
        assert!(err.message.contains("Choose another worker"), "{err}");

        case.request.worker_id = Some(revoked.id);
        assert_eq!(case.fail().code, ErrorCode::WorkerUnknown);
    }

    #[test]
    fn named_worker_is_used_even_when_another_ranks_higher() {
        let a = worker("mini", vec![claude()]);
        let b = worker("studio", vec![claude()]);
        let mut case = Case::task(vec![a, b.clone()]);
        case.preferred = vec![case.workers[0].id];
        case.request.worker_id = Some(b.id);
        assert_eq!(case.chosen(), "studio");
    }

    #[test]
    fn named_worker_without_the_account_fails_and_says_why() {
        let a = worker("mini", vec![claude()]);
        let mut case = Case::task(vec![a.clone()]).select("Claude Personal", "opus-5.5");
        case.request.worker_id = Some(a.id);
        let err = case.fail();
        assert_eq!(err.code, ErrorCode::NoMatchingAccount);
        assert_eq!(
            err.message,
            "mini has no harness account named Claude Personal. \
             Choose an account that mini has, or leave the worker empty."
        );

        // The label exists, but the model does not.
        let mut case = Case::task(case.workers).select("Claude Code", "opus-9");
        case.request.worker_id = Some(a.id);
        let err = case.fail();
        assert_eq!(err.code, ErrorCode::NoMatchingAccount);
        assert!(
            err.message.contains("does not offer the model opus-9"),
            "{err}"
        );

        // The account is signed out.
        let mut signed_out = claude();
        signed_out.auth_state = AuthState::SignedOut;
        let b = worker("studio", vec![signed_out]);
        let mut case = Case::task(vec![b.clone()]).select("Claude Code", "opus-5.5");
        case.request.worker_id = Some(b.id);
        let err = case.fail();
        assert_eq!(err.code, ErrorCode::NoMatchingAccount);
        assert!(err.message.contains("is not signed in"), "{err}");

        // The account is turned off.
        let mut off = claude();
        off.enabled = false;
        let c = worker("laptop", vec![off]);
        let mut case = Case::task(vec![c.clone()]).select("Claude Code", "opus-5.5");
        case.request.worker_id = Some(c.id);
        assert!(
            case.fail().message.contains("is turned off"),
            "{}",
            case.fail()
        );
    }

    #[test]
    fn named_worker_without_any_usable_account_fails_by_default_rule() {
        let a = worker("mini", vec![]);
        let mut case = Case::task(vec![a.clone()]);
        case.request.worker_id = Some(a.id);
        let err = case.fail();
        assert_eq!(err.code, ErrorCode::NoMatchingAccount);
        assert!(err.message.contains("Sign in to a harness"), "{err}");
    }

    #[test]
    fn named_worker_that_fails_require_gets_an_invalid_error() {
        let a = worker("mini", vec![claude()]);
        let mut case = Case::task(vec![a.clone()]);
        case.request.worker_id = Some(a.id);
        case.request.require = Some(PlacementRequire {
            os: Some(Os::Windows),
            ..PlacementRequire::default()
        });
        let err = case.fail();
        assert_eq!(err.code, ErrorCode::Invalid);
        assert!(err.message.contains("is a Windows machine"), "{err}");
        assert!(err.message.contains("mini does not"), "{err}");
    }

    #[test]
    fn named_worker_that_violates_policy_gets_policy_denied() {
        let url = "git@github.com:watzon/sayso.git";
        let mut a = worker("mini", vec![claude()]);
        inventory(&mut a).policy.allowed_roots = vec!["/home/chris/code".to_owned()];
        inventory(&mut a).checkouts = vec![checkout(url, "/tmp/sayso")];
        let mut case = Case::task(vec![a.clone()]);
        case.request.worker_id = Some(a.id);
        case.repos = vec![repo("https://github.com/watzon/sayso")];
        case.isolation = Isolation::InPlace;
        let err = case.fail();
        assert_eq!(err.code, ErrorCode::PolicyDenied);
        assert!(err.message.contains("/tmp/sayso"), "{err}");
        assert!(err.message.contains("allowed_roots"), "{err}");
    }

    // Candidates -------------------------------------------------------------

    #[test]
    fn revoked_workers_are_ignored() {
        let mut revoked = worker("old", vec![claude()]);
        revoked.revoked_at = Some(Timestamp::from_millis(5));
        let mut case = Case::task(vec![revoked]);
        assert_eq!(case.wait_reason(), "Waiting for a worker to come online.");
        case.workers.push(worker("mini", vec![claude()]));
        assert_eq!(case.chosen(), "mini");
    }

    #[test]
    fn paused_and_offline_workers_are_skipped() {
        let mut paused = worker("paused", vec![claude()]);
        paused.paused = true;
        let mut offline = worker("offline", vec![claude()]);
        offline.state = WorkerState::Offline;
        let good = worker("good", vec![claude()]);
        let case = Case::task(vec![paused, offline, good]);
        assert_eq!(case.chosen(), "good");
    }

    #[test]
    fn worker_without_inventory_is_skipped() {
        let mut empty = worker("empty", vec![claude()]);
        empty.inventory = None;
        let good = worker("good", vec![claude()]);
        let mut case = Case::task(vec![empty, good]);
        assert_eq!(case.chosen(), "good");
        case.workers.remove(1);
        assert_eq!(
            case.wait_reason(),
            "Waiting for the online workers to send their inventory."
        );
    }

    // Account choice ---------------------------------------------------------

    #[test]
    fn selection_picks_the_account_by_label_and_model() {
        let personal = account("Claude Personal", "claude", &["opus-5.5"]);
        let work = account("Claude Work", "claude", &["sonnet-5.5"]);
        let personal_id = personal.account_id;
        let a = worker("mini", vec![work.clone()]);
        let b = worker("studio", vec![work, personal]);
        let mut case = Case::task(vec![a, b]).select("Claude Personal", "opus-5.5");
        case.selection
            .as_mut()
            .expect("selection is set")
            .options
            .insert("effort".to_owned(), OptionValue::Text("high".to_owned()));
        let placement = case.run().unwrap();
        assert_eq!(placement.worker_name, "studio");
        assert_eq!(placement.account_id, personal_id);
        assert_eq!(placement.account_label, "Claude Personal");
        assert_eq!(placement.harness, "claude");
        assert_eq!(placement.model, "opus-5.5");
        assert_eq!(
            placement.options.get("effort"),
            Some(&OptionValue::Text("high".to_owned()))
        );
    }

    #[test]
    fn selection_needs_the_model_on_that_account() {
        let personal = account("Claude Personal", "claude", &["sonnet-5.5"]);
        let case =
            Case::task(vec![worker("mini", vec![personal])]).select("Claude Personal", "opus-5.5");
        assert_eq!(
            case.wait_reason(),
            "No online worker has the account Claude Personal with the model opus-5.5."
        );
    }

    #[test]
    fn default_choice_prefers_claude_code_then_codex_then_pi() {
        let pi = account("Pi", "pi", &["pi-1"]);
        let codex = account("Codex", "codex", &["gpt-6"]);
        let claude = claude();
        let w = worker("mini", vec![pi.clone(), codex.clone(), claude.clone()]);
        let placement = Case::task(vec![w]).run().unwrap();
        assert_eq!(placement.account_label, "Claude Code");
        assert_eq!(
            placement.model, "opus-5.5",
            "the first model of the account"
        );
        assert!(placement.options.is_empty());

        let w = worker("mini", vec![pi.clone(), codex.clone()]);
        assert_eq!(Case::task(vec![w]).run().unwrap().account_label, "Codex");

        let w = worker("mini", vec![pi]);
        assert_eq!(Case::task(vec![w]).run().unwrap().account_label, "Pi");
    }

    #[test]
    fn default_choice_uses_the_harness_id_when_the_label_is_custom() {
        let pi = account("My Pi", "pi", &["pi-1"]);
        let codex = account("Work codex login", "codex", &["gpt-6"]);
        let claude = account("Claude Personal", "claude", &["opus-5.5"]);
        let gemini = account("Gem", "acp:gemini-cli", &["gemini-3"]);
        let other = account("Zed", "acp:zed", &["z-1"]);

        let rank = |a: &HarnessAccount| harness_rank(a);
        assert_eq!(rank(&claude), 0);
        assert_eq!(rank(&codex), 1);
        assert_eq!(rank(&gemini), 3);
        assert_eq!(rank(&pi), 4);
        assert_eq!(rank(&other), 5);

        let w = worker("mini", vec![other, pi, gemini, codex, claude]);
        assert_eq!(
            Case::task(vec![w]).run().unwrap().account_label,
            "Claude Personal"
        );
    }

    #[test]
    fn known_labels_rank_before_harness_ids_and_ignore_case() {
        // The label "codex" (any case) ranks 1 even when the harness is odd.
        let a = account("CODEX", "acp:other", &["m"]);
        assert_eq!(harness_rank(&a), 1);
        // A custom label on harness "claude" ranks like Claude Code.
        let b = account("Claude Personal", "claude", &["m"]);
        assert_eq!(harness_rank(&b), 0);
        // The label wins over the harness id.
        let c = account("Pi", "claude", &["m"]);
        assert_eq!(harness_rank(&c), 4);
    }

    #[test]
    fn default_choice_ties_go_to_the_first_account_in_the_inventory() {
        let first = account("Claude A", "claude", &["a-1"]);
        let second = account("Claude B", "claude", &["b-1"]);
        let w = worker("mini", vec![first, second]);
        assert_eq!(Case::task(vec![w]).run().unwrap().account_label, "Claude A");
    }

    #[test]
    fn default_choice_skips_accounts_that_cannot_run() {
        let mut signed_out = claude();
        signed_out.auth_state = AuthState::SignedOut;
        let mut off = account("Codex", "codex", &["gpt-6"]);
        off.enabled = false;
        let no_models = account("OpenCode", "opencode", &[]);
        let pi = account("Pi", "pi", &["pi-1"]);
        let w = worker("mini", vec![signed_out, off, no_models, pi]);
        assert_eq!(Case::task(vec![w]).run().unwrap().account_label, "Pi");
    }

    #[test]
    fn signed_out_account_is_skipped_for_a_selection() {
        let mut signed_out = claude();
        signed_out.auth_state = AuthState::SignedOut;
        let a = worker("mini", vec![signed_out]);
        let b = worker("studio", vec![claude()]);
        let case = Case::task(vec![a, b]).select("Claude Code", "opus-5.5");
        assert_eq!(case.chosen(), "studio");
    }

    #[test]
    fn approval_mode_filters_accounts_that_cannot_enforce_it() {
        // Pi supports only Auto and Full access, so it cannot run in Ask.
        let mut pi = account("Pi", "pi", &["pi-1"]);
        pi.capabilities.approval_modes = vec![ApprovalMode::Auto, ApprovalMode::FullAccess];

        // Default choice: the best-ranked account cannot enforce Ask, so
        // the next one wins.
        let mut limited = claude();
        limited.capabilities.approval_modes = vec![ApprovalMode::Auto, ApprovalMode::FullAccess];
        let mut codex = account("Codex", "codex", &["gpt-6"]);
        codex.capabilities.approval_modes = ApprovalMode::ALL.to_vec();
        let mut case = Case::task(vec![worker("mini", vec![limited, codex])]);
        case.approval_mode = ApprovalMode::Ask;
        assert_eq!(case.run().unwrap().account_label, "Codex");
        case.approval_mode = ApprovalMode::Auto;
        assert_eq!(case.run().unwrap().account_label, "Claude Code");

        // Only Pi: wait with a reason about the mode.
        let case = Case::task(vec![worker("mini", vec![pi.clone()])]);
        let reason = case.wait_reason();
        assert!(reason.contains("run in Ask mode"), "{reason}");
        assert!(reason.contains("more access"), "{reason}");

        // Pi is fine in Auto mode.
        let mut case = Case::task(vec![worker("mini", vec![pi.clone()])]);
        case.approval_mode = ApprovalMode::Auto;
        assert_eq!(case.run().unwrap().account_label, "Pi");

        // A selection of Pi in Ask mode waits, and fails for a named worker.
        let w = worker("mini", vec![pi]);
        let mut case = Case::task(vec![w.clone()]).select("Pi", "pi-1");
        assert!(case.wait_reason().contains("run in Ask mode"));
        case.request.worker_id = Some(w.id);
        let err = case.fail();
        assert_eq!(err.code, ErrorCode::NoMatchingAccount);
        assert!(
            err.message
                .contains("The account Pi on mini cannot run in Ask mode"),
            "{err}"
        );
    }

    #[test]
    fn empty_approval_mode_list_means_unknown_and_is_accepted() {
        let account = claude();
        assert!(account.capabilities.approval_modes.is_empty());
        let mut case = Case::task(vec![worker("mini", vec![account])]);
        case.approval_mode = ApprovalMode::Ask;
        assert!(case.run().is_ok());
    }

    // Require ----------------------------------------------------------------

    #[test]
    fn require_filters_by_os() {
        let mac = worker("mac", vec![claude()]);
        let mut win = worker("win", vec![claude()]);
        win.os = Os::Windows;
        let mut case = Case::task(vec![mac, win]);
        case.request.require = Some(PlacementRequire {
            os: Some(Os::Windows),
            ..PlacementRequire::default()
        });
        assert_eq!(case.chosen(), "win");
        case.workers.remove(1);
        assert_eq!(case.wait_reason(), "No online worker is a Windows machine.");
    }

    #[test]
    fn require_filters_by_tags_from_the_user_or_the_inventory() {
        let plain = worker("plain", vec![claude()]);
        let mut user_label = worker("user", vec![claude()]);
        user_label.tags = vec!["gpu".to_owned()];
        let mut inv_label = worker("inv", vec![claude()]);
        inventory(&mut inv_label).tags = vec!["gpu".to_owned(), "big".to_owned()];

        let mut case = Case::task(vec![plain.clone(), user_label, inv_label.clone()]);
        case.request.require = Some(PlacementRequire {
            tags: vec!["gpu".to_owned()],
            ..PlacementRequire::default()
        });
        assert_eq!(case.chosen(), "user");

        // Both tags needed: only the inventory worker has both.
        case.request.require = Some(PlacementRequire {
            tags: vec!["gpu".to_owned(), "big".to_owned()],
            ..PlacementRequire::default()
        });
        assert_eq!(case.chosen(), "inv");

        let mut none = Case::task(vec![plain]);
        none.request.require = Some(PlacementRequire {
            tags: vec!["gpu".to_owned(), "big".to_owned()],
            ..PlacementRequire::default()
        });
        assert_eq!(
            none.wait_reason(),
            "No online worker has the tags gpu and big."
        );
    }

    #[test]
    fn require_filters_by_local_mcp_servers() {
        let plain = worker("plain", vec![claude()]);
        let mut home = worker("home", vec![claude()]);
        inventory(&mut home).local_mcp_servers = vec![LocalMcpServerInfo {
            name: "home-assistant".to_owned(),
            description: None,
            tools: Vec::new(),
        }];
        let mut case = Case::task(vec![plain, home]);
        case.request.require = Some(PlacementRequire {
            local_mcp_servers: vec!["home-assistant".to_owned()],
            ..PlacementRequire::default()
        });
        assert_eq!(case.chosen(), "home");
        case.workers.remove(1);
        assert_eq!(
            case.wait_reason(),
            "No online worker has the local MCP server home-assistant."
        );
    }

    #[test]
    fn require_reason_lists_every_constraint() {
        let mut case = Case::task(vec![worker("mini", vec![claude()])]);
        case.request.require = Some(PlacementRequire {
            os: Some(Os::Linux),
            tags: vec!["gpu".to_owned(), "big".to_owned(), "fast".to_owned()],
            local_mcp_servers: vec!["a".to_owned(), "b".to_owned()],
        });
        assert_eq!(
            case.wait_reason(),
            "No online worker is a Linux machine and has the tags gpu, big and fast \
             and has the local MCP servers a and b."
        );
    }

    #[test]
    fn require_reason_says_who_is_left_after_the_account_check() {
        let mut win = worker("win", vec![claude()]);
        win.os = Os::Windows;
        let mac = worker("mac", vec![account("Codex", "codex", &["gpt-6"])]);
        let mut case = Case::task(vec![win, mac]).select("Codex", "gpt-6");
        case.request.require = Some(PlacementRequire {
            os: Some(Os::Windows),
            ..PlacementRequire::default()
        });
        assert_eq!(
            case.wait_reason(),
            "No online worker with the account Codex is a Windows machine."
        );
    }

    // Ranking ----------------------------------------------------------------

    #[test]
    fn coordinator_prefers_the_hub_labelled_worker() {
        let a = worker("laptop", vec![claude()]);
        let mut hub = worker("hub-box", vec![claude()]);
        hub.tags = vec!["hub".to_owned()];
        let mut case = Case::coordinator(vec![a.clone(), hub]);
        case.preferred = vec![a.id];
        assert_eq!(case.chosen(), "hub-box");
    }

    #[test]
    fn hub_label_can_come_from_the_inventory() {
        let a = worker("laptop", vec![claude()]);
        let mut hub = worker("hub-box", vec![claude()]);
        inventory(&mut hub).tags = vec!["hub".to_owned()];
        let case = Case::coordinator(vec![a, hub]);
        assert_eq!(case.chosen(), "hub-box");
    }

    #[test]
    fn coordinator_without_a_hub_worker_follows_preferred_then_load_then_order() {
        let a = worker("a", vec![claude()]);
        let b = worker("b", vec![claude()]);
        let c = worker("c", vec![claude()]);
        let mut case = Case::coordinator(vec![a.clone(), b.clone(), c.clone()]);

        // No preference, no load: pairing order.
        assert_eq!(case.chosen(), "a");

        // Preferred workers come first, in list order.
        case.preferred = vec![c.id, b.id];
        assert_eq!(case.chosen(), "c");

        // A preferred worker that cannot run it is passed over.
        case.workers[2].state = WorkerState::Offline;
        assert_eq!(case.chosen(), "b");

        // With no preference, the worker with the fewest sessions wins.
        case.preferred.clear();
        case.workers[2].state = WorkerState::Online;
        case.open_sessions.insert(a.id, 3);
        case.open_sessions.insert(b.id, 1);
        assert_eq!(case.chosen(), "c");
    }

    #[test]
    fn coordinator_ignores_checkouts() {
        let url = "git@github.com:watzon/sayso.git";
        let a = worker("a", vec![claude()]);
        let mut b = worker("b", vec![claude()]);
        inventory(&mut b).checkouts = vec![checkout(url, "/code/sayso")];
        let mut case = Case::coordinator(vec![a, b]);
        case.repos = vec![repo(url)];
        assert_eq!(case.chosen(), "a");
    }

    #[test]
    fn task_prefers_the_worker_with_the_most_checkouts() {
        let sayso = "git@github.com:watzon/sayso.git";
        let other = "git@github.com:watzon/other.git";
        let a = worker("a", vec![claude()]);
        let mut b = worker("b", vec![claude()]);
        inventory(&mut b).checkouts = vec![checkout(sayso, "/code/sayso")];
        let mut c = worker("c", vec![claude()]);
        inventory(&mut c).checkouts = vec![
            checkout(sayso, "/code/sayso"),
            checkout(other, "/code/other"),
        ];
        let mut case = Case::task(vec![a.clone(), b.clone(), c]);
        case.repos = vec![repo(sayso), repo(other)];
        // The checkout count beats the preferred list and the load.
        case.preferred = vec![a.id, b.id];
        case.open_sessions.insert(case.workers[2].id, 9);
        assert_eq!(case.chosen(), "c");
    }

    #[test]
    fn task_falls_back_to_preferred_then_policy_then_load_then_order() {
        let a = worker("a", vec![claude()]);
        let b = worker("b", vec![claude()]);
        let c = worker("c", vec![claude()]);
        let mut case = Case::task(vec![a.clone(), b.clone(), c.clone()]);

        // Nothing separates them: pairing order.
        assert_eq!(case.chosen(), "a");

        // Fewest sessions.
        case.open_sessions.insert(a.id, 2);
        case.open_sessions.insert(b.id, 1);
        assert_eq!(case.chosen(), "c");

        // The larger of the hub's count and the worker's own count.
        inventory(&mut case.workers[2]).load.active_sessions = 5;
        assert_eq!(case.chosen(), "b");

        // A worker whose policy allows the mode ranks before busier or
        // earlier workers.
        case.approval_mode = ApprovalMode::Auto;
        inventory(&mut case.workers[1]).policy.max_approval_mode = ApprovalMode::Auto;
        inventory(&mut case.workers[0]).policy.max_approval_mode = ApprovalMode::FullAccess;
        inventory(&mut case.workers[2]).policy.max_approval_mode = ApprovalMode::Ask;
        case.open_sessions.insert(a.id, 8);
        assert_eq!(case.chosen(), "b", "b allows Auto and is less busy than a");

        // Preferred workers beat the policy and the load.
        case.preferred = vec![c.id];
        assert_eq!(case.chosen(), "c");
    }

    #[test]
    fn a_worker_whose_policy_caps_the_mode_is_not_removed() {
        let mut only = worker("only", vec![claude()]);
        inventory(&mut only).policy.max_approval_mode = ApprovalMode::Ask;
        let mut case = Case::task(vec![only]);
        case.approval_mode = ApprovalMode::FullAccess;
        assert_eq!(case.chosen(), "only");
    }

    // Policy and checkouts -----------------------------------------------------

    #[test]
    fn allowed_roots_limit_in_place_runs() {
        let url = "git@github.com:watzon/sayso.git";
        let mut inside = worker("inside", vec![claude()]);
        inventory(&mut inside).policy.allowed_roots = vec!["/home/chris/code".to_owned()];
        inventory(&mut inside).checkouts = vec![checkout(url, "/home/chris/code/sayso")];
        let mut outside = worker("outside", vec![claude()]);
        inventory(&mut outside).policy.allowed_roots = vec!["/home/chris/code".to_owned()];
        inventory(&mut outside).checkouts = vec![checkout(url, "/tmp/sayso")];

        // Put the violating worker first, with a checkout, so only the
        // policy can remove it.
        let mut case = Case::task(vec![outside.clone(), inside]);
        case.repos = vec![repo(url)];
        case.isolation = Isolation::InPlace;
        let placement = case.run().unwrap();
        assert_eq!(placement.worker_name, "inside");
        assert_eq!(
            placement.checkouts[0].checkout_path.as_deref(),
            Some("/home/chris/code/sayso")
        );

        // Only the violating worker: wait, and say why.
        let mut only = Case::task(vec![outside.clone()]);
        only.repos = vec![repo(url)];
        only.isolation = Isolation::InPlace;
        let reason = only.wait_reason();
        assert!(reason.contains("in-place run"), "{reason}");
        assert!(reason.contains("allowed_roots"), "{reason}");

        // A worktree or a clone does not need the root.
        let mut worktree = Case::task(vec![outside]);
        worktree.repos = vec![repo(url)];
        worktree.isolation = Isolation::Worktree;
        assert_eq!(worktree.chosen(), "outside");
    }

    #[test]
    fn allowed_roots_apply_only_to_repos_that_have_a_checkout() {
        let known = "git@github.com:watzon/known.git";
        let fresh = "git@github.com:watzon/fresh.git";
        let mut w = worker("w", vec![claude()]);
        inventory(&mut w).policy.allowed_roots = vec!["/code".to_owned()];
        inventory(&mut w).checkouts = vec![checkout(known, "/code/known")];
        let mut case = Case::task(vec![w]);
        case.repos = vec![repo(known), repo(fresh)];
        case.isolation = Isolation::InPlace;
        let placement = case.run().unwrap();
        assert_eq!(
            placement.checkouts[0].checkout_path.as_deref(),
            Some("/code/known")
        );
        assert_eq!(placement.checkouts[1].checkout_path, None);
    }

    #[test]
    fn empty_allowed_roots_do_not_limit() {
        let url = "git@github.com:watzon/sayso.git";
        let mut w = worker("w", vec![claude()]);
        inventory(&mut w).checkouts = vec![checkout(url, "/anywhere/sayso")];
        let mut case = Case::task(vec![w]);
        case.repos = vec![repo(url)];
        case.isolation = Isolation::InPlace;
        assert_eq!(case.chosen(), "w");
    }

    #[test]
    fn in_place_run_picks_the_checkout_inside_an_allowed_root() {
        let url = "https://github.com/watzon/sayso";
        let mut w = worker("w", vec![claude()]);
        inventory(&mut w).policy.allowed_roots = vec!["/code".to_owned()];
        inventory(&mut w).checkouts =
            vec![checkout(url, "/tmp/sayso"), checkout(url, "/code/sayso")];
        let mut case = Case::task(vec![w]);
        case.repos = vec![repo(url)];
        case.isolation = Isolation::InPlace;
        let placement = case.run().unwrap();
        assert_eq!(
            placement.checkouts[0].checkout_path.as_deref(),
            Some("/code/sayso")
        );

        // Without the restriction, the first checkout wins.
        case.isolation = Isolation::Worktree;
        let placement = case.run().unwrap();
        assert_eq!(
            placement.checkouts[0].checkout_path.as_deref(),
            Some("/tmp/sayso")
        );
    }

    #[test]
    fn path_prefix_rules() {
        // Whole segments only.
        assert!(is_under("/code/sayso", "/code", false));
        assert!(is_under("/code", "/code", false));
        assert!(is_under("/code/sayso", "/code/", false));
        assert!(!is_under("/code2/sayso", "/code", false));
        assert!(!is_under("/cod", "/code", false));
        // Dot segments cannot climb out of a root.
        assert!(!is_under("/code/../etc", "/code", false));
        assert!(is_under("/code/./a/../b", "/code", false));
        // Case matters on Unix and not on Windows.
        assert!(!is_under("/Code/sayso", "/code", false));
        assert!(is_under(
            "C:\\Users\\Chris\\Code\\sayso",
            "c:/users/chris/code",
            true
        ));
        assert!(is_under(
            "c:/users/chris/code",
            "C:\\Users\\Chris\\Code\\",
            true
        ));
        assert!(!is_under(
            "C:\\Users\\Chris\\Code\\sayso",
            "c:/users/chris/code",
            false
        ));
        assert!(!is_under("D:\\Code", "C:\\Code", true));
        // Absolute and relative paths do not mix. A network path is not a
        // local path.
        assert!(!is_under("code/sayso", "/code", false));
        assert!(!is_under("//server/share/a", "/server/share", false));
        assert!(is_under("//server/share/a", "\\\\server\\share", true));
        // A root of `/` holds every absolute path.
        assert!(is_under("/anything", "/", false));
    }

    #[test]
    fn windows_worker_ignores_case_in_allowed_roots() {
        let url = "git@github.com:watzon/sayso.git";
        let mut w = worker("win", vec![claude()]);
        w.os = Os::Windows;
        inventory(&mut w).policy.allowed_roots = vec!["C:\\Users\\chris\\Code".to_owned()];
        inventory(&mut w).checkouts = vec![checkout(url, "c:/users/Chris/code/sayso")];
        let mut case = Case::task(vec![w]);
        case.repos = vec![repo(url)];
        case.isolation = Isolation::InPlace;
        assert_eq!(case.chosen(), "win");
    }

    #[test]
    fn checkouts_match_ssh_and_https_urls_of_one_repo() {
        let mut w = worker("w", vec![claude()]);
        inventory(&mut w).checkouts = vec![
            checkout("git@github.com:watzon/sayso.git", "/code/sayso"),
            checkout("ssh://git@GitHub.com/watzon/Consort.git/", "/code/consort"),
        ];
        let sayso = repo("https://github.com/watzon/sayso");
        let consort = repo("https://github.com/watzon/consort.git");
        let missing = repo("https://github.com/watzon/missing");
        let mut case = Case::task(vec![w]);
        case.repos = vec![sayso.clone(), missing.clone(), consort.clone()];
        let placement = case.run().unwrap();
        assert_eq!(
            placement.checkouts,
            vec![
                RepoCheckout {
                    repo_id: sayso.id,
                    url: sayso.url,
                    checkout_path: Some("/code/sayso".to_owned()),
                },
                RepoCheckout {
                    repo_id: missing.id,
                    url: missing.url,
                    checkout_path: None,
                },
                RepoCheckout {
                    repo_id: consort.id,
                    url: consort.url,
                    checkout_path: Some("/code/consort".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn a_session_without_repos_has_no_checkouts() {
        let case = Case::coordinator(vec![worker("w", vec![claude()])]);
        assert!(case.run().unwrap().checkouts.is_empty());
    }

    // Wait reasons -----------------------------------------------------------

    #[test]
    fn wait_reason_without_workers() {
        let case = Case::task(vec![]);
        assert_eq!(case.wait_reason(), "Waiting for a worker to come online.");
    }

    #[test]
    fn wait_reason_when_every_worker_is_offline() {
        let mut a = worker("a", vec![claude()]);
        a.state = WorkerState::Offline;
        let case = Case::task(vec![a]);
        assert_eq!(case.wait_reason(), "Waiting for a worker to come online.");
    }

    #[test]
    fn wait_reason_when_every_worker_is_paused() {
        let mut a = worker("a", vec![claude()]);
        a.paused = true;
        let mut b = worker("b", vec![claude()]);
        b.paused = true;
        b.state = WorkerState::Offline;
        let mut case = Case::task(vec![a, b]);
        assert_eq!(case.wait_reason(), "All workers are paused.");

        // An unpaused worker that is offline: the user waits for it.
        let mut c = worker("c", vec![claude()]);
        c.state = WorkerState::Offline;
        case.workers.push(c);
        assert_eq!(case.wait_reason(), "Waiting for a worker to come online.");
    }

    #[test]
    fn wait_reason_when_no_worker_has_a_signed_in_account() {
        let mut signed_out = claude();
        signed_out.auth_state = AuthState::SignedOut;
        let case = Case::task(vec![worker("a", vec![signed_out]), worker("b", vec![])]);
        assert_eq!(
            case.wait_reason(),
            "No online worker has a signed-in harness account."
        );
    }

    #[test]
    fn wait_reason_names_the_selected_account_and_model() {
        let case =
            Case::task(vec![worker("a", vec![claude()])]).select("Claude Personal", "opus-5.5");
        assert_eq!(
            case.wait_reason(),
            "No online worker has the account Claude Personal with the model opus-5.5."
        );
    }

    #[test]
    fn coordinator_waits_with_the_same_reasons() {
        let case = Case::coordinator(vec![worker("a", vec![])]);
        assert_eq!(
            case.wait_reason(),
            "No online worker has a signed-in harness account."
        );
    }

    // Channel tags ----------------------------------------------------------

    fn tagged(name: &str, tags: &[&str]) -> Worker {
        let mut worker = worker(name, vec![claude()]);
        worker.tags = tags.iter().map(|t| (*t).to_owned()).collect();
        worker
    }

    fn with_channel_tags(mut case: Case, tags: &[&str]) -> Case {
        case.channel_tags = tags.iter().map(|t| (*t).to_owned()).collect();
        case
    }

    #[test]
    fn channel_tags_limit_a_task_to_workers_with_every_tag() {
        // The untagged worker comes first and has no load, so without the
        // tag it would win.
        let mut busy = tagged("busy", &["work"]);
        busy.paired_at = Timestamp::from_millis(5);
        let case = with_channel_tags(
            Case::task(vec![worker("plain", vec![claude()]), busy]),
            &["work"],
        );
        assert_eq!(case.chosen(), "busy");
    }

    #[test]
    fn channel_tags_need_every_tag_and_read_the_inventory_tags_too() {
        let work_only = tagged("work-only", &["work"]);
        let mut both = tagged("both", &["work"]);
        inventory(&mut both).tags = vec!["gpu-apple".to_owned()];
        let case = with_channel_tags(Case::task(vec![work_only, both]), &["work", "gpu-apple"]);
        assert_eq!(case.chosen(), "both");
    }

    #[test]
    fn channel_tags_add_to_the_tags_of_the_request() {
        let work_only = tagged("work-only", &["work"]);
        let gpu_only = tagged("gpu-only", &["gpu"]);
        let both = tagged("both", &["work", "gpu"]);
        let mut case = with_channel_tags(Case::task(vec![work_only, gpu_only, both]), &["work"]);
        case.request.require = Some(PlacementRequire {
            tags: vec!["gpu".to_owned()],
            ..PlacementRequire::default()
        });
        assert_eq!(case.chosen(), "both");
    }

    #[test]
    fn channel_tags_apply_to_a_coordinator() {
        let mut hub = worker("hub", vec![claude()]);
        hub.tags = vec!["hub".to_owned()];
        let case = with_channel_tags(
            Case::coordinator(vec![hub, tagged("work", &["work"])]),
            &["work"],
        );
        // The hub tag ranks first for a coordinator, but not without the
        // channel tag.
        assert_eq!(case.chosen(), "work");
    }

    #[test]
    fn a_channel_without_tags_places_as_before() {
        let case = with_channel_tags(Case::task(vec![worker("a", vec![claude()])]), &[]);
        assert_eq!(case.chosen(), "a");
    }

    #[test]
    fn work_waits_when_no_worker_has_the_channel_tags() {
        let case = with_channel_tags(Case::task(vec![worker("a", vec![claude()])]), &["work"]);
        assert_eq!(
            case.wait_reason(),
            "Waiting for an online worker with the tag work. \
             Add the tag to a worker in Settings > Workers."
        );
        let case = with_channel_tags(
            Case::coordinator(vec![worker("a", vec![claude()])]),
            &["work", "gpu"],
        );
        assert_eq!(
            case.wait_reason(),
            "Waiting for an online worker with the tags work and gpu. \
             Add the tags to a worker in Settings > Workers."
        );
    }

    #[test]
    fn work_waits_when_the_tagged_worker_is_offline() {
        let mut away = tagged("away", &["work"]);
        away.state = WorkerState::Offline;
        let case = with_channel_tags(
            Case::task(vec![worker("plain", vec![claude()]), away]),
            &["work"],
        );
        // The reason does not tell the user to add the tag: a worker has it.
        assert_eq!(
            case.wait_reason(),
            "Waiting for an online worker with the tag work."
        );
    }

    #[test]
    fn work_waits_when_the_tagged_worker_is_paused() {
        let mut paused = tagged("paused", &["work"]);
        paused.paused = true;
        let case = with_channel_tags(
            Case::task(vec![worker("plain", vec![claude()]), paused]),
            &["work"],
        );
        assert_eq!(
            case.wait_reason(),
            "Waiting for an online worker with the tag work. \
             Every worker with the tag is paused."
        );
    }

    #[test]
    fn a_removed_worker_does_not_count_as_having_the_tag() {
        let mut gone = tagged("gone", &["work"]);
        gone.revoked_at = Some(Timestamp::from_millis(1));
        let case = with_channel_tags(Case::task(vec![gone]), &["work"]);
        assert!(case.wait_reason().contains("Settings > Workers"));
    }

    #[test]
    fn the_tagged_worker_without_an_account_gives_the_account_reason() {
        let no_account = {
            let mut w = worker("tagged", vec![]);
            w.tags = vec!["work".to_owned()];
            w
        };
        let case = with_channel_tags(
            Case::task(vec![no_account, worker("plain", vec![claude()])]),
            &["work"],
        );
        assert_eq!(
            case.wait_reason(),
            "No online worker with a signed-in harness account has the tag work."
        );
    }

    #[test]
    fn a_named_worker_without_the_channel_tags_fails_and_says_so() {
        let plain = worker("plain", vec![claude()]);
        let id = plain.id;
        let mut case = with_channel_tags(Case::task(vec![plain]), &["work"]);
        case.request.worker_id = Some(id);
        let error = case.fail();
        assert_eq!(error.code, ErrorCode::Invalid);
        assert!(
            error.message.contains("has the tag work"),
            "{}",
            error.message
        );
    }

    #[test]
    fn placement_does_not_change_its_input() {
        let case = Case::task(vec![worker("a", vec![claude()])]);
        let before = case.workers.clone();
        let first = case.run().unwrap();
        let second = case.run().unwrap();
        assert_eq!(first, second);
        assert_eq!(case.workers, before);
    }
}
