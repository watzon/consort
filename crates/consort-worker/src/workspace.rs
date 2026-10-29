//! Session workspaces, made with the `git` command.
//!
//! The worker uses the `git` program, not a git library, so a session gets
//! the user's own credentials, hooks and configuration. See
//! "Workspaces and local changes" in `docs/architecture.md`.

use std::path::{Path, PathBuf};

use consort_proto::ids::TaskId;
use consort_proto::model::Isolation;
use consort_proto::worker_link::{WorkspaceRepo, WorkspaceSpec};

use crate::WorkerError;
use crate::config::PolicyConfig;
use crate::store::{WorkspaceEntry, WorkspaceRecord};

/// A prepared workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The folder where the session runs.
    pub path: PathBuf,
    /// What the worker made.
    pub record: WorkspaceRecord,
}

async fn git(args: &[&str], cwd: Option<&Path>) -> Result<String, WorkerError> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let output = cmd
        .output()
        .await
        .map_err(|e| WorkerError::Workspace(format!("could not run git: {e}")))?;
    if !output.status.success() {
        return Err(WorkerError::Workspace(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A folder name for a repo, from the last part of its URL.
pub fn repo_folder_name(repo: &WorkspaceRepo) -> String {
    let last = repo
        .url
        .trim_end_matches('/')
        .rsplit(['/', ':', '\\'])
        .next()
        .unwrap_or_default()
        .trim_end_matches(".git");
    let clean: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if clean.is_empty() || clean == "." || clean == ".." {
        repo.repo_id.to_string()
    } else {
        clean
    }
}

/// Checks that a folder is inside one of the allowed roots. An empty list
/// allows every folder.
pub fn check_allowed(path: &Path, policy: &PolicyConfig) -> Result<(), WorkerError> {
    if policy.allowed_roots.is_empty() {
        return Ok(());
    }
    let real = std::fs::canonicalize(path)
        .map_err(|e| WorkerError::Workspace(format!("{} cannot be used: {e}", path.display())))?;
    for root in &policy.allowed_roots {
        let root = consort_harness::account::expand_home(root);
        if let Ok(root) = std::fs::canonicalize(&root)
            && real.starts_with(&root)
        {
            return Ok(());
        }
    }
    Err(WorkerError::Policy(format!(
        "{} is outside the allowed roots of this worker (policy.allowed_roots)",
        path.display()
    )))
}

async fn carry_changes(checkout: &Path, dest: &Path) -> Result<(), WorkerError> {
    let diff = git(&["diff", "--binary", "HEAD"], Some(checkout)).await?;
    if !diff.trim().is_empty() {
        let patch = dest.join(".consort-carry.patch");
        tokio::fs::write(&patch, diff.as_bytes())
            .await
            .map_err(|e| WorkerError::io("could not write the carry patch", e))?;
        let result = git(
            &[
                "apply",
                "--binary",
                "--whitespace=nowarn",
                ".consort-carry.patch",
            ],
            Some(dest),
        )
        .await;
        let _ = tokio::fs::remove_file(&patch).await;
        result?;
    }
    let untracked = git(
        &["ls-files", "--others", "--exclude-standard", "-z"],
        Some(checkout),
    )
    .await?;
    for rel in untracked.split('\0').filter(|s| !s.is_empty()) {
        let from = checkout.join(rel);
        let to = dest.join(rel);
        if let Some(parent) = to.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| WorkerError::io("could not copy an untracked file", e))?;
        }
        if from.is_file() {
            tokio::fs::copy(&from, &to)
                .await
                .map_err(|e| WorkerError::io(format!("could not copy {}", from.display()), e))?;
        }
    }
    Ok(())
}

/// Makes the workspace of a session under `workspaces_dir/<key>`.
pub async fn prepare(
    workspaces_dir: &Path,
    key: &str,
    task_id: Option<TaskId>,
    spec: &WorkspaceSpec,
    policy: &PolicyConfig,
) -> Result<Prepared, WorkerError> {
    spec.validate()
        .map_err(|e| WorkerError::Workspace(e.to_string()))?;
    let root = workspaces_dir.join(key);
    let mut entries = Vec::new();
    let mut paths = Vec::new();

    if spec.repos.is_empty() {
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| WorkerError::io(format!("could not create {}", root.display()), e))?;
        entries.push(WorkspaceEntry::Empty {
            path: root.to_string_lossy().into_owned(),
        });
        return Ok(Prepared {
            path: root.clone(),
            record: WorkspaceRecord {
                task_id,
                root: root.to_string_lossy().into_owned(),
                entries,
            },
        });
    }

    for repo in &spec.repos {
        match spec.isolation {
            Isolation::InPlace => {
                let checkout = PathBuf::from(repo.checkout_path.as_deref().unwrap_or_default());
                if !checkout.is_dir() {
                    return Err(WorkerError::Workspace(format!(
                        "the checkout {} does not exist",
                        checkout.display()
                    )));
                }
                check_allowed(&checkout, policy)?;
                entries.push(WorkspaceEntry::InPlace {
                    path: checkout.to_string_lossy().into_owned(),
                });
                paths.push(checkout);
            }
            Isolation::Worktree => {
                let checkout = PathBuf::from(repo.checkout_path.as_deref().unwrap_or_default());
                if !checkout.is_dir() {
                    return Err(WorkerError::Workspace(format!(
                        "the checkout {} does not exist",
                        checkout.display()
                    )));
                }
                check_allowed(&checkout, policy)?;
                tokio::fs::create_dir_all(&root).await.map_err(|e| {
                    WorkerError::io(format!("could not create {}", root.display()), e)
                })?;
                let dest = root.join(repo_folder_name(repo));
                let dest_text = dest.to_string_lossy().into_owned();
                let target = repo.branch.as_deref().unwrap_or("HEAD");
                git(
                    &["worktree", "add", "--detach", &dest_text, target],
                    Some(&checkout),
                )
                .await?;
                entries.push(WorkspaceEntry::Worktree {
                    checkout: checkout.to_string_lossy().into_owned(),
                    path: dest_text,
                });
                if spec.carry_uncommitted {
                    carry_changes(&checkout, &dest).await?;
                }
                paths.push(dest);
            }
            Isolation::Clone => {
                tokio::fs::create_dir_all(&root).await.map_err(|e| {
                    WorkerError::io(format!("could not create {}", root.display()), e)
                })?;
                let dest = root.join(repo_folder_name(repo));
                let dest_text = dest.to_string_lossy().into_owned();
                let mut args = vec!["clone"];
                if let Some(branch) = &repo.branch {
                    args.push("--branch");
                    args.push(branch);
                }
                args.push(&repo.url);
                args.push(&dest_text);
                git(&args, None).await?;
                entries.push(WorkspaceEntry::Clone { path: dest_text });
                paths.push(dest);
            }
        }
    }
    let path = if paths.len() == 1 || spec.isolation == Isolation::InPlace {
        paths[0].clone()
    } else {
        root.clone()
    };
    Ok(Prepared {
        path,
        record: WorkspaceRecord {
            task_id,
            root: root.to_string_lossy().into_owned(),
            entries,
        },
    })
}

