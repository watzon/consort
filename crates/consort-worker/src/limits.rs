//! The limits of this worker: the highest approval mode, the allowed folders
//! and shell access. The user changes them in the app. The hub forwards the
//! change, the worker saves it in `worker.db` and applies it at once, with no
//! restart.

use std::path::Path;
use std::sync::{Arc, RwLock};

use consort_proto::execution_link::PolicySet;

use crate::WorkerError;
use crate::config::PolicyConfig;
use crate::journal::Journal;

/// The limits that this worker uses now.
#[derive(Debug)]
pub struct Limits {
    journal: Arc<Journal>,
    current: RwLock<PolicyConfig>,
}

impl Limits {
    /// Reads the saved limits. A new worker has the defaults.
    pub fn load(journal: Arc<Journal>) -> Result<Self, WorkerError> {
        let current = journal.policy()?.unwrap_or_default();
        Ok(Self::starting_with(journal, current))
    }

    /// Limits that start with `current`, and that are not saved yet.
    pub fn starting_with(journal: Arc<Journal>, current: PolicyConfig) -> Self {
        Self {
            journal,
            current: RwLock::new(current),
        }
    }

    /// The limits now.
    pub fn get(&self) -> PolicyConfig {
        self.current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Saves a change and applies it. Nothing changes when the save fails.
    pub fn change(&self, change: PolicySet) -> Result<PolicyConfig, WorkerError> {
        let mut current = self.current.write().unwrap_or_else(|e| e.into_inner());
        let mut next = current.clone();
        if let Some(mode) = change.max_approval_mode {
            next.max_approval_mode = mode;
        }
        if let Some(roots) = change.allowed_roots {
            next.allowed_roots = checked_roots(roots)?;
        }
        if let Some(shell) = change.allow_shell {
            next.allow_shell = shell;
        }
        self.journal.put_policy(&next)?;
        *current = next.clone();
        Ok(next)
    }
}

/// The folders without duplicates. The folder may not exist yet. Only this
/// machine can say that a path is a full path, because the app may run on
/// another operating system.
fn checked_roots(roots: Vec<String>) -> Result<Vec<String>, WorkerError> {
    let mut checked: Vec<String> = Vec::with_capacity(roots.len());
    for root in roots {
        let root = root.trim().to_owned();
        let home = root == "~" || root.starts_with("~/") || root.starts_with("~\\");
        if !home && !Path::new(&root).is_absolute() {
            return Err(WorkerError::Workspace(format!(
                "`{root}` is not a full path on this worker. Enter the full path of the folder."
            )));
        }
        if !checked.contains(&root) {
            checked.push(root);
        }
    }
    Ok(checked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::model::ApprovalMode;

    fn full_path() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    #[test]
    fn a_change_applies_at_once_and_stays_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("worker.db");
        let limits = Limits::load(Arc::new(Journal::open(&db).unwrap())).unwrap();
        assert_eq!(limits.get(), PolicyConfig::default());
        let root = full_path();
        limits
            .change(PolicySet {
                max_approval_mode: Some(ApprovalMode::Ask),
                allowed_roots: Some(vec![root.clone(), root.clone()]),
                allow_shell: Some(false),
            })
            .unwrap();
        let wanted = PolicyConfig {
            max_approval_mode: ApprovalMode::Ask,
            allowed_roots: vec![root],
            allow_shell: false,
        };
        assert_eq!(limits.get(), wanted);
        drop(limits);
        let again = Limits::load(Arc::new(Journal::open(&db).unwrap())).unwrap();
        assert_eq!(again.get(), wanted);
    }

    #[test]
    fn a_missing_field_keeps_its_value() {
        let limits = Limits::load(Arc::new(Journal::in_memory().unwrap())).unwrap();
        limits
            .change(PolicySet {
                allow_shell: Some(false),
                ..PolicySet::default()
            })
            .unwrap();
        limits
            .change(PolicySet {
                max_approval_mode: Some(ApprovalMode::FullAccess),
                ..PolicySet::default()
            })
            .unwrap();
        assert!(!limits.get().allow_shell);
        assert_eq!(limits.get().max_approval_mode, ApprovalMode::FullAccess);
    }

    #[test]
    fn a_relative_folder_is_refused_and_changes_nothing() {
        let limits = Limits::load(Arc::new(Journal::in_memory().unwrap())).unwrap();
        let err = limits
            .change(PolicySet {
                allowed_roots: Some(vec!["code/project".to_owned()]),
                allow_shell: Some(false),
                ..PolicySet::default()
            })
            .unwrap_err();
        assert!(err.to_string().contains("not a full path"), "{err}");
        assert_eq!(limits.get(), PolicyConfig::default());
    }

    #[test]
    fn a_policy_block_in_an_old_worker_toml_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("worker.toml"),
            "[policy]\nmax_approval_mode = \"full_access\"\nallow_shell = false\n",
        )
        .unwrap();
        crate::config::WorkerToml::load(dir.path()).unwrap();
        let limits = Limits::load(Arc::new(
            Journal::open(&dir.path().join("worker.db")).unwrap(),
        ))
        .unwrap();
        assert_eq!(limits.get(), PolicyConfig::default());
    }
}
