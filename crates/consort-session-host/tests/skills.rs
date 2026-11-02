//! Skill delivery, against the fake hub
//! (`common/mod.rs`) and the fake harness.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use common::{CODE, FakeHub};

use consort_harness::{AccountConfig, Harness, HarnessKind};
use consort_proto::ids::{AccountId, ChannelId, SessionId, TaskId};
use consort_proto::model::{
    ApprovalMode, ConnectorContextEntry, Isolation, SessionRole, SkillBundleRef, SkillContextEntry,
};
use consort_proto::session_host_link::{
    SessionEvent, SessionExtensions, SessionStart, SessionStop, WorkspaceSpec,
};
use consort_session_host::config::HostToml;
use consort_session_host::{Harnesses, HostConfig, HostHandle};
use consort_testkit::{FakeHarness, FakeRecord, FakeScript};

fn test_config(dir: &std::path::Path) -> HostConfig {
    HostConfig {
        ws_path: consort_proto::auth::SESSION_HOST_WS_PATH,
        config_dir: dir.to_path_buf(),
        consort_home: Some(dir.join("home")),
        check_tool_updates: false,
    }
}

struct Rig {
    hub: FakeHub,
    dir: tempfile::TempDir,
    worker: HostHandle,
    account_id: AccountId,
    record: Arc<std::sync::Mutex<FakeRecord>>,
}

/// A paired worker with one account of `kind`, served by the fake harness.
async fn rig(kind: HarnessKind) -> Rig {
    rig_with(kind, |_| {}).await
}

/// [`rig`], with a change to the account before the worker starts.
async fn rig_with(kind: HarnessKind, change: impl FnOnce(&mut AccountConfig)) -> Rig {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    consort_session_host::pair(&hub.url, CODE, dir.path(), None, vec![])
        .await
        .unwrap();
    let mut toml = HostToml::load(dir.path()).unwrap();
    let mut account = AccountConfig::default_for(kind);
    change(&mut account);
    let account_id = account.id;
    toml.accounts = vec![account];
    toml.save(dir.path()).unwrap();
    let fake = FakeHarness::new(kind, FakeScript::reply("Hi."));
    let record = fake.record();
    let mut harnesses: Harnesses = HashMap::new();
    harnesses.insert(kind, Arc::new(fake) as Arc<dyn Harness>);
    let worker = consort_session_host::run_with(test_config(dir.path()), harnesses)
        .await
        .unwrap();
    hub.wait_connected().await;
    Rig {
        hub,
        dir,
        worker,
        account_id,
        record,
    }
}

fn start(
    session_id: SessionId,
    account_id: AccountId,
    extensions: SessionExtensions,
) -> SessionStart {
    SessionStart {
        execution: None,
        session_id,
        role: SessionRole::Coordinator,
        channel_id: ChannelId::new(),
        task_id: Some(TaskId::new()),
        account_id,
        model: "fake-model".into(),
        options: Default::default(),
        approval_mode: ApprovalMode::Auto,
        workspace: WorkspaceSpec {
            repos: vec![],
            isolation: Isolation::Clone,
            carry_uncommitted: false,
        },
        system_prompt_append: "You are the coordinator.".into(),
        allowed_local_mcp_servers: vec![],
        channel_files: vec![],
        extensions,
        resume: None,
        turn: None,
        turn_approval_mode: None,
    }
}

fn extensions(skills: Vec<SkillBundleRef>) -> SessionExtensions {
    SessionExtensions {
        connectors: vec![ConnectorContextEntry {
            handle: "gmail_work".into(),
            service_name: "Gmail".into(),
            identity: Some("me@example.com".into()),
            needs_sign_in: false,
            assistant_owned: false,
        }],
        skill_context: skills
            .iter()
            .map(|s| SkillContextEntry {
                name: s.name.clone(),
                description: format!("Does {}.", s.name),
            })
            .collect(),
        skills,
        monid: false,
        monid_requestable: false,
        other_connections: Vec::new(),
    }
}

const SKILL_FILES: &[(&str, &str, bool)] = &[
    (
        "SKILL.md",
        "---\nname: pdf-forms\ndescription: Does pdf-forms.\n---\nFill the form.\n",
        false,
    ),
    ("scripts/fill.sh", "#!/bin/sh\necho filled\n", true),
];

