//! Tests of the account, sign-in and tool commands with fake harnesses and
//! a fake `claude` program.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use consort_harness::{Harness, HarnessKind};
use consort_proto::error::ErrorCode;
use consort_proto::frame::Frame;
use consort_proto::ids::{AccountId, SignInId};
use consort_proto::model::{SecretEnv, SignInMethod, SignInState, ToolId as ProtoToolId};
use consort_proto::worker_link::{
    AccountCreateCommand, AccountRemoveCommand, AccountSignOutCommand, AccountUpdateCommand,
    HubToWorker, SignInCancelCommand, SignInCodeCommand, SignInStartCommand, SignInUpdate,
    ToolsInstall,
};
use consort_testkit::{FakeHarness, FakeScript};
use tokio::sync::mpsc;

use crate::config::WorkerToml;
use crate::core::Core;
use crate::core::tests::test_core;
use crate::profile::{ENV_FILE, read_secrets};

fn fakes(managed: bool) -> crate::Harnesses {
    let mut harnesses: crate::Harnesses = HashMap::new();
    for kind in [
        HarnessKind::ClaudeCode,
        HarnessKind::Codex,
        HarnessKind::GeminiCli,
        HarnessKind::OpenCode,
    ] {
        let mut fake = FakeHarness::new(kind, FakeScript::reply("Hi."));
        fake.uses_managed_tools = managed && kind == HarnessKind::ClaudeCode;
        harnesses.insert(kind, Arc::new(fake) as Arc<dyn Harness>);
    }
    harnesses
}

/// A worker with fake harnesses, and the frames that it sends.
fn worker(
    managed: bool,
) -> (
    tempfile::TempDir,
    Arc<Core>,
    mpsc::UnboundedReceiver<String>,
) {
    let dir = tempfile::tempdir().unwrap();
    let core = test_core(dir.path(), fakes(managed), WorkerToml::default());
    let (tx, rx) = mpsc::unbounded_channel();
    core.link.attach(tx);
    (dir, core, rx)
}

fn create(account_id: AccountId, harness: &str, env: &[(&str, &str)]) -> HubToWorker {
    AccountCreateCommand {
        account_id,
        harness: harness.to_owned(),
        label: "Claude Personal".to_owned(),
        acp: None,
        env: env.iter().copied().collect::<SecretEnv>(),
    }
    .into()
}

/// Collects the sign-in updates that the worker sent.
#[derive(Default)]
struct Updates(Vec<SignInState>);

impl Updates {
    fn take(&mut self, rx: &mut mpsc::UnboundedReceiver<String>) {
        while let Ok(text) = rx.try_recv() {
            if let Ok(Frame::Event(event)) = Frame::parse(&text)
                && event.kind == "sign_in.update"
            {
                let update: SignInUpdate = event.decode_as().unwrap();
                self.0.push(update.state);
            }
        }
    }

