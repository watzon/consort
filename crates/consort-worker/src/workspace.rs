//! Session workspaces, made with the `git` command.
//!
//! The worker uses the `git` program, not a git library, so a session gets
//! the user's own credentials, hooks and configuration. See
//! "Workspaces and local changes" in `docs/architecture.md`.
//!
//! The repo URL and the branch come from the hub, so the worker checks them
//! before it gives them to `git`:
//!
//! - A URL or a branch that starts with `-` is refused, and `--` (or
//!   `--end-of-options`) comes before them, so `git` never reads one as an
//!   option such as `--upload-pack=<command>`.
//! - A clone of a remote repo may use only `https`, `ssh` and `git`
//!   (`GIT_ALLOW_PROTOCOL`).
//! - A clone of a local path is allowed only inside the allowed roots of
//!   the worker policy.
//!
//! Each `git` command has a time limit, and the whole `prepare` has
//! [`PREPARE_LIMIT`]. When `prepare` fails, it removes what it made, so the
//! hub can try again.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::time::Instant;

use consort_proto::ids::TaskId;
use consort_proto::model::Isolation;
use consort_proto::worker_link::{WorkspaceRepo, WorkspaceSpec};

use crate::WorkerError;
use crate::config::{PolicyConfig, create_private_dir};
use crate::store::{WorkspaceEntry, WorkspaceRecord};

/// How long a clone may take.
const CLONE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How long the whole `prepare` may take, for all repos together. The hub
/// waits 60 minutes for `session.start` (`SESSION_START_TIMEOUT` in
/// `consort-hub`). The harness open and the first turn come after the
/// workspace. They take at most about 8 minutes (two ACP opens of 3 minutes
/// when Full access falls back to Auto), so this leaves a margin of at
/// least 10 minutes.
pub const PREPARE_LIMIT: Duration = Duration::from_secs(40 * 60);
/// How long any other `git` command may take.
const GIT_TIMEOUT: Duration = Duration::from_secs(2 * 60);
/// The protocols that a clone of a remote repo may use.
const REMOTE_PROTOCOLS: &str = "https:ssh:git";

/// A prepared workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The folder where the session runs.
    pub path: PathBuf,
    /// What the worker made.
    pub record: WorkspaceRecord,
}