/// Releases a workspace. With `delete`, removes worktrees and clones. A
/// checkout used in place is never deleted.
pub async fn release(record: &WorkspaceRecord, delete: bool) -> Result<(), WorkerError> {
    if !delete {
        return Ok(());
    }
    let mut in_place = false;
    for entry in &record.entries {
        match entry {
            WorkspaceEntry::InPlace { .. } => in_place = true,
            WorkspaceEntry::Worktree { checkout, path } => {
                if let Err(err) = git(
                    &["worktree", "remove", "--force", path],
                    Some(Path::new(checkout)),
                )
                .await
                {
                    tracing::warn!(error = %err, "git worktree remove failed; removing the folder");
                }
                let _ = tokio::fs::remove_dir_all(path).await;
                let _ = git(&["worktree", "prune"], Some(Path::new(checkout))).await;
            }
            WorkspaceEntry::Clone { path } | WorkspaceEntry::Empty { path } => {
                let _ = tokio::fs::remove_dir_all(path).await;
            }
        }
    }
    if !in_place {
        let _ = tokio::fs::remove_dir_all(&record.root).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::ids::RepoId;

    async fn repo_with_changes(dir: &Path) -> PathBuf {
        let repo = dir.join("checkout");
        std::fs::create_dir_all(&repo).unwrap();
        git(&["init", "-q", "-b", "main"], Some(&repo))
            .await
            .unwrap();
        git(&["config", "user.email", "test@example.com"], Some(&repo))
            .await
            .unwrap();
        git(&["config", "user.name", "Test"], Some(&repo))
            .await
            .unwrap();
        git(&["config", "commit.gpgsign", "false"], Some(&repo))
            .await
            .unwrap();
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&["add", "."], Some(&repo)).await.unwrap();
        git(&["commit", "-q", "-m", "first"], Some(&repo))
            .await
            .unwrap();
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::create_dir_all(repo.join("new")).unwrap();
        std::fs::write(repo.join("new/b.txt"), "untracked\n").unwrap();
        repo
    }

    fn spec(repo: &Path, isolation: Isolation, carry: bool) -> WorkspaceSpec {
        WorkspaceSpec {
            repos: vec![WorkspaceRepo {
                repo_id: RepoId::new(),
                url: repo.to_string_lossy().into_owned(),
                branch: None,
                checkout_path: (isolation != Isolation::Clone)
                    .then(|| repo.to_string_lossy().into_owned()),
            }],
            isolation,
            carry_uncommitted: carry,
        }
    }

    #[tokio::test]
    async fn worktree_with_and_without_uncommitted_changes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_changes(dir.path()).await;
        let workspaces = dir.path().join("workspaces");
        let policy = PolicyConfig::default();

        let carried = prepare(
            &workspaces,
            "tsk_a",
            None,
            &spec(&repo, Isolation::Worktree, true),
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(carried.path.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
        assert_eq!(
            std::fs::read_to_string(carried.path.join("new/b.txt")).unwrap(),
            "untracked\n"
        );
        // The user's checkout keeps its changes.
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );

        let clean = prepare(
            &workspaces,
            "tsk_b",
            None,
            &spec(&repo, Isolation::Worktree, false),
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(clean.path.join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(!clean.path.join("new/b.txt").exists());

        release(&carried.record, true).await.unwrap();
        assert!(!carried.path.exists());
        let list = git(&["worktree", "list"], Some(&repo)).await.unwrap();
        assert!(!list.contains("tsk_a"), "{list}");
        release(&clean.record, true).await.unwrap();
    }

    #[tokio::test]
    async fn clone_in_place_and_empty() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_changes(dir.path()).await;
        let workspaces = dir.path().join("workspaces");
        let policy = PolicyConfig::default();

        let cloned = prepare(
            &workspaces,
            "tsk_c",
            None,
            &spec(&repo, Isolation::Clone, false),
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(cloned.path, workspaces.join("tsk_c").join("checkout"));
        assert_eq!(
            std::fs::read_to_string(cloned.path.join("a.txt")).unwrap(),
            "one\n"
        );

        let in_place = prepare(
            &workspaces,
            "tsk_d",
            None,
            &spec(&repo, Isolation::InPlace, false),
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(in_place.path, repo);
        release(&in_place.record, true).await.unwrap();
        assert!(
            repo.join("a.txt").exists(),
            "a checkout used in place is never deleted"
        );

        let empty = prepare(
            &workspaces,
            "ses_e",
            None,
            &WorkspaceSpec {
                repos: vec![],
                isolation: Isolation::Clone,
                carry_uncommitted: false,
            },
            &policy,
        )
        .await
        .unwrap();
        assert!(empty.path.is_dir());
        release(&empty.record, true).await.unwrap();
        assert!(!empty.path.exists());

        release(&cloned.record, false).await.unwrap();
        assert!(cloned.path.exists());
        release(&cloned.record, true).await.unwrap();
        assert!(!cloned.path.exists());
    }

    #[tokio::test]
    async fn policy_roots_and_invalid_specs() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_changes(dir.path()).await;
        let workspaces = dir.path().join("workspaces");
        let elsewhere = tempfile::tempdir().unwrap();
        let policy = PolicyConfig {
            allowed_roots: vec![elsewhere.path().to_string_lossy().into_owned()],
            ..PolicyConfig::default()
        };
        let err = prepare(
            &workspaces,
            "tsk_f",
            None,
            &spec(&repo, Isolation::InPlace, false),
            &policy,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, WorkerError::Policy(_)), "{err}");
        let allowed = PolicyConfig {
            allowed_roots: vec![dir.path().to_string_lossy().into_owned()],
            ..PolicyConfig::default()
        };
        prepare(
            &workspaces,
            "tsk_g",
            None,
            &spec(&repo, Isolation::InPlace, false),
            &allowed,
        )
        .await
        .unwrap();
        let bad = spec(&repo, Isolation::Clone, true);
        assert!(
            prepare(&workspaces, "tsk_h", None, &bad, &PolicyConfig::default())
                .await
                .is_err()
        );
    }

    #[test]
    fn folder_names() {
        let r = |url: &str| WorkspaceRepo {
            repo_id: RepoId::new(),
            url: url.into(),
            branch: None,
            checkout_path: None,
        };
        assert_eq!(
            repo_folder_name(&r("git@github.com:watzon/sayso.git")),
            "sayso"
        );
        assert_eq!(repo_folder_name(&r("https://github.com/a/b/")), "b");
        assert!(repo_folder_name(&r("..")).starts_with("rep_"));
    }
}