    async fn wait_for(
        &mut self,
        rx: &mut mpsc::UnboundedReceiver<String>,
        what: &str,
        check: impl Fn(&SignInState) -> bool,
    ) -> SignInState {
        for _ in 0..500 {
            self.take(rx);
            if let Some(state) = self.0.iter().find(|s| check(s)) {
                return state.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {what}; saw {:?}", self.0);
    }
}

#[tokio::test]
async fn create_update_and_remove_an_account_with_a_profile_folder() {
    let (_dir, core, _rx) = worker(false);
    let id = AccountId::new();
    core.account_command(create(
        id,
        "claude",
        &[("ANTHROPIC_API_KEY", "sk-ant-secret")],
    ))
    .await
    .unwrap();
    // A repeat of the same command is no error and adds nothing.
    core.account_command(create(
        id,
        "claude",
        &[("ANTHROPIC_API_KEY", "sk-ant-secret")],
    ))
    .await
    .unwrap();
    let config = WorkerToml::load(&core.config_dir).unwrap();
    assert_eq!(config.accounts.len(), 1);
    assert!(config.accounts[0].profile);
    let text = std::fs::read_to_string(WorkerToml::path(&core.config_dir)).unwrap();
    assert!(!text.contains("sk-ant-secret"), "no secret in worker.toml");

    let profile = core.profile_dir(&core.config_snapshot().account(id).unwrap().clone());
    // The folder is named after the label.
    assert!(
        profile.ends_with("profiles/claude-personal"),
        "{}",
        profile.display()
    );
    assert_eq!(
        config.accounts[0].profile_dir.as_deref(),
        Some("claude-personal")
    );
    // A second account with the same label gets its own folder.
    let other = AccountId::new();
    core.account_command(create(other, "claude", &[]))
        .await
        .unwrap();
    assert_eq!(
        core.config_snapshot()
            .account(other)
            .unwrap()
            .profile_dir
            .as_deref(),
        Some("claude-personal-2")
    );
    core.account_command(AccountRemoveCommand { account_id: other }.into())
        .await
        .unwrap();
    assert!(profile.join("claude").is_dir());
    assert_eq!(
        read_secrets(&profile).unwrap()["ANTHROPIC_API_KEY"],
        "sk-ant-secret"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(profile.join(ENV_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let inventory = core.inventory();
    let account = &inventory.accounts[0];
    assert!(account.profile);
    assert_eq!(account.secret_names, ["ANTHROPIC_API_KEY"]);
    assert!(account.sign_in_methods.contains(&SignInMethod::ClaudePlan));
    let json = serde_json::to_string(&inventory).unwrap();
    assert!(
        !json.contains("sk-ant-secret"),
        "the inventory has names only"
    );

    // The session env has the profile variables, then the secrets, and no
    // login from the worker's own environment.
    let prepared = core.with_profile(&config.accounts[0]).unwrap();
    assert_eq!(
        prepared.runtime.env["CLAUDE_CONFIG_DIR"],
        profile.join("claude").to_string_lossy()
    );
    assert_eq!(prepared.runtime.env["ANTHROPIC_API_KEY"], "sk-ant-secret");
    assert!(
        prepared
            .runtime
            .env_remove
            .contains(&"OPENAI_API_KEY".to_owned())
    );

    let mut env_set = SecretEnv::new();
    env_set.insert("OTHER_KEY", "x");
    core.account_command(
        AccountUpdateCommand {
            account_id: id,
            label: Some("Claude Work".into()),
            env_set,
            env_remove: vec!["ANTHROPIC_API_KEY".into()],
            extras_enabled: None,
        }
        .into(),
    )
    .await
    .unwrap();
    assert_eq!(core.config_snapshot().accounts[0].label, "Claude Work");
    assert_eq!(core.inventory().accounts[0].secret_names, ["OTHER_KEY"]);

    // The provider extras toggle is not built yet. The worker refuses the
    // whole update, so the name does not change either.
    let err = core
        .account_command(
            AccountUpdateCommand {
                account_id: id,
                label: Some("Claude Other".into()),
                env_set: SecretEnv::new(),
                env_remove: vec![],
                extras_enabled: Some(false),
            }
            .into(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
    assert_eq!(core.config_snapshot().accounts[0].label, "Claude Work");
    assert!(core.inventory().accounts[0].extras_enabled);

    core.account_command(AccountRemoveCommand { account_id: id }.into())
        .await
        .unwrap();
    assert!(!profile.exists(), "the profile folder is gone");
    assert!(
        WorkerToml::load(&core.config_dir)
            .unwrap()
            .accounts
            .is_empty()
    );
    // A repeat of the remove is no error.
    core.account_command(AccountRemoveCommand { account_id: id }.into())
        .await
        .unwrap();
}

#[tokio::test]
async fn bad_account_commands_are_refused() {
    let (_dir, core, _rx) = worker(false);
    let err = core
        .account_command(create(AccountId::new(), "pi", &[]))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
    let err = core
        .account_command(create(AccountId::new(), "claude", &[("BAD-NAME", "x")]))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
    let err = core
        .account_command(
            ToolsInstall {
                tool: ProtoToolId::new("pip"),
            }
            .into(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
    let err = core
        .account_command(
            AccountSignOutCommand {
                account_id: AccountId::new(),
            }
            .into(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // A Claude method does not fit a Codex account.
    let id = AccountId::new();
    core.account_command(create(id, "codex", &[]))
        .await
        .unwrap();
    let err = core
        .account_command(
            SignInStartCommand {
                sign_in_id: SignInId::new(),
                account_id: id,
                method: SignInMethod::ClaudePlan,
            }
            .into(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Invalid);
    // A code for a sign-in that does not run.
    let err = core
        .account_command(
            SignInCodeCommand {
                sign_in_id: SignInId::new(),
                code: "x".into(),
            }
            .into(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn an_api_key_sign_in_probes_the_saved_key() {
    let (_dir, core, mut rx) = worker(false);
    let id = AccountId::new();
    core.account_command(create(id, "gemini", &[("GEMINI_API_KEY", "g-secret")]))
        .await
        .unwrap();
    let start = |variable: &str| -> HubToWorker {
        SignInStartCommand {
            sign_in_id: SignInId::new(),
            account_id: id,
            method: SignInMethod::ApiKey {
                variable: variable.into(),
            },
        }
        .into()
    };
    core.account_command(start("GEMINI_API_KEY")).await.unwrap();
    let mut updates = Updates::default();
    let state = updates
        .wait_for(&mut rx, "a final state", SignInState::is_final)
        .await;
    assert!(matches!(state, SignInState::Succeeded { .. }), "{state:?}");

    // The app can start the sign-in before it saves the key. The sign-in
    // waits for the key.
    core.account_command(start("OTHER_KEY")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut updates = Updates::default();
    updates.take(&mut rx);
    assert!(updates.0.is_empty(), "it waits: {:?}", updates.0);
    let mut env_set = SecretEnv::new();
    env_set.insert("OTHER_KEY", "o-secret");
    core.account_command(
        AccountUpdateCommand {
            account_id: id,
            label: None,
            env_set,
            env_remove: vec![],
            extras_enabled: None,
        }
        .into(),
    )
    .await
    .unwrap();
    let state = updates
        .wait_for(&mut rx, "a final state", SignInState::is_final)
        .await;
    assert!(matches!(state, SignInState::Succeeded { .. }), "{state:?}");

    // A sign-in that waits for a key can be cancelled.
    let waiting = SignInId::new();
    core.account_command(
        SignInStartCommand {
            sign_in_id: waiting,
            account_id: id,
            method: SignInMethod::ApiKey {
                variable: "THIRD_KEY".into(),
            },
        }
        .into(),
    )
    .await
    .unwrap();
    core.account_command(
        SignInCancelCommand {
            sign_in_id: waiting,
        }
        .into(),
    )
    .await
    .unwrap();
    let mut updates = Updates::default();
    let state = updates
        .wait_for(&mut rx, "a final state", SignInState::is_final)
        .await;
    assert_eq!(state, SignInState::Cancelled);
}

/// Writes `version` of a tool as the store lays it out, and returns its
/// folder.
#[cfg(unix)]
fn fake_tool(home: &Path, tool: &str, version: &str) -> std::path::PathBuf {
    let dir = home.join("tools").join(tool);
    std::fs::create_dir_all(dir.join(version)).unwrap();
    std::fs::write(dir.join("current"), version).unwrap();
    dir.join(version)
}

/// Makes the managed Claude Code look installed, with the fake `claude`
/// as its program.
#[cfg(unix)]
fn install_fake_claude(core: &Core) {
    let claude_dir = fake_tool(&core.consort_home, "claude-code", "9.9.9");
    consort_testkit::write_fake_claude(&claude_dir);
    fake_tool(&core.consort_home, "node", "24.0.0");
    fake_tool(&core.consort_home, "claude-agent-acp", "0.99.0");
}

/// Captures the log of the current thread.
#[cfg(unix)]
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

#[cfg(unix)]
impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_claude_sign_in_runs_claude_auth_login_with_the_pasted_code() {
    let log = LogBuffer::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    // A global subscriber: a scoped one misses log lines when other tests
    // on other threads hit the same log lines first. Only this test sets
    // it, so the other tests of this binary log into the buffer too.
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let (_dir, core, mut rx) = worker(true);
    install_fake_claude(&core);
    let id = AccountId::new();
    core.account_command(create(id, "claude", &[]))
        .await
        .unwrap();
    let sign_in_id = SignInId::new();
    core.account_command(
        SignInStartCommand {
            sign_in_id,
            account_id: id,
            method: SignInMethod::ClaudePlan,
        }
        .into(),
    )
    .await
    .unwrap();
    let mut updates = Updates::default();
    let state = updates
        .wait_for(&mut rx, "needs_code", |s| {
            matches!(s, SignInState::NeedsCode { .. }) || s.is_final()
        })
        .await;
    assert_eq!(
        state,
        SignInState::NeedsCode {
            url: consort_testkit::FAKE_CLAUDE_URL.to_owned()
        }
    );
    // The login ran with the profile home, without a browser and without
    // the self-update.
    let claude_home = core
        .profile_dir(&core.config_snapshot().account(id).unwrap().clone())
        .join("claude");
    let seen = std::fs::read_to_string(claude_home.join("fake-login")).unwrap();
    let browser = consort_harness::signin::claude::no_browser_program();
    assert_eq!(
        seen.trim(),
        format!("--claudeai|BROWSER={browser}|DISABLE_AUTOUPDATER=1")
    );

    core.account_command(
        SignInCodeCommand {
            sign_in_id,
            code: consort_testkit::FAKE_CLAUDE_GOOD_CODE.to_owned(),
        }
        .into(),
    )
    .await
    .unwrap();
    let state = updates
        .wait_for(&mut rx, "a final state", SignInState::is_final)
        .await;
    assert_eq!(
        state,
        SignInState::Succeeded {
            detail: Some("fake@example.com, Max plan".to_owned())
        }
    );
    assert!(updates.0.contains(&SignInState::Verifying));

    // Sign out runs `claude auth logout` with the same home.
    core.account_command(AccountSignOutCommand { account_id: id }.into())
        .await
        .unwrap();
    assert!(!claude_home.join("fake-signed-in").exists());

    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("a sign-in starts"), "the log works: {text}");
    assert!(
        !text.contains(consort_testkit::FAKE_CLAUDE_GOOD_CODE),
        "the code is never logged: {text}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_wrong_code_fails_without_showing_the_code() {
    let (_dir, core, mut rx) = worker(true);
    install_fake_claude(&core);
    let id = AccountId::new();
    core.account_command(create(id, "claude", &[]))
        .await
        .unwrap();
    let sign_in_id = SignInId::new();
    core.account_command(
        SignInStartCommand {
            sign_in_id,
            account_id: id,
            method: SignInMethod::ClaudeConsole,
        }
        .into(),
    )
    .await
    .unwrap();
    let mut updates = Updates::default();
    updates
        .wait_for(&mut rx, "needs_code", |s| {
            matches!(s, SignInState::NeedsCode { .. })
        })
        .await;
    let code = "wrong-code-4711";
    core.account_command(
        SignInCodeCommand {
            sign_in_id,
            code: code.to_owned(),
        }
        .into(),
    )
    .await
    .unwrap();
    let state = updates
        .wait_for(&mut rx, "a final state", SignInState::is_final)
        .await;
    let SignInState::Failed { message } = state else {
        panic!("{state:?}")
    };
    assert!(message.contains("not accepted"), "{message}");
    assert!(!message.contains(code), "{message}");
    let seen = std::fs::read_to_string(
        core.profile_dir(&core.config_snapshot().account(id).unwrap().clone())
            .join("claude")
            .join("fake-login"),
    )
    .unwrap();
    assert!(seen.starts_with("--console|"), "{seen}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_cancelled_sign_in_stops_the_login() {
    let (_dir, core, mut rx) = worker(true);
    install_fake_claude(&core);
    let id = AccountId::new();
    core.account_command(create(id, "claude", &[]))
        .await
        .unwrap();
    let sign_in_id = SignInId::new();
    core.account_command(
        SignInStartCommand {
            sign_in_id,
            account_id: id,
            method: SignInMethod::ClaudePlan,
        }
        .into(),
    )
    .await
    .unwrap();
    let mut updates = Updates::default();
    updates
        .wait_for(&mut rx, "needs_code", |s| {
            matches!(s, SignInState::NeedsCode { .. })
        })
        .await;
    core.account_command(SignInCancelCommand { sign_in_id }.into())
        .await
        .unwrap();
    let state = updates
        .wait_for(&mut rx, "a final state", SignInState::is_final)
        .await;
    assert_eq!(state, SignInState::Cancelled);
    assert!(core.sign_ins.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn a_session_of_a_profile_account_gets_the_managed_launch() {
    use consort_proto::ids::SessionId;
    let dir = tempfile::tempdir().unwrap();
    let mut fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
    fake.uses_managed_tools = true;
    let record = fake.record();
    let mut harnesses: crate::Harnesses = HashMap::new();
    harnesses.insert(HarnessKind::ClaudeCode, Arc::new(fake) as Arc<dyn Harness>);
    let core = test_core(dir.path(), harnesses, WorkerToml::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    core.link.attach(tx);
    install_fake_claude(&core);
    let id = AccountId::new();
    core.account_command(create(id, "claude", &[("ANTHROPIC_API_KEY", "k")]))
        .await
        .unwrap();
    let mut start = crate::store::tests::start(SessionId::new());
    start.account_id = id;
    let frame_id = consort_proto::frame::FrameId::new("start-1");
    let command: HubToWorker = start.into();
    let Frame::Request(request) = Frame::family_request(frame_id.clone(), &command).unwrap() else {
        panic!("a request")
    };
    core.dispatch(request);
    for _ in 0..250 {
        if core
            .store
            .command_result(frame_id.as_str())
            .unwrap()
            .is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let record = record.lock().unwrap();
    let account = record.opened_accounts.first().expect("the session opened");
    let launch = account.runtime.launch.as_ref().expect("a managed launch");
    assert!(launch.env["CLAUDE_CODE_EXECUTABLE"].ends_with("claude"));
    assert_eq!(account.runtime.env["ANTHROPIC_API_KEY"], "k");
    // While the session runs, the tool is in use, so no update replaces it.
    assert!(core.tools.in_use(&consort_tools::ToolId::ClaudeCode));
}

/// Installs the real Codex as a managed tool in a new Consort home, then
/// probes a new profile account with it. Codex reports that nobody is
/// signed in. Run with `--ignored --nocapture real_codex`.
#[tokio::test]
#[ignore = "downloads Codex from GitHub"]
async fn real_codex_installs_and_probes_a_new_profile() {
    let dir = tempfile::tempdir().unwrap();
    let core = test_core(
        dir.path(),
        crate::default_harnesses(),
        WorkerToml::default(),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    core.link.attach(tx);
    let id = AccountId::new();
    core.account_command(create(id, "codex", &[]))
        .await
        .unwrap();
    // The new account installs Codex and probes in the background.
    let mut background = None;
    for _ in 0..600 {
        let inventory = core.inventory();
        let account = &inventory.accounts[0];
        let detail = account.auth_detail.clone().unwrap_or_default();
        if account.auth_state != consort_proto::model::AuthState::Unknown
            && !detail.starts_with("Installing")
        {
            background = Some((account.auth_state, detail));
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!("background probe: {background:?}");
    let account = core.config_snapshot().account(id).unwrap().clone();
    let adapter = core.harnesses[&HarnessKind::Codex].clone();
    let (prepared, lease) = core
        .prepare_for_run(&account, adapter.as_ref())
        .await
        .unwrap();
    let launch = prepared.runtime.launch.clone().unwrap();
    println!("launch: {} {:?}", launch.program.display(), launch.args);
    assert!(lease.is_some());
    let status = adapter.probe(&prepared).await;
    println!(
        "status: {:?} {:?} {:?}",
        status.auth_state, status.harness_version, status.auth_detail
    );
    assert_eq!(
        status.auth_state,
        consort_proto::model::AuthState::SignedOut
    );
    assert!(status.harness_version.is_some());
    let config =
        std::fs::read_to_string(core.profile_dir(&account).join("codex/config.toml")).unwrap();
    assert!(config.contains("cli_auth_credentials_store"));
    let tools = core.inventory().tools;
    let codex = tools.iter().find(|t| t.tool.as_str() == "codex").unwrap();
    println!("tool: {codex:?}");

    // Two probes at the same time on a new, empty profile both work.
    let second = AccountId::new();
    core.account_command(create(second, "codex", &[]))
        .await
        .unwrap();
    let account = core.config_snapshot().account(second).unwrap().clone();
    let (prepared, _lease) = core
        .prepare_for_run(&account, adapter.as_ref())
        .await
        .unwrap();
    let (a, b) = tokio::join!(adapter.probe(&prepared), adapter.probe(&prepared));
    println!("parallel: {:?} {:?}", a.auth_detail, b.auth_detail);
    assert_eq!(a.auth_state, consort_proto::model::AuthState::SignedOut);
    assert_eq!(b.auth_state, consort_proto::model::AuthState::SignedOut);
}

/// Installs the real Claude Code, its ACP adapter and Node as managed tools
/// in a new Consort home, probes a new profile account (signed out, from
/// `claude auth status`), starts a Claude plan sign-in up to `needs_code`,
/// and cancels it. No sign-in happens. Run with
/// `--ignored --nocapture real_claude`.
#[tokio::test]
#[ignore = "downloads Node, Claude Code and claude-agent-acp"]
async fn real_claude_installs_probes_and_starts_a_sign_in() {
    let dir = tempfile::tempdir().unwrap();
    let core = test_core(
        dir.path(),
        crate::default_harnesses(),
        WorkerToml::default(),
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    core.link.attach(tx);
    let id = AccountId::new();
    core.account_command(create(id, "claude", &[]))
        .await
        .unwrap();
    let account = core.config_snapshot().account(id).unwrap().clone();
    let adapter = core.harnesses[&HarnessKind::ClaudeCode].clone();
    let (prepared, _lease) = core
        .prepare_for_run(&account, adapter.as_ref())
        .await
        .unwrap();
    let launch = prepared.runtime.launch.clone().unwrap();
    println!("launch: {} {:?}", launch.program.display(), launch.args);
    let status = adapter.probe(&prepared).await;
    println!("status: {:?} {:?}", status.auth_state, status.auth_detail);
    assert_eq!(
        status.auth_state,
        consort_proto::model::AuthState::SignedOut
    );

    let sign_in_id = SignInId::new();
    core.account_command(
        SignInStartCommand {
            sign_in_id,
            account_id: id,
            method: SignInMethod::ClaudePlan,
        }
        .into(),
    )
    .await
    .unwrap();
    let mut updates = Updates::default();
    let state = updates
        .wait_for(&mut rx, "needs_code", |s| {
            matches!(s, SignInState::NeedsCode { .. }) || s.is_final()
        })
        .await;
    let SignInState::NeedsCode { url } = state else {
        panic!("{state:?}")
    };
    println!("needs_code: {}...", &url[..url.len().min(50)]);
    core.account_command(SignInCancelCommand { sign_in_id }.into())
        .await
        .unwrap();
    let state = updates
        .wait_for(&mut rx, "a final state", SignInState::is_final)
        .await;
    assert_eq!(state, SignInState::Cancelled);
}
