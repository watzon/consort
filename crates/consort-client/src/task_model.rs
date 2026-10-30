//! The model of a task after it started: the pick that the thread offers,
//! and whether a pick starts a new session.
//!
//! The hub decides. It keeps the session when the pick is the same account
//! and the harness can switch its model, and it starts a new session in the
//! same workspace in every other case. This module only tells the user
//! before the pick, from what the worker reported. A wrong guess here costs
//! a note that is not exact. It never changes what the hub does.

use consort_proto::model::{AuthState, Capabilities, HarnessAccount, Worker};

use crate::view::TaskModelOption;

/// The note under a pick that starts a new session.
pub const NEW_SESSION_NOTE: &str = "This starts a new session in the same folder.";

/// Whether a pick starts a new session.
///
/// - `has_session`: the task has a live session. With none, the next turn
///   starts one anyway, so the pick costs nothing.
/// - `current_account`: the label of the account that the session runs on.
/// - `can_switch`: the harness of that account can change its model in a
///   running session.
pub fn starts_new_session(
    has_session: bool,
    current_account: Option<&str>,
    can_switch: bool,
    picked_account: &str,
) -> bool {
    if !has_session {
        return false;
    }
    !(can_switch && current_account == Some(picked_account))
}

fn can_switch(account: &HarnessAccount) -> bool {
    let Capabilities { switch_model, .. } = &account.capabilities;
    *switch_model
}

/// The models that the worker of a task offers, with the note for each
/// pick. Empty when the worker is not known or has not reported.
pub fn model_options(
    worker: Option<&Worker>,
    has_session: bool,
    current_account: Option<&str>,
) -> Vec<TaskModelOption> {
    let Some(inventory) = worker.and_then(|w| w.inventory.as_ref()) else {
        return Vec::new();
    };
    let can_switch_now = current_account
        .and_then(|label| inventory.accounts.iter().find(|a| a.label == label))
        .is_some_and(can_switch);
    inventory
        .accounts
        .iter()
        .filter(|a| a.enabled && a.auth_state != AuthState::SignedOut)
        .flat_map(|account| {
            account.models.iter().map(move |model| {
                let new_session = starts_new_session(
                    has_session,
                    current_account,
                    can_switch_now,
                    &account.label,
                );
                TaskModelOption {
                    account_label: account.label.clone(),
                    model_id: model.id.clone(),
                    model_name: model.name.clone(),
                    starts_new_session: new_session,
                    note: new_session.then(|| NEW_SESSION_NOTE.to_owned()),
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_account_on_a_harness_that_switches_keeps_the_session() {
        assert!(!starts_new_session(
            true,
            Some("Claude Work"),
            true,
            "Claude Work"
        ));
    }

    #[test]
    fn another_account_a_harness_that_cannot_switch_or_no_known_account_starts_a_new_session() {
        assert!(starts_new_session(true, Some("Claude Work"), true, "Codex"));
        assert!(starts_new_session(true, Some("Codex"), false, "Codex"));
        assert!(starts_new_session(true, None, true, "Codex"));
    }

    #[test]
    fn a_task_with_no_session_starts_nothing_new() {
        assert!(!starts_new_session(
            false,
            Some("Claude Work"),
            true,
            "Codex"
        ));
    }
}
