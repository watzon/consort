//! Turn attachments and the channel file copy, against the fake hub
//! (`common/mod.rs`) and the fake harness. The worker downloads each file
//! with the signed blob GET.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{CODE, FakeHub};

use consort_harness::attachments::attachment_path;
use consort_harness::{AccountConfig, Harness, HarnessKind};
use consort_proto::ids::{AccountId, ChannelId, SessionId};
use consort_proto::model::{ApprovalMode, BlobRef, FileRef, Isolation, SessionRole, Sha256Hash};
use consort_proto::worker_link::{
    SessionEvent, SessionStart, SessionStartResult, SessionSteer, SessionStop, SessionTurn,
    TurnInput, WorkspaceSpec,
};
use consort_testkit::{FakeHarness, FakeRecord, FakeScript};
use consort_worker::config::WorkerToml;
use consort_worker::{Harnesses, WorkerConfig, WorkerHandle};
use sha2::{Digest, Sha256};

struct Rig {
    hub: FakeHub,
    dir: tempfile::TempDir,
    _worker: WorkerHandle,
    account_id: AccountId,
    record: Arc<std::sync::Mutex<FakeRecord>>,
}

async fn rig() -> Rig {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    consort_worker::pair(&hub.url, CODE, dir.path(), None, vec![])
        .await
        .unwrap();
    let mut toml = WorkerToml::load(dir.path()).unwrap();
    let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
    let account_id = account.id;
    toml.accounts = vec![account];
    toml.save(dir.path()).unwrap();
    let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Seen."));
    let record = fake.record();
    let mut harnesses: Harnesses = HashMap::new();
    harnesses.insert(HarnessKind::ClaudeCode, Arc::new(fake) as Arc<dyn Harness>);
    let worker = consort_worker::run_with(
        WorkerConfig {
            config_dir: dir.path().to_path_buf(),
            consort_home: Some(dir.path().join("home")),
            check_tool_updates: false,
        },
        harnesses,
    )
    .await
    .unwrap();
    hub.wait_connected().await;
    Rig {
        hub,
        dir,
        _worker: worker,
        account_id,
        record,
    }
}

fn sha(bytes: &[u8]) -> Sha256Hash {
    Sha256Hash::from_bytes(Sha256::digest(bytes).into())
}

impl Rig {
    /// Serves `bytes` as the blob with hash `hash`.
    fn serve_as(&self, hash: &Sha256Hash, bytes: &[u8]) {
        self.hub
            .state
            .blob_bodies
            .lock()
            .unwrap()
            .insert(hash.to_string(), bytes.to_vec());
    }

    fn serve(&self, bytes: &[u8]) -> Sha256Hash {
        let hash = sha(bytes);
        self.serve_as(&hash, bytes);
        hash
    }

    fn gets(&self) -> usize {
        self.hub.state.blob_gets.lock().unwrap().len()
    }

    fn start(
        &self,
        session_id: SessionId,
        channel_files: Vec<FileRef>,
        turn: TurnInput,
    ) -> SessionStart {
        SessionStart {
            session_id,
            role: SessionRole::Coordinator,
            channel_id: ChannelId::new(),
            task_id: None,
            account_id: self.account_id,
            model: "fake-model".into(),
            options: Default::default(),
            approval_mode: ApprovalMode::Auto,
            workspace: WorkspaceSpec {
                repos: vec![],
                isolation: Isolation::Clone,
                carry_uncommitted: false,
            },
            system_prompt_append: String::new(),
            allowed_local_mcp_servers: vec![],
            channel_files,
            extensions: Default::default(),
            resume: None,
            turn: Some(turn),
            turn_approval_mode: None,
        }
    }

    /// Starts a session and returns its workspace.
    async fn open(&self, id: &str, start: SessionStart) -> PathBuf {
        let response = self.hub.command(id, start.into()).await;
        let result: SessionStartResult =
            serde_json::from_value(response.result.expect("the start works")).unwrap();
        PathBuf::from(result.workspace_path)
    }

    fn error_events(&self, session: SessionId) -> Vec<(String, String)> {
        self.hub
            .events(session)
            .into_iter()
            .filter_map(|e| match e.event {
                SessionEvent::Error { code, message } => Some((code, message)),
                _ => None,
            })
            .collect()
    }

    fn turns(&self) -> Vec<TurnInput> {
        self.record.lock().unwrap().turns.clone()
    }
}