fn opened(record: &Arc<std::sync::Mutex<FakeRecord>>, n: usize) -> consort_harness::SessionSpec {
    record.lock().unwrap().opened[n].clone()
}

#[tokio::test]
async fn a_claude_session_gets_a_skills_plugin_and_the_context_text() {
    let rig = rig(HarnessKind::ClaudeCode).await;
    let skill = rig.hub.serve_skill("pdf-forms", SKILL_FILES);
    let session_id = SessionId::new();
    let response = rig
        .hub
        .command(
            "start-1",
            start(session_id, rig.account_id, extensions(vec![skill.clone()])).into(),
        )
        .await;
    assert!(response.result.is_ok(), "{:?}", response.result);

    let spec = opened(&rig.record, 0);
    let skills = spec.skills.expect("the session has skills");
    assert_eq!(skills.names, ["pdf-forms"]);
    // The harness reads them from a skills-only plugin of this session.
    let plugin = consort_harness::skills::claude_plugin_dir(&skills);
    assert!(plugin.join(".claude-plugin/plugin.json").is_file());
    assert_eq!(
        std::fs::read_to_string(plugin.join("skills/pdf-forms/SKILL.md")).unwrap(),
        SKILL_FILES[0].1
    );
    assert!(plugin.join("skills/pdf-forms/scripts/fill.sh").is_file());
    // The folder is in the session's runtime folder, not in an account folder.
    assert!(skills.root.starts_with(rig.dir.path().join("runtime")));
    assert!(skills.root.starts_with(&spec.runtime_dir));

    // The cache has the files by content hash, checked against the signed
    // blob requests.
    assert!(
        rig.dir
            .path()
            .join("home/cache/skills")
            .join(skill.content_hash.as_str())
            .join("bundle.json")
            .is_file()
    );
    assert_eq!(rig.hub.state.blob_gets.lock().unwrap().len(), 2);
    assert_eq!(
        *rig.hub.state.bundle_requests.lock().unwrap(),
        [skill.content_hash.to_string()]
    );

    // The prompt has the hub text, the connectors and the skills of the
    // channel, with the native wording.
    let prompt = spec.system_prompt_append;
    assert!(
        prompt.starts_with("You are the coordinator.\n\n"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Connectors in this channel:\n- "),
        "{prompt}"
    );
    assert!(prompt.contains("gmail_work"), "{prompt}");
    assert!(
        prompt.contains("Skills in this channel:\n- pdf-forms"),
        "{prompt}"
    );
    assert!(prompt.contains("They are installed as skills."), "{prompt}");
    assert!(!prompt.contains("read_skill"), "{prompt}");

    // A second session with the same version reads the cache.
    let second = SessionId::new();
    assert!(
        rig.hub
            .command(
                "start-2",
                start(second, rig.account_id, extensions(vec![skill.clone()])).into()
            )
            .await
            .result
            .is_ok()
    );
    let spec2 = opened(&rig.record, 1);
    let skills2 = spec2.skills.unwrap();
    assert_ne!(skills2.root, skills.root, "each session has its own folder");
    assert_eq!(rig.hub.state.bundle_requests.lock().unwrap().len(), 1);
    assert_eq!(rig.hub.state.blob_gets.lock().unwrap().len(), 2);

    // The folder goes away with its session. The other session keeps its own.
    let stop = rig
        .hub
        .command(
            "stop-1",
            SessionStop {
                session_id,
                reason: "done".into(),
            }
            .into(),
        )
        .await;
    assert!(stop.result.is_ok());
    assert!(!skills.root.exists(), "the folder of an ended session goes");
    assert!(skills2.root.exists(), "a running session keeps its skills");
    rig.worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_resumed_session_gets_the_same_skills_from_the_cache() {
    let rig = rig(HarnessKind::ClaudeCode).await;
    let skill = rig.hub.serve_skill("pdf-forms", SKILL_FILES);
    let session_id = SessionId::new();
    assert!(
        rig.hub
            .command(
                "start-1",
                start(session_id, rig.account_id, extensions(vec![skill.clone()])).into()
            )
            .await
            .result
            .is_ok()
    );
    let first = opened(&rig.record, 0).skills.unwrap();
    rig.hub
        .wait_for("the native ref", |h| {
            h.events(session_id)
                .iter()
                .any(|e| matches!(e.event, SessionEvent::NativeRef { .. }))
                .then_some(())
        })
        .await;
    let Rig {
        hub,
        dir,
        worker,
        record,
        ..
    } = rig;
    worker.shutdown().await.unwrap();
    assert!(
        !first.root.exists(),
        "the folder goes when the worker stops"
    );
    hub.wait_for("the old link to close", |h| {
        h.state.conn.lock().unwrap().is_none().then_some(())
    })
    .await;

    // The hub no longer serves the version. The worker has it in the cache.
    hub.state.bundles.lock().unwrap().clear();
    hub.state.blob_bodies.lock().unwrap().clear();
    let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
    let again = fake.record();
    let mut harnesses: Harnesses = HashMap::new();
    harnesses.insert(HarnessKind::ClaudeCode, Arc::new(fake) as Arc<dyn Harness>);
    let worker = consort_session_host::run_with(test_config(dir.path()), harnesses)
        .await
        .unwrap();
    hub.wait_connected().await;
    hub.wait_for("the resumed open", |_| {
        (again.lock().unwrap().opened.len() == 1).then_some(())
    })
    .await;
    let resumed = again.lock().unwrap().opened[0].clone();
    assert!(resumed.resume.is_some());
    let skills = resumed.skills.expect("the resumed session has its skills");
    assert_eq!(skills.names, ["pdf-forms"]);
    assert!(
        consort_harness::skills::claude_plugin_dir(&skills)
            .join("skills/pdf-forms/SKILL.md")
            .is_file()
    );
    assert!(
        resumed.system_prompt_append.contains("installed as skills"),
        "{}",
        resumed.system_prompt_append
    );
    assert_eq!(record.lock().unwrap().opened.len(), 1);
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_harness_without_native_skills_gets_the_hub_tool_hint_and_no_files() {
    let rig = rig(HarnessKind::Acp).await;
    let skill = rig.hub.serve_skill("pdf-forms", SKILL_FILES);
    let session_id = SessionId::new();
    assert!(
        rig.hub
            .command(
                "start-1",
                start(session_id, rig.account_id, extensions(vec![skill])).into()
            )
            .await
            .result
            .is_ok()
    );
    let spec = opened(&rig.record, 0);
    assert!(spec.skills.is_none());
    assert!(
        spec.system_prompt_append
            .contains("call read_skill with its name"),
        "{}",
        spec.system_prompt_append
    );
    assert!(
        spec.system_prompt_append
            .contains("Connectors in this channel")
    );
    // The worker does not fetch what the harness cannot load.
    assert!(rig.hub.state.bundle_requests.lock().unwrap().is_empty());
    assert!(rig.hub.state.blob_gets.lock().unwrap().is_empty());
    rig.worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_skill_that_fails_does_not_fail_the_session() {
    let rig = rig(HarnessKind::ClaudeCode).await;
    let good = rig.hub.serve_skill("pdf-forms", SKILL_FILES);
    let missing = rig.hub.serve_skill("notes", &[("SKILL.md", "x", false)]);
    rig.hub
        .state
        .bundles
        .lock()
        .unwrap()
        .remove(missing.content_hash.as_str());
    let session_id = SessionId::new();
    let response = rig
        .hub
        .command(
            "start-1",
            start(session_id, rig.account_id, extensions(vec![good, missing])).into(),
        )
        .await;
    assert!(response.result.is_ok(), "{:?}", response.result);
    let spec = opened(&rig.record, 0);
    assert_eq!(spec.skills.unwrap().names, ["pdf-forms"]);
    // The prompt does not describe the skill that the session lacks.
    assert!(
        spec.system_prompt_append.contains("- pdf-forms"),
        "{}",
        spec.system_prompt_append
    );
    assert!(
        !spec.system_prompt_append.contains("- notes"),
        "{}",
        spec.system_prompt_append
    );
    // The user sees why.
    let note = rig
        .hub
        .wait_for("the skills note", |h| {
            h.events(session_id)
                .into_iter()
                .find_map(|e| match e.event {
                    SessionEvent::Error { code, message } if code == "skills_not_delivered" => {
                        Some(message)
                    }
                    _ => None,
                })
        })
        .await;
    assert!(note.contains("notes"), "{note}");
    rig.worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn codex_and_opencode_sessions_get_a_plain_skills_folder() {
    for kind in [HarnessKind::Codex, HarnessKind::OpenCode] {
        let rig = rig(kind).await;
        let skill = rig.hub.serve_skill("pdf-forms", SKILL_FILES);
        let session_id = SessionId::new();
        assert!(
            rig.hub
                .command(
                    "start-1",
                    start(session_id, rig.account_id, extensions(vec![skill])).into()
                )
                .await
                .result
                .is_ok()
        );
        let spec = opened(&rig.record, 0);
        let skills = spec.skills.expect("the session has skills");
        assert_eq!(
            std::fs::read_to_string(skills.root.join("skills/pdf-forms/SKILL.md")).unwrap(),
            SKILL_FILES[0].1,
            "{kind}"
        );
        assert!(skills.root.starts_with(&spec.runtime_dir), "{kind}");
        assert!(
            spec.system_prompt_append.contains("installed as skills"),
            "{kind}"
        );
        rig.worker.shutdown().await.unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_gemini_session_gets_its_own_home_that_goes_with_the_session() {
    let account_home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(account_home.path().join(".gemini")).unwrap();
    std::fs::write(account_home.path().join(".gemini/settings.json"), "{}").unwrap();
    let path = account_home.path().to_string_lossy().into_owned();
    let rig = rig_with(HarnessKind::GeminiCli, |account| {
        account.env.insert("GEMINI_CLI_HOME".into(), path);
    })
    .await;
    let skill = rig.hub.serve_skill("pdf-forms", SKILL_FILES);
    let session_id = SessionId::new();
    assert!(
        rig.hub
            .command(
                "start-1",
                start(session_id, rig.account_id, extensions(vec![skill])).into()
            )
            .await
            .result
            .is_ok()
    );
    let skills = opened(&rig.record, 0)
        .skills
        .expect("the session has skills");
    let home = std::path::PathBuf::from(&skills.env["GEMINI_CLI_HOME"]);
    assert!(home.starts_with(&skills.root));
    assert!(home.join(".gemini/skills/pdf-forms/SKILL.md").is_file());
    assert!(
        home.join(".gemini/settings.json")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // Gemini makes a file for the first time in the session.
    std::fs::write(home.join(".gemini/trustedFolders.json"), "{}").unwrap();
    assert!(
        rig.hub
            .command(
                "stop-1",
                SessionStop {
                    session_id,
                    reason: "done".into()
                }
                .into()
            )
            .await
            .result
            .is_ok()
    );
    assert!(!skills.root.exists());
    // The new file moved to the account. The account has no skill of ours.
    assert!(
        account_home
            .path()
            .join(".gemini/trustedFolders.json")
            .is_file()
    );
    assert!(!account_home.path().join(".gemini/skills").exists());
    rig.worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_coordinator_session_has_no_shell_and_a_task_session_keeps_it() {
    let rig = rig(HarnessKind::ClaudeCode).await;
    // The worker policy allows the shell. The coordinator gets none anyway,
    // so a command that the harness would run without asking cannot run.
    let coordinator = SessionId::new();
    let response = rig
        .hub
        .command(
            "start-c",
            start(coordinator, rig.account_id, extensions(vec![])).into(),
        )
        .await;
    assert!(response.result.is_ok(), "{:?}", response.result);
    let spec = opened(&rig.record, 0);
    assert!(!spec.allow_shell);
    assert!(spec.coordinator);

    let task = SessionId::new();
    let mut task_start = start(task, rig.account_id, extensions(vec![]));
    task_start.role = SessionRole::Task;
    let response = rig.hub.command("start-t", task_start.into()).await;
    assert!(response.result.is_ok(), "{:?}", response.result);
    let spec = opened(&rig.record, 1);
    assert!(spec.allow_shell);
    assert!(!spec.coordinator);
}