/// Runs `git` and returns its stdout as bytes. `env` adds variables.
async fn git_with(
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<Vec<u8>, WorkerError> {
    git_with_input(args, cwd, env, timeout, None).await
}

/// Runs `git` with `input` on its stdin, or with no stdin, and returns its
/// stdout as bytes. `env` adds variables.
async fn git_with_input(
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    timeout: Duration,
    input: Option<&[u8]>,
) -> Result<Vec<u8>, WorkerError> {
    use tokio::io::AsyncWriteExt;

    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in env {
        cmd.env(key, value);
    }
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let run = async {
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        // Write the input while git runs, so a full pipe cannot block both
        // sides. A write error means that git stopped early, and its exit
        // status says why.
        let write = async {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                let _ = stdin.write_all(input).await;
            }
        };
        let ((), output) = tokio::join!(write, child.wait_with_output());
        output
    };
    // On a time-out the future drops, and `kill_on_drop` kills git.
    let output = tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| {
            WorkerError::Workspace(format!(
                "`git {}` did not finish in {} seconds",
                args.first().copied().unwrap_or_default(),
                timeout.as_secs()
            ))
        })?
        .map_err(|e| WorkerError::Workspace(format!("could not run git: {e}")))?;
    if !output.status.success() {
        return Err(WorkerError::Workspace(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

/// Runs `git` with the default time limit and returns its stdout as bytes.
async fn git(args: &[&str], cwd: Option<&Path>) -> Result<Vec<u8>, WorkerError> {
    git_with(args, cwd, &[], GIT_TIMEOUT).await
}

/// The time limit of one step of `prepare`: `limit`, or less when the
/// whole `prepare` has less time left.
fn step_limit(deadline: Instant, limit: Duration) -> Result<Duration, WorkerError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(WorkerError::Workspace(format!(
            "the workspace took longer than {} minutes to prepare. Use fewer or smaller repos, or a checkout on this worker",
            PREPARE_LIMIT.as_secs() / 60
        )));
    }
    Ok(limit.min(left))
}

/// Runs `git` within the time that `prepare` has left.
async fn git_until(
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    limit: Duration,
    deadline: Instant,
) -> Result<Vec<u8>, WorkerError> {
    git_with(args, cwd, env, step_limit(deadline, limit)?).await
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

/// Refuses a value from the hub that `git` could read as an option.
fn check_git_value(what: &str, value: &str) -> Result<(), WorkerError> {
    if value.trim().is_empty() {
        return Err(WorkerError::Workspace(format!("the {what} is empty")));
    }
    if value.starts_with('-') {
        return Err(WorkerError::Workspace(format!(
            "the {what} `{value}` starts with `-`, so git could read it as an option"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(WorkerError::Workspace(format!(
            "the {what} has a control character"
        )));
    }
    Ok(())
}

/// Where a clone comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneSource {
    /// A remote repo over `https`, `ssh` or `git`.
    Remote,
    /// A repo on this machine.
    Local(PathBuf),
}

/// Sorts a repo URL from the hub into a remote URL or a local path, and
/// refuses the rest: a URL that starts with `-`, another URL scheme (such as
/// `ext::` or `http://`), and a relative path.
pub fn clone_source(url: &str) -> Result<CloneSource, WorkerError> {
    check_git_value("repo URL", url)?;
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("file://") {
        // `Url` turns `file:///C:/repo` into `C:\repo` on Windows, and
        // decodes `%20` and the like.
        let path = url::Url::parse(url)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .ok_or_else(|| {
                WorkerError::Workspace(format!(
                    "the repo URL `{url}` is not a file URL of this machine"
                ))
            })?;
        return Ok(CloneSource::Local(path));
    }
    if ["https://", "ssh://", "git://", "git+ssh://", "ssh+git://"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
    {
        return Ok(CloneSource::Remote);
    }
    if url.contains("://") || url.contains("::") {
        return Err(WorkerError::Workspace(format!(
            "the repo URL `{url}` uses a protocol that this worker does not allow. Use https, ssh or git"
        )));
    }
    let path = Path::new(url);
    if path.is_absolute() {
        return Ok(CloneSource::Local(path.to_path_buf()));
    }
    // The scp form of ssh: `[user@]host:path`, with no `/` before the colon.
    if let Some(colon) = url.find(':')
        && colon > 0
        && !url[..colon].contains('/')
    {
        return Ok(CloneSource::Remote);
    }
    Err(WorkerError::Workspace(format!(
        "the repo URL `{url}` is a relative path. Use an absolute path or an https, ssh or git URL"
    )))
}

/// A path from the NUL-separated output of `git ls-files -z`.
#[cfg(unix)]
fn path_from_git(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

/// A path from the NUL-separated output of `git ls-files -z`. Git writes
/// UTF-8 paths on Windows.
#[cfg(not(unix))]
fn path_from_git(bytes: &[u8]) -> Option<PathBuf> {
    std::str::from_utf8(bytes).ok().map(PathBuf::from)
}

/// Copies one untracked file. A link stays a link.
async fn copy_untracked(from: &Path, to: &Path) -> Result<(), WorkerError> {
    let Ok(meta) = tokio::fs::symlink_metadata(from).await else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        {
            let target = tokio::fs::read_link(from)
                .await
                .map_err(|e| WorkerError::io(format!("could not read {}", from.display()), e))?;
            tokio::fs::symlink(&target, to)
                .await
                .map_err(|e| WorkerError::io(format!("could not copy {}", from.display()), e))?;
        }
        return Ok(());
    }
    if meta.is_file() {
        tokio::fs::copy(from, to)
            .await
            .map_err(|e| WorkerError::io(format!("could not copy {}", from.display()), e))?;
    }
    Ok(())
}

async fn carry_changes(checkout: &Path, dest: &Path, deadline: Instant) -> Result<(), WorkerError> {
    // The patch stays bytes from end to end, so files that are not UTF-8
    // keep their content.
    let diff = git_until(
        &["diff", "--binary", "HEAD"],
        Some(checkout),
        &[],
        GIT_TIMEOUT,
        deadline,
    )
    .await?;
    if !diff.iter().all(u8::is_ascii_whitespace) {
        // The patch goes to `git apply` on stdin, not through a file. A
        // file in the worktree could be a link that the repo commits, and
        // then the worker would write the patch somewhere else.
        git_with_input(
            &["apply", "--binary", "--whitespace=nowarn", "-"],
            Some(dest),
            &[],
            step_limit(deadline, GIT_TIMEOUT)?,
            Some(&diff),
        )
        .await?;
    }
    let untracked = git_until(
        &["ls-files", "--others", "--exclude-standard", "-z"],
        Some(checkout),
        &[],
        GIT_TIMEOUT,
        deadline,
    )
    .await?;
    for rel in untracked.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let Some(rel) = path_from_git(rel) else {
            tracing::warn!(
                name = %String::from_utf8_lossy(rel),
                "an untracked file name is not UTF-8; the worker does not copy it"
            );
            continue;
        };
        let from = checkout.join(&rel);
        let to = dest.join(&rel);
        if let Some(parent) = to.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| WorkerError::io("could not copy an untracked file", e))?;
        }
        copy_untracked(&from, &to).await?;
    }
    Ok(())
}

/// Removes what one workspace entry made. A checkout used in place stays.
async fn remove_entry(entry: &WorkspaceEntry) {
    match entry {
        WorkspaceEntry::InPlace { .. } => {}
        WorkspaceEntry::Worktree { checkout, path } => {
            if let Err(err) = git(
                &["worktree", "remove", "--force", "--end-of-options", path],
                Some(Path::new(checkout)),
            )
            .await
            {
                tracing::debug!(error = %err, "git worktree remove failed; removing the folder");
            }
            let _ = tokio::fs::remove_dir_all(path).await;
            let _ = git(&["worktree", "prune"], Some(Path::new(checkout))).await;
        }
        WorkspaceEntry::Clone { path } | WorkspaceEntry::Empty { path } => {
            let _ = tokio::fs::remove_dir_all(path).await;
        }
    }
}

/// Removes the leftovers of an earlier attempt that stopped before the
/// worker saved its record, for example in a crash. The root belongs to the
/// worker. A stale worktree entry in a checkout is pruned.
async fn clear_leftovers(root: &Path, spec: &WorkspaceSpec) {
    if tokio::fs::symlink_metadata(root).await.is_err() {
        return;
    }
    tracing::info!(root = %root.display(), "removing an earlier workspace attempt");
    let _ = tokio::fs::remove_dir_all(root).await;
    if spec.isolation == Isolation::Worktree {
        for repo in &spec.repos {
            if let Some(checkout) = repo.checkout_path.as_deref()
                && Path::new(checkout).is_dir()
            {
                let _ = git(&["worktree", "prune"], Some(Path::new(checkout))).await;
            }
        }
    }
}

async fn make_root(root: &Path) -> Result<(), WorkerError> {
    if let Some(parent) = root.parent() {
        create_private_dir(parent)
            .map_err(|e| WorkerError::io(format!("could not create {}", parent.display()), e))?;
    }
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|e| WorkerError::io(format!("could not create {}", root.display()), e))
}

/// Makes one repo of a workspace and adds its entry to `entries`, also
/// when a later step fails, so that the caller can remove it.
async fn prepare_repo(
    root: &Path,
    repo: &WorkspaceRepo,
    spec: &WorkspaceSpec,
    policy: &PolicyConfig,
    entries: &mut Vec<WorkspaceEntry>,
    deadline: Instant,
) -> Result<PathBuf, WorkerError> {
    if let Some(branch) = &repo.branch {
        check_git_value("branch", branch)?;
    }
    match spec.isolation {
        Isolation::InPlace | Isolation::Worktree => {
            let checkout = PathBuf::from(repo.checkout_path.as_deref().unwrap_or_default());
            if !checkout.is_dir() {
                return Err(WorkerError::Workspace(format!(
                    "the checkout {} does not exist",
                    checkout.display()
                )));
            }
            check_allowed(&checkout, policy)?;
            if spec.isolation == Isolation::InPlace {
                entries.push(WorkspaceEntry::InPlace {
                    path: checkout.to_string_lossy().into_owned(),
                });
                return Ok(checkout);
            }
            make_root(root).await?;
            let dest = root.join(repo_folder_name(repo));
            let dest_text = dest.to_string_lossy().into_owned();
            let target = repo.branch.as_deref().unwrap_or("HEAD");
            git_until(
                &[
                    "worktree",
                    "add",
                    "--detach",
                    "--end-of-options",
                    &dest_text,
                    target,
                ],
                Some(&checkout),
                &[],
                GIT_TIMEOUT,
                deadline,
            )
            .await?;
            entries.push(WorkspaceEntry::Worktree {
                checkout: checkout.to_string_lossy().into_owned(),
                path: dest_text,
            });
            if spec.carry_uncommitted {
                carry_changes(&checkout, &dest, deadline).await?;
            }
            Ok(dest)
        }
        Isolation::Clone => {
            let protocols = match clone_source(&repo.url)? {
                CloneSource::Remote => REMOTE_PROTOCOLS,
                CloneSource::Local(path) => {
                    if !path.is_dir() {
                        return Err(WorkerError::Workspace(format!(
                            "the repo {} does not exist on this worker",
                            path.display()
                        )));
                    }
                    check_allowed(&path, policy)?;
                    "file"
                }
            };
            make_root(root).await?;
            let dest = root.join(repo_folder_name(repo));
            let dest_text = dest.to_string_lossy().into_owned();
            let mut args = vec!["clone"];
            if let Some(branch) = &repo.branch {
                args.push("--branch");
                args.push(branch);
            }
            args.push("--");
            args.push(&repo.url);
            args.push(&dest_text);
            // Record the clone first: git can leave a partial folder.
            entries.push(WorkspaceEntry::Clone {
                path: dest_text.clone(),
            });
            git_until(
                &args,
                None,
                &[("GIT_ALLOW_PROTOCOL", protocols)],
                CLONE_TIMEOUT,
                deadline,
            )
            .await?;
            Ok(dest)
        }
    }
}

/// Makes the workspace of a session under `workspaces_dir/<key>`, in at
/// most [`PREPARE_LIMIT`]. On an error, it removes what it made.
pub async fn prepare(
    workspaces_dir: &Path,
    key: &str,
    task_id: Option<TaskId>,
    spec: &WorkspaceSpec,
    policy: &PolicyConfig,
) -> Result<Prepared, WorkerError> {
    prepare_within(workspaces_dir, key, task_id, spec, policy, PREPARE_LIMIT).await
}

/// [`prepare`] with another time limit, for tests.
async fn prepare_within(
    workspaces_dir: &Path,
    key: &str,
    task_id: Option<TaskId>,
    spec: &WorkspaceSpec,
    policy: &PolicyConfig,
    limit: Duration,
) -> Result<Prepared, WorkerError> {
    let deadline = Instant::now() + limit;
    spec.validate()
        .map_err(|e| WorkerError::Workspace(e.to_string()))?;
    let root = workspaces_dir.join(key);
    clear_leftovers(&root, spec).await;

    if spec.repos.is_empty() {
        make_root(&root).await?;
        return Ok(Prepared {
            path: root.clone(),
            record: WorkspaceRecord {
                task_id,
                root: root.to_string_lossy().into_owned(),
                entries: vec![WorkspaceEntry::Empty {
                    path: root.to_string_lossy().into_owned(),
                }],
            },
        });
    }

    let mut entries = Vec::new();
    let mut paths = Vec::new();
    for repo in &spec.repos {
        match prepare_repo(&root, repo, spec, policy, &mut entries, deadline).await {
            Ok(path) => paths.push(path),
            Err(err) => {
                for entry in entries.iter().rev() {
                    remove_entry(entry).await;
                }
                if spec.isolation != Isolation::InPlace {
                    let _ = tokio::fs::remove_dir_all(&root).await;
                }
                return Err(err);
            }
        }
    }
    let path = match paths.as_slice() {
        [only] => only.clone(),
        [first, ..] if spec.isolation == Isolation::InPlace => first.clone(),
        _ => root.clone(),
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
        if matches!(entry, WorkspaceEntry::InPlace { .. }) {
            in_place = true;
        }
        remove_entry(entry).await;
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
        // Keep line endings as they are. Without this, a user's
        // `core.autocrlf = true` (common on Windows) changes LF to CRLF in
        // the checkouts, and the byte checks below fail.
        std::fs::write(repo.join(".gitattributes"), "* -text\n").unwrap();
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
        let list = String::from_utf8_lossy(&list);
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
    fn clone_sources_are_sorted_and_checked() {
        let remote = [
            "https://github.com/watzon/sayso.git",
            "ssh://git@github.com/watzon/sayso.git",
            "git://example.com/repo.git",
            "git@github.com:watzon/sayso.git",
            "github.com:watzon/sayso",
        ];
        for url in remote {
            assert_eq!(clone_source(url).unwrap(), CloneSource::Remote, "{url}");
        }
        #[cfg(unix)]
        {
            assert_eq!(
                clone_source("file:///srv/repo").unwrap(),
                CloneSource::Local(PathBuf::from("/srv/repo"))
            );
            assert_eq!(
                clone_source("file:///srv/my%20repo").unwrap(),
                CloneSource::Local(PathBuf::from("/srv/my repo"))
            );
            // A file URL of another host is not a path on this machine.
            assert!(clone_source("file://server/share/repo").is_err());
        }
        #[cfg(windows)]
        assert_eq!(
            clone_source("file:///C:/repo").unwrap(),
            CloneSource::Local(PathBuf::from(r"C:\repo"))
        );
        #[cfg(unix)]
        assert_eq!(
            clone_source("/srv/repo").unwrap(),
            CloneSource::Local(PathBuf::from("/srv/repo"))
        );
        for bad in [
            "--upload-pack=touch /tmp/x",
            "-uevil",
            "ext::sh -c touch% /tmp/x",
            "http://example.com/repo.git",
            "fd::7",
            "relative/path",
            "",
            "https://example.com/a\nb",
        ] {
            assert!(clone_source(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn hub_values_never_become_git_options() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_changes(dir.path()).await;
        let workspaces = dir.path().join("workspaces");
        let marker = dir.path().join("pwned");
        let policy = PolicyConfig::default();

        // The review's proof: this URL ran a command through --upload-pack.
        let mut evil = spec(&repo, Isolation::Clone, false);
        evil.repos[0].url = format!("--upload-pack=touch {}", marker.display());
        let err = prepare(&workspaces, "tsk_x", None, &evil, &policy)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("starts with `-`"), "{err}");
        assert!(!marker.exists(), "git ran a command from the URL");

        let mut evil = spec(&repo, Isolation::Clone, false);
        evil.repos[0].branch = Some(format!("--upload-pack=touch {}", marker.display()));
        assert!(
            prepare(&workspaces, "tsk_y", None, &evil, &policy)
                .await
                .is_err()
        );
        let mut evil = spec(&repo, Isolation::Worktree, false);
        evil.repos[0].branch = Some("--orphan".into());
        assert!(
            prepare(&workspaces, "tsk_z", None, &evil, &policy)
                .await
                .is_err()
        );
        assert!(!marker.exists());
        assert!(!workspaces.join("tsk_x").exists());
    }

    #[tokio::test]
    async fn local_clones_stay_inside_the_allowed_roots() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_changes(dir.path()).await;
        let workspaces = dir.path().join("workspaces");
        let elsewhere = tempfile::tempdir().unwrap();
        let policy = PolicyConfig {
            allowed_roots: vec![elsewhere.path().to_string_lossy().into_owned()],
            ..PolicyConfig::default()
        };
        let mut as_file_url = spec(&repo, Isolation::Clone, false);
        as_file_url.repos[0].url = format!("file://{}", repo.display());
        for (key, s) in [
            ("tsk_p", spec(&repo, Isolation::Clone, false)),
            ("tsk_q", as_file_url),
        ] {
            let err = prepare(&workspaces, key, None, &s, &policy)
                .await
                .unwrap_err();
            assert!(matches!(err, WorkerError::Policy(_)), "{err}");
            assert!(!workspaces.join(key).exists());
        }
        let allowed = PolicyConfig {
            allowed_roots: vec![dir.path().to_string_lossy().into_owned()],
            ..PolicyConfig::default()
        };
        let cloned = prepare(
            &workspaces,
            "tsk_r",
            None,
            &spec(&repo, Isolation::Clone, false),
            &allowed,
        )
        .await
        .unwrap();
        assert!(cloned.path.join("a.txt").exists());
    }

    #[tokio::test]
    async fn a_failed_prepare_removes_what_it_made_so_a_retry_works() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_changes(dir.path()).await;
        let other = dir.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        git(&["init", "-q", "-b", "main"], Some(&other))
            .await
            .unwrap();
        git(
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=T",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
            Some(&other),
        )
        .await
        .unwrap();
        let workspaces = dir.path().join("workspaces");
        let policy = PolicyConfig::default();
        let two = |second: &Path| {
            let mut s = spec(&repo, Isolation::Clone, false);
            let mut next = s.repos[0].clone();
            next.repo_id = RepoId::new();
            next.url = second.to_string_lossy().into_owned();
            s.repos.push(next);
            s
        };
        // The first clone works and the second fails.
        let missing = dir.path().join("missing");
        assert!(
            prepare(&workspaces, "tsk_m", None, &two(&missing), &policy)
                .await
                .is_err()
        );
        assert!(
            !workspaces.join("tsk_m").exists(),
            "the first clone is gone"
        );
        // Before the fix, the retry failed: the first clone "already exists".
        let ok = prepare(&workspaces, "tsk_m", None, &two(&other), &policy)
            .await
            .unwrap();
        assert_eq!(ok.record.entries.len(), 2);

        // Leftovers of a crashed attempt, which has no record, go too.
        let crashed = workspaces.join("tsk_n").join("checkout");
        std::fs::create_dir_all(&crashed).unwrap();
        std::fs::write(crashed.join("partial"), "x").unwrap();
        prepare(
            &workspaces,
            "tsk_n",
            None,
            &spec(&repo, Isolation::Clone, false),
            &policy,
        )
        .await
        .unwrap();
        assert!(!crashed.join("partial").exists());
    }

    #[tokio::test]
    async fn carried_changes_keep_bytes_that_are_not_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_changes(dir.path()).await;
        // Latin-1 text, as in the review's proof: `caf\xe9 one`.
        std::fs::write(repo.join("latin1.txt"), b"caf\xe9 one\n").unwrap();
        git(&["add", "latin1.txt"], Some(&repo)).await.unwrap();
        git(&["commit", "-q", "-m", "latin1"], Some(&repo))
            .await
            .unwrap();
        let changed: &[u8] = b"caf\xe9 one\ncaf\xe9 two\n";
        std::fs::write(repo.join("latin1.txt"), changed).unwrap();
        std::fs::write(repo.join("raw.bin"), [0xff_u8, 0xfe, 0x00, 0x80]).unwrap();
        let carried = prepare(
            &dir.path().join("workspaces"),
            "tsk_l",
            None,
            &spec(&repo, Isolation::Worktree, true),
            &PolicyConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(carried.path.join("latin1.txt")).unwrap(),
            changed
        );
        assert_eq!(
            std::fs::read(carried.path.join("raw.bin")).unwrap(),
            vec![0xff, 0xfe, 0x00, 0x80]
        );
    }

    /// A repo can commit a link with any name. The carry must not write
    /// through it to a file outside the worktree.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_carry_does_not_write_through_a_link_in_the_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_changes(dir.path()).await;
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "keep\n").unwrap();
        for name in [".consort-carry.patch", "carry.patch"] {
            std::os::unix::fs::symlink(&outside, repo.join(name)).unwrap();
        }
        git(&["add", ".consort-carry.patch", "carry.patch"], Some(&repo))
            .await
            .unwrap();
        git(&["commit", "-q", "-m", "links"], Some(&repo))
            .await
            .unwrap();
        let carried = prepare(
            &dir.path().join("workspaces"),
            "tsk_s",
            None,
            &spec(&repo, Isolation::Worktree, true),
            &PolicyConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep\n");
        assert_eq!(
            std::fs::read_to_string(carried.path.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
        assert_eq!(
            std::fs::read_to_string(carried.path.join("new/b.txt")).unwrap(),
            "untracked\n"
        );
    }

    /// File names that are not UTF-8 exist on Linux file systems. macOS
    /// refuses them, and Windows uses UTF-16 names.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn untracked_files_with_names_that_are_not_utf8_are_carried() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_changes(dir.path()).await;
        let name = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
        std::fs::write(repo.join(name), "x").unwrap();
        let carried = prepare(
            &dir.path().join("workspaces"),
            "tsk_u",
            None,
            &spec(&repo, Isolation::Worktree, true),
            &PolicyConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(carried.path.join(name)).unwrap(), b"x");
    }

    #[tokio::test]
    async fn prepare_has_a_time_limit_for_all_repos() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_changes(dir.path()).await;
        let workspaces = dir.path().join("workspaces");
        let err = prepare_within(
            &workspaces,
            "tsk_t",
            None,
            &spec(&repo, Isolation::Clone, false),
            &PolicyConfig::default(),
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("longer than 40 minutes"), "{err}");
        assert!(!workspaces.join("tsk_t").exists(), "nothing is left");
    }

    #[tokio::test]
    async fn git_commands_have_a_time_limit() {
        let dir = tempfile::tempdir().unwrap();
        let err = git_with(&["init", "-q"], Some(dir.path()), &[], Duration::ZERO)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("did not finish"), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_workspaces_folder_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let workspaces = dir.path().join("workspaces");
        prepare(
            &workspaces,
            "ses_p",
            None,
            &WorkspaceSpec {
                repos: vec![],
                isolation: Isolation::Clone,
                carry_uncommitted: false,
            },
            &PolicyConfig::default(),
        )
        .await
        .unwrap();
        let mode = std::fs::metadata(&workspaces).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
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