fn blob(name: &str, bytes: &[u8]) -> BlobRef {
    BlobRef {
        sha256: sha(bytes),
        name: name.into(),
        mime: "text/plain".into(),
        size: bytes.len() as u64,
    }
}

fn channel_file(workspace: &Path, path: &str) -> PathBuf {
    workspace.join(".channel/files").join(path)
}

#[tokio::test]
async fn the_first_turn_gets_its_attachments_and_the_channel_files() {
    let rig = rig().await;
    rig.serve(b"plan");
    rig.serve(b"photo bytes");
    let photo = blob("../../photo.png", b"photo bytes");
    let session = SessionId::new();
    let workspace = rig
        .open(
            "start-1",
            rig.start(
                session,
                vec![FileRef {
                    path: "docs/plan.md".into(),
                    sha256: sha(b"plan"),
                }],
                TurnInput {
                    text: "Look at this".into(),
                    attachments: vec![photo.clone()],
                },
            ),
        )
        .await;

    // The attachment is in the workspace before the harness gets the turn,
    // under a safe name.
    let saved = attachment_path(&workspace, &photo);
    assert!(saved.starts_with(workspace.join(".consort/attachments")));
    assert!(
        saved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("-_.._photo.png")
    );
    assert_eq!(std::fs::read(&saved).unwrap(), b"photo bytes");
    let turns = rig.turns();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].attachments, vec![photo]);

    // The channel file is a read-only copy.
    let plan = channel_file(&workspace, "docs/plan.md");
    assert_eq!(std::fs::read(&plan).unwrap(), b"plan");
    assert!(std::fs::metadata(&plan).unwrap().permissions().readonly());
    // Both came with the signed GET, which the fake hub checks.
    assert_eq!(rig.gets(), 2);
    // Both are in the cache under the Consort home.
    let cache = rig.dir.path().join("home/cache/blobs");
    assert!(cache.join(sha(b"plan").as_str()).is_file());
    assert!(cache.join(sha(b"photo bytes").as_str()).is_file());
    assert!(rig.error_events(session).is_empty());
}

#[tokio::test]
async fn a_bad_attachment_fails_alone_and_the_user_hears_of_it() {
    let rig = rig().await;
    rig.serve(b"good");
    let good = blob("good.txt", b"good");
    // The hub sends bytes that do not match the hash.
    let tampered = blob("tampered.txt", b"what the client sent");
    rig.serve_as(&tampered.sha256, b"something else");
    // The hub does not have this one.
    let missing = blob("missing.txt", b"never uploaded");
    let session = SessionId::new();
    let workspace = rig
        .open(
            "start-2",
            rig.start(
                session,
                vec![],
                TurnInput {
                    text: "Three files".into(),
                    attachments: vec![good.clone(), tampered.clone(), missing.clone()],
                },
            ),
        )
        .await;
    // The turn runs with all three named. Only the good one is there.
    assert_eq!(rig.turns()[0].attachments.len(), 3);
    assert!(attachment_path(&workspace, &good).is_file());
    assert!(!attachment_path(&workspace, &tampered).exists());
    assert!(!attachment_path(&workspace, &missing).exists());
    let errors = rig
        .hub
        .wait_for("two attachment errors", |_| {
            let errors = rig.error_events(session);
            (errors.len() == 2).then_some(errors)
        })
        .await;
    assert!(errors.iter().all(|(code, _)| code == "attachment_failed"));
    assert!(
        errors[0]
            .1
            .contains("tampered.txt, because the downloaded bytes do not match"),
        "{errors:?}"
    );
    assert!(errors[1].1.contains("missing.txt"), "{errors:?}");
    // Nothing with a wrong hash stays in the cache.
    let cache = rig.dir.path().join("home/cache/blobs");
    assert!(!cache.join(tampered.sha256.as_str()).exists());
}

#[tokio::test]
async fn a_turn_refreshes_the_channel_files_and_uses_the_cache() {
    let rig = rig().await;
    rig.serve(b"v1");
    rig.serve(b"v2");
    rig.serve(b"old notes");
    rig.serve(b"shared");
    let session = SessionId::new();
    let workspace = rig
        .open(
            "start-3",
            rig.start(
                session,
                vec![
                    FileRef {
                        path: "plan.md".into(),
                        sha256: sha(b"v1"),
                    },
                    FileRef {
                        path: "old/notes.txt".into(),
                        sha256: sha(b"old notes"),
                    },
                ],
                TurnInput {
                    text: "First".into(),
                    attachments: vec![blob("shared.txt", b"shared")],
                },
            ),
        )
        .await;
    assert_eq!(rig.gets(), 3);

    // A turn with no new list leaves the copy as it is.
    let turn = |id: &str, files: Option<Vec<FileRef>>, attachments: Vec<BlobRef>| {
        let command = SessionTurn {
            session_id: session,
            turn: TurnInput {
                text: id.into(),
                attachments,
            },
            channel_files: files,
            approval_mode: None,
        };
        let hub = rig.hub.clone();
        let id = id.to_owned();
        async move { hub.command(&id, command.into()).await }
    };
    rig.hub
        .wait_for("the first turn to end", |h| {
            h.events(session)
                .iter()
                .any(|e| matches!(e.event, SessionEvent::TurnEnded { .. }))
                .then_some(())
        })
        .await;
    turn("turn-2", None, vec![]).await.result.unwrap();
    assert_eq!(
        std::fs::read(channel_file(&workspace, "plan.md")).unwrap(),
        b"v1"
    );
    assert_eq!(rig.gets(), 3);

    // A new list: a new version, and a file that is gone. The attachment of
    // this turn is in the cache already, so the hub sends it no more.
    rig.hub
        .wait_for("the second turn to end", |h| {
            (h.events(session)
                .iter()
                .filter(|e| matches!(e.event, SessionEvent::TurnEnded { .. }))
                .count()
                == 2)
                .then_some(())
        })
        .await;
    turn(
        "turn-3",
        Some(vec![FileRef {
            path: "plan.md".into(),
            sha256: sha(b"v2"),
        }]),
        vec![blob("shared.txt", b"shared")],
    )
    .await
    .result
    .unwrap();
    assert_eq!(
        std::fs::read(channel_file(&workspace, "plan.md")).unwrap(),
        b"v2"
    );
    assert!(!channel_file(&workspace, "old/notes.txt").exists());
    assert!(!channel_file(&workspace, "old").exists());
    assert_eq!(rig.gets(), 4, "only v2 came from the hub");

    // A steer with an attachment saves it before the harness gets it.
    rig.serve(b"steered");
    let steered = blob("more.txt", b"steered");
    let response = rig
        .hub
        .command(
            "steer-1",
            SessionSteer {
                session_id: session,
                turn: TurnInput {
                    text: "And this".into(),
                    attachments: vec![steered.clone()],
                },
            }
            .into(),
        )
        .await;
    // The fake turn may have ended already, so the steer may be refused. The
    // file is saved either way.
    let _ = response.result;
    assert!(attachment_path(&workspace, &steered).is_file());

    rig.hub
        .command(
            "stop-3",
            SessionStop {
                session_id: session,
                reason: "done".into(),
            }
            .into(),
        )
        .await;
}

#[tokio::test]
async fn a_git_workspace_does_not_see_the_worker_folders() {
    // A repo on this machine, so the clone needs no network.
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str], cwd: &Path| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    git(&["init", "-q", "-b", "main"], repo.path());
    std::fs::write(repo.path().join("README.md"), "hi\n").unwrap();
    git(&["add", "README.md"], repo.path());
    git(
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "first",
        ],
        repo.path(),
    );

    let rig = rig().await;
    rig.serve(b"doc");
    rig.serve(b"att");
    let mut start = rig.start(
        SessionId::new(),
        vec![FileRef {
            path: "doc.md".into(),
            sha256: sha(b"doc"),
        }],
        TurnInput {
            text: "Go".into(),
            attachments: vec![blob("a.txt", b"att")],
        },
    );
    start.workspace = WorkspaceSpec {
        repos: vec![consort_proto::worker_link::WorkspaceRepo {
            repo_id: consort_proto::ids::RepoId::new(),
            url: format!("file://{}", repo.path().display()),
            branch: None,
            checkout_path: None,
        }],
        isolation: Isolation::Clone,
        carry_uncommitted: false,
    };
    let workspace = rig.open("start-git", start).await;
    assert!(channel_file(&workspace, "doc.md").is_file());
    // The clone ran with the system settings of git. On Windows these have
    // `core.autocrlf=true`, and the files have CRLF line ends. This `git`
    // ignores the system settings, so say the same setting here.
    let autocrlf: &[&str] = if cfg!(windows) {
        &["-c", "core.autocrlf=true"]
    } else {
        &[]
    };
    let status = git(&[autocrlf, &["status", "--porcelain"]].concat(), &workspace);
    assert_eq!(status, "", "git sees the worker folders: {status}");
}
