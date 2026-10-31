//! Accounts and managed tools: the `account.*` and `tools.*` hub commands,
//! the profile environment of an account, and the tools loop.
//!
//! Sign-in flows are in `signin.rs`. The design is in
//! `docs/harness-adapters.md` ("Harness accounts" and "Install harnesses").

use std::sync::Arc;
use std::time::Duration;

use consort_harness::account::LOGIN_VARIABLES;
use consort_harness::{AccountConfig, AccountStatus, Harness, HarnessKind, Launch};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::ids::AccountId;
use consort_proto::model::{AcpAccountSpec, SignInMethod, normalize_tags};
use consort_proto::session_host_link::{
    AccountCreateCommand, AccountRemoveCommand, AccountSignOutCommand, AccountUpdateCommand,
    HubToSessionHost,
};
use consort_tools::{LaunchSpec, ToolId};
use tokio::sync::watch;

use crate::HostError;
use crate::config::HostToml;
use crate::core::Core;
use crate::profile::{
    free_profile_name, prepare_profile, profile_dir, profile_env, read_secrets, remove_profile,
    valid_variable_name, write_secrets,
};
use crate::sessions::Job;
use crate::tools::{ToolLease, tool_name};

/// The longest account name.
const LABEL_MAX_CHARS: usize = 80;

/// The model providers that OpenCode accounts offer an API key for.
pub const OPENCODE_KEY_VARIABLES: [&str; 4] = [
    "FIREWORKS_API_KEY",
    "OPENROUTER_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
];

/// A managed tool launch as the harness crate takes it.
pub(crate) fn managed_launch(spec: LaunchSpec) -> Launch {
    Launch {
        program: spec.program,
        args: spec.args,
        env: spec.env,
    }
}

/// The `harness` of an account in the inventory: `acp:<registry id>` for an
/// agent from the ACP registry, `acp:custom` for a custom ACP command, else
/// the harness name. A hand-written `acp` account stays `acp`.
pub(crate) fn inventory_harness(account: &AccountConfig) -> String {
    match (&account.harness, &account.acp) {
        (HarnessKind::Acp, Some(AcpAccountSpec::Registry { id })) => format!("acp:{id}"),
        // A custom command has no registry entry and no logo of its own.
        (HarnessKind::Acp, Some(AcpAccountSpec::Command { .. })) => "acp:custom".to_owned(),
        (kind, _) => kind.as_str().to_owned(),
    }
}

/// The sign-in methods that the worker knows for a harness.
pub(crate) fn harness_methods(kind: HarnessKind) -> Vec<SignInMethod> {
    let key = |variable: &str| SignInMethod::ApiKey {
        variable: variable.to_owned(),
    };
    match kind {
        HarnessKind::ClaudeCode => vec![
            SignInMethod::ClaudePlan,
            SignInMethod::ClaudeConsole,
            key("ANTHROPIC_API_KEY"),
        ],
        HarnessKind::Codex => vec![
            SignInMethod::ChatgptDevice,
            SignInMethod::ChatgptBrowser,
            key("OPENAI_API_KEY"),
        ],
        HarnessKind::GeminiCli => vec![key("GEMINI_API_KEY")],
        HarnessKind::OpenCode => OPENCODE_KEY_VARIABLES.iter().map(|v| key(v)).collect(),
        HarnessKind::Pi | HarnessKind::Acp => Vec::new(),
    }
}

/// The sign-in methods that a client may offer for an account. An account
/// from `session-host.toml` signs in with the harness's own command, so it has
/// none. An ACP agent has the methods of its last probe.
pub(crate) fn sign_in_methods(
    account: &AccountConfig,
    status: Option<&AccountStatus>,
) -> Vec<SignInMethod> {
    if !account.profile {
        return Vec::new();
    }
    match account.harness {
        HarnessKind::Acp => status
            .map(|s| s.sign_in_methods.clone())
            .unwrap_or_default(),
        kind => harness_methods(kind),
    }
}

/// The variables of the API key methods of an account.
fn key_variables(account: &AccountConfig, status: Option<&AccountStatus>) -> Vec<String> {
    let mut methods = harness_methods(account.harness);
    if let Some(status) = status {
        methods.extend(status.sign_in_methods.iter().cloned());
    }
    methods
        .into_iter()
        .filter_map(|m| match m {
            SignInMethod::ApiKey { variable } => Some(variable),
            _ => None,
        })
        .collect()
}

/// The variables that the worker sets from the profile folder. A secret
/// variable cannot replace them.
const PROFILE_VARIABLES: [&str; 7] = [
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "GEMINI_CLI_HOME",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
];

fn invalid(message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::Invalid, message)
}

fn worker_error(err: HostError) -> ErrorBody {
    err.to_error_body()
}

/// Checks the names and values of secret variables.
fn check_secrets<'a>(secrets: impl Iterator<Item = (&'a str, &'a str)>) -> Result<(), ErrorBody> {
    for (name, value) in secrets {
        if !valid_variable_name(name) {
            return Err(invalid(format!(
                "\"{name}\" is not a valid variable name. Use letters, digits and underscores."
            )));
        }
        if PROFILE_VARIABLES.contains(&name) {
            return Err(invalid(format!(
                "{name} is set by the worker for the profile folder. Use another variable."
            )));
        }
        if value.trim().is_empty() {
            return Err(invalid(format!(
                "The value of {name} is empty. Paste the key, then save again."
            )));
        }
    }
    Ok(())
}

/// The harness of `account.create`: `claude`, `codex`, `opencode`,
/// `gemini`, `acp`, or `acp:<registry id>`.
fn parse_harness(
    text: &str,
    acp: Option<AcpAccountSpec>,
) -> Result<(HarnessKind, Option<AcpAccountSpec>), ErrorBody> {
    if let Some(id) = text.strip_prefix("acp:") {
        return match acp {
            None => Ok((
                HarnessKind::Acp,
                Some(AcpAccountSpec::Registry { id: id.to_owned() }),
            )),
            Some(AcpAccountSpec::Registry { id: given }) if given == id => Ok((
                HarnessKind::Acp,
                Some(AcpAccountSpec::Registry { id: given }),
            )),
            Some(_) => Err(invalid(format!(
                "The harness \"{text}\" does not match the ACP agent."
            ))),
        };
    }
    let kind: HarnessKind = serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|_| {
            invalid(format!(
                "\"{text}\" is not a harness that this worker knows."
            ))
        })?;
    match kind {
        HarnessKind::Pi => Err(invalid(
            "This worker cannot run Pi accounts yet. Choose another provider.",
        )),
        HarnessKind::Acp => match acp {
            Some(AcpAccountSpec::Registry { id }) => {
                if id.parse::<ToolId>().is_err() && format!("acp:{id}").parse::<ToolId>().is_err() {
                    return Err(invalid(format!("\"{id}\" is not a valid ACP registry ID.")));
                }
                Ok((kind, Some(AcpAccountSpec::Registry { id })))
            }
            Some(AcpAccountSpec::Command { program, args }) => {
                if program.trim().is_empty() {
                    return Err(invalid("Give the program of the ACP agent."));
                }
                Ok((kind, Some(AcpAccountSpec::Command { program, args })))
            }
            None => Err(invalid(
                "An ACP account needs an agent from the registry or a command.",
            )),
        },
        other if acp.is_some() => Err(invalid(format!(
            "Only an `acp` account takes an ACP agent, not {}.",
            other.display_name()
        ))),
        other => Ok((other, None)),
    }
}

fn check_label(label: &str) -> Result<String, ErrorBody> {
    let label = label.trim();
    if label.is_empty() {
        return Err(invalid("Give the account a name."));
    }
    if label.chars().count() > LABEL_MAX_CHARS {
        return Err(invalid(format!(
            "The account name is longer than {LABEL_MAX_CHARS} characters. Use a shorter name."
        )));
    }
    Ok(label.to_owned())
}

fn not_found(account: AccountId) -> ErrorBody {
    ErrorBody::new(
        ErrorCode::NotFound,
        format!("This worker has no account {account}."),
    )
}

impl Core {
    /// The profile folder of an account.
    pub fn profile_dir(&self, account: &AccountConfig) -> std::path::PathBuf {
        profile_dir(
            &self.consort_home,
            account.id,
            account.profile_dir.as_deref(),
        )
    }

    /// The account with its profile variables and secret variables in
    /// `runtime`, for an account that Consort made. Other accounts come
    /// back as they are.
    pub fn with_profile(&self, account: &AccountConfig) -> Result<AccountConfig, ErrorBody> {
        let mut prepared = account.clone();
        if !account.profile {
            return Ok(prepared);
        }
        let dir = self.profile_dir(account);
        prepare_profile(account.harness, &dir).map_err(worker_error)?;
        let mut env = profile_env(account.harness, &dir);
        // OpenCode gets its own `HOME`, so its tools need their real places.
        if account.harness == HarnessKind::OpenCode
            && let Some(real_home) = consort_harness::account::user_home()
        {
            env.extend(crate::profile::tool_path_pins(&real_home, &|name| {
                std::env::var_os(name).is_some_and(|v| !v.is_empty())
            }));
        }
        env.extend(read_secrets(&dir).map_err(worker_error)?);
        prepared.runtime.env = env;
        prepared.runtime.env_remove = LOGIN_VARIABLES.iter().map(|v| (*v).to_owned()).collect();
        Ok(prepared)
    }

    /// The account ready to run: the profile, and the launch of its managed
    /// tool. A missing tool is installed first, so this can take minutes.
    /// The lease keeps the tool from an update while the caller uses it.
    pub async fn prepare_for_run(
        &self,
        account: &AccountConfig,
        adapter: &dyn Harness,
    ) -> Result<(AccountConfig, Option<ToolLease>), ErrorBody> {
        let mut prepared = self.with_profile(account)?;
        let Some(tool) = adapter.managed_tool(&prepared) else {
            return Ok((prepared, None));
        };
        let lease = self.tools.lease(&tool);
        let installed = self.tools.store().installed(&tool).is_some();
        if !installed {
            self.inventory_changed();
        }
        let launch = self.tools.ensure(&tool).await;
        if !installed {
            self.inventory_changed();
        }
        let launch = launch.map_err(|err| {
            ErrorBody::new(
                ErrorCode::ToolInstallFailed,
                format!(
                    "Could not install {}: {err}. Check the network of the worker, then try again.",
                    tool_name(&tool)
                ),
            )
        })?;
        prepared.runtime.launch = Some(managed_launch(launch));
        Ok((prepared, Some(lease)))
    }

    /// Adds the boundary that a harness needs. It fails closed: a harness
    /// that is not proven, or a machine with no boundary, starts nothing.
    pub fn apply_boundary(
        &self,
        account: &mut AccountConfig,
        adapter: &dyn Harness,
    ) -> Result<(), ErrorBody> {
        use consort_harness::boundary::{self, BoundaryConfig, BoundarySupport};
        match adapter.boundary_support(account) {
            BoundarySupport::NotNeeded => Ok(()),
            BoundarySupport::Off(reason) => Err(ErrorBody::new(ErrorCode::PolicyDenied, reason)),
            BoundarySupport::Proven => {
                let kind = boundary::detect()
                    .map_err(|err| ErrorBody::new(ErrorCode::PolicyDenied, err.to_string()))?;
                let mut read_only = vec![self.tools.store().root().to_path_buf()];
                if let Some(binary) = &account.binary
                    && let Some(found) = consort_harness::process::resolve_program(binary)
                    && let Ok(real) = std::fs::canonicalize(found)
                    && let Some(parent) = real.parent()
                {
                    read_only.push(parent.to_path_buf());
                }
                account.runtime.boundary = Some(BoundaryConfig {
                    kind,
                    read_only,
                    helper: std::env::current_exe().ok(),
                });
                Ok(())
            }
        }
    }

    /// `command_check.run`: one question to a model of an account, inside the
    /// boundary. Any failure is an error, and the hub then asks the user.
    pub(crate) async fn command_check(
        self: &Arc<Self>,
        run: consort_proto::session_host_link::CommandCheckRun,
    ) -> Result<consort_proto::session_host_link::CommandCheckReply, ErrorBody> {
        let account = self
            .config_snapshot()
            .account(run.account_id)
            .cloned()
            .ok_or_else(|| not_found(run.account_id))?;
        let adapter = self
            .harnesses
            .get(&account.harness)
            .cloned()
            .ok_or_else(|| not_found(run.account_id))?;
        let (mut prepared, _lease) = self.prepare_for_run(&account, adapter.as_ref()).await?;
        self.apply_boundary(&mut prepared, adapter.as_ref())?;
        let folder = self
            .config_dir
            .join("checks")
            .join(uuid::Uuid::now_v7().to_string());
        std::fs::create_dir_all(&folder).map_err(|err| {
            HostError::io("could not make a folder for the check", err).to_error_body()
        })?;
        let answer = adapter
            .one_shot(
                &prepared,
                &folder,
                consort_harness::OneShot {
                    model: run.model,
                    system: run.system,
                    prompt: run.prompt,
                },
            )
            .await;
        let _ = std::fs::remove_dir_all(&folder);
        answer
            .map(|text| consort_proto::session_host_link::CommandCheckReply { text })
            .map_err(|err| HostError::Harness(err).to_error_body())
    }

    /// The names of the secret variables of an account.
    pub fn secret_names(&self, account: &AccountConfig) -> Vec<String> {
        if !account.profile {
            return Vec::new();
        }
        read_secrets(&self.profile_dir(account))
            .map(|s| s.into_keys().collect())
            .unwrap_or_default()
    }

    /// The tags of the inventory: the detected tags, then the valid tags
    /// from `session-host.toml`.
    pub fn inventory_tags(&self, config: &HostToml) -> Vec<String> {
        let mut tags = self.detected_tags.clone();
        for tag in &config.tags {
            match normalize_tags([tag]) {
                Ok(valid) => tags.extend(valid),
                Err(err) => {
                    tracing::warn!(tag = %tag, "session-host.toml has a tag that is not valid, so the inventory leaves it out: {err}");
                }
            }
        }
        let mut seen = Vec::new();
        tags.retain(|t| {
            let new = !seen.contains(t);
            seen.push(t.clone());
            new
        });
        tags
    }

    /// Changes `session-host.toml`: the change runs on a copy, the copy is saved
    /// through a temporary file, and then it replaces the loaded config.
    pub fn update_config<T>(
        &self,
        change: impl FnOnce(&mut HostToml) -> Result<T, ErrorBody>,
    ) -> Result<T, ErrorBody> {
        let mut guard = match self.config.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut copy = guard.clone();
        let out = change(&mut copy)?;
        copy.save(&self.config_dir).map_err(worker_error)?;
        *guard = copy;
        Ok(out)
    }

    /// Probes one account again, keeps the result, and sends the
    /// inventory.
    pub async fn reprobe(self: &Arc<Self>, account_id: AccountId) -> Option<AccountStatus> {
        let account = self.config_snapshot().account(account_id).cloned()?;
        let status = self.probe_account(&account).await;
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.insert(account_id, status.clone());
        }
        self.send_inventory(false);
        Some(status)
    }

    /// Runs an account, sign-in or tool command.
    pub async fn account_command(
        self: &Arc<Self>,
        command: HubToSessionHost,
    ) -> Result<(), ErrorBody> {
        match command {
            HubToSessionHost::AccountCreate(create) => self.create_account(create).await,
            HubToSessionHost::AccountUpdate(update) => self.update_account(update).await,
            HubToSessionHost::AccountRemove(remove) => self.remove_account(remove).await,
            HubToSessionHost::AccountSignOut(sign_out) => self.sign_out_command(sign_out).await,
            HubToSessionHost::SignInStart(start) => self.start_sign_in(start),
            HubToSessionHost::SignInCode(code) => self.sign_in_code(code),
            HubToSessionHost::SignInCancel(cancel) => {
                self.cancel_sign_in(cancel.sign_in_id);
                Ok(())
            }
            HubToSessionHost::ToolsCheck(_) => {
                let core = self.clone();
                tokio::spawn(async move {
                    core.tools.store().check_updates().await;
                    core.send_inventory(false);
                });
                Ok(())
            }
            HubToSessionHost::ToolsInstall(install) => {
                let tool: ToolId = install.tool.as_str().parse().map_err(|_| {
                    invalid(format!(
                        "\"{}\" is not a tool that this worker can install.",
                        install.tool
                    ))
                })?;
                self.tools.request_install(tool);
                Ok(())
            }
            _ => Err(ErrorBody::internal()),
        }
    }

    async fn create_account(
        self: &Arc<Self>,
        create: AccountCreateCommand,
    ) -> Result<(), ErrorBody> {
        let label = check_label(&create.label)?;
        let (kind, acp) = parse_harness(&create.harness, create.acp)?;
        if !self.harnesses.contains_key(&kind) {
            return Err(invalid(format!(
                "This worker build has no adapter for {}.",
                kind.display_name()
            )));
        }
        check_secrets(create.env.iter())?;
        if let Some(existing) = self.config_snapshot().account(create.account_id) {
            // The hub sends a command again after a lost answer. The same
            // account is a repeat, anything else is a conflict.
            if existing.harness == kind && existing.label == label && existing.profile {
                return Ok(());
            }
            return Err(ErrorBody::conflict(format!(
                "This worker already has an account {}.",
                create.account_id
            )));
        }
        let mut account = AccountConfig::default_for(kind);
        account.id = create.account_id;
        account.label = label;
        account.profile = true;
        if let Some(AcpAccountSpec::Command { program, args }) = &acp {
            // A custom command runs as it is, like a hand-written binary.
            account.binary = Some(program.clone());
            account.args = args.clone();
        }
        account.acp = acp;
        let taken: Vec<String> = self
            .config_snapshot()
            .accounts
            .iter()
            .filter_map(|a| a.profile_dir.clone())
            .collect();
        account.profile_dir = Some(free_profile_name(
            &self.consort_home,
            &account.label,
            &taken,
        ));
        let dir = self.profile_dir(&account);
        prepare_profile(kind, &dir).map_err(worker_error)?;
        let secrets = create.env.into_map();
        if let Err(err) = write_secrets(&dir, &secrets) {
            let _ = remove_profile(&dir);
            return Err(worker_error(err));
        }
        let added = account.clone();
        if let Err(err) = self.update_config(move |config| {
            config.accounts.push(added);
            Ok(())
        }) {
            let _ = remove_profile(&dir);
            return Err(err);
        }
        tracing::info!(account = %account.id, harness = %kind, secrets = ?secrets.keys().collect::<Vec<_>>(), "added an account");
        let core = self.clone();
        let codex_key = (kind == HarnessKind::Codex)
            .then(|| secrets.get("OPENAI_API_KEY").cloned())
            .flatten();
        tokio::spawn(async move {
            if let Some(key) = codex_key {
                core.codex_api_key(account.id, &key).await;
            }
            core.reprobe(account.id).await;
        });
        self.send_inventory(false);
        Ok(())
    }

    /// Stores an OpenAI API key in the Codex home of an account, because
    /// Codex does not read it from the environment.
    pub(crate) async fn codex_api_key(
        self: &Arc<Self>,
        account_id: AccountId,
        key: &str,
    ) -> Option<String> {
        let account = self.config_snapshot().account(account_id).cloned()?;
        let adapter = self.harnesses.get(&account.harness).cloned()?;
        let result = match self.prepare_for_run(&account, adapter.as_ref()).await {
            Ok((prepared, _lease)) => consort_harness::codex::login::api_key_login(&prepared, key)
                .await
                .map_err(|e| e.to_string()),
            Err(err) => Err(err.message),
        };
        match result {
            Ok(()) => None,
            Err(err) => {
                tracing::warn!(account = %account_id, error = %err, "Codex did not take the API key");
                Some(err)
            }
        }
    }

    async fn update_account(
        self: &Arc<Self>,
        update: AccountUpdateCommand,
    ) -> Result<(), ErrorBody> {
        let account = self
            .config_snapshot()
            .account(update.account_id)
            .cloned()
            .ok_or_else(|| not_found(update.account_id))?;
        let label = update.label.as_deref().map(check_label).transpose()?;
        let env_change = !update.env_set.is_empty() || !update.env_remove.is_empty();
        if env_change && !account.profile {
            return Err(invalid(
                "Consort did not make this account, so the app cannot change its variables. Remove it, and add it again in Settings > Providers.",
            ));
        }
        check_secrets(update.env_set.iter())?;
        let mut codex_key = None;
        if env_change {
            let dir = self.profile_dir(&account);
            let mut secrets = read_secrets(&dir).map_err(worker_error)?;
            for name in &update.env_remove {
                secrets.remove(name);
            }
            for (name, value) in update.env_set.iter() {
                secrets.insert(name.to_owned(), value.to_owned());
            }
            write_secrets(&dir, &secrets).map_err(worker_error)?;
            if account.harness == HarnessKind::Codex {
                codex_key = update.env_set.get("OPENAI_API_KEY").map(str::to_owned);
            }
            tracing::info!(account = %account.id, set = ?update.env_set.names(), removed = ?update.env_remove, "changed the secret variables of an account");
        }
        if let Some(label) = label
            && label != account.label
        {
            self.update_config(|config| {
                let entry = config
                    .accounts
                    .iter_mut()
                    .find(|a| a.id == account.id)
                    .ok_or_else(|| not_found(account.id))?;
                entry.label = label;
                Ok(())
            })?;
        }
        self.send_inventory(false);
        if env_change {
            let core = self.clone();
            tokio::spawn(async move {
                if let Some(key) = codex_key {
                    core.codex_api_key(account.id, &key).await;
                }
                core.reprobe(account.id).await;
            });
        }
        Ok(())
    }

    async fn sign_out_command(
        self: &Arc<Self>,
        command: AccountSignOutCommand,
    ) -> Result<(), ErrorBody> {
        let account = self
            .config_snapshot()
            .account(command.account_id)
            .cloned()
            .ok_or_else(|| not_found(command.account_id))?;
        let result = self.sign_out(&account).await;
        let core = self.clone();
        tokio::spawn(async move {
            core.reprobe(account.id).await;
        });
        result
    }

    /// Signs an account out with the harness's own command, and deletes its
    /// API keys from `env.json`.
    pub async fn sign_out(self: &Arc<Self>, account: &AccountConfig) -> Result<(), ErrorBody> {
        let mut prepared = self.with_profile(account)?;
        let adapter = self.harnesses.get(&account.harness).cloned();
        // Sign out with the installed tool. A tool that is not installed
        // has no login to remove.
        let mut _lease = None;
        if let Some(tool) = adapter.as_ref().and_then(|a| a.managed_tool(&prepared)) {
            match self.tools.store().launch(&tool) {
                Ok(launch) => {
                    prepared.runtime.launch = Some(managed_launch(launch));
                    _lease = Some(self.tools.lease(&tool));
                }
                Err(_) => {
                    tracing::debug!(account = %account.id, "the managed tool is not installed, so there is no login to sign out");
                }
            }
        }
        let managed_missing = adapter
            .as_ref()
            .and_then(|a| a.managed_tool(&prepared))
            .is_some()
            && prepared.runtime.launch.is_none();
        let result = if managed_missing {
            Ok(())
        } else {
            match account.harness {
                HarnessKind::ClaudeCode => {
                    match consort_harness::signin::claude::ClaudeCommand::for_account(&prepared) {
                        Ok(command) => command.logout().await.map_err(|e| e.to_string()),
                        Err(_) => Ok(()),
                    }
                }
                HarnessKind::Codex => consort_harness::codex::login::logout(&prepared)
                    .await
                    .map_err(|e| e.to_string()),
                HarnessKind::OpenCode | HarnessKind::GeminiCli | HarnessKind::Acp => {
                    consort_harness::acp::logout(account.harness, &prepared)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
                HarnessKind::Pi => Ok(()),
            }
        };
        if account.profile {
            let status = self
                .statuses
                .lock()
                .ok()
                .and_then(|s| s.get(&account.id).cloned());
            let dir = self.profile_dir(account);
            let mut secrets = read_secrets(&dir).map_err(worker_error)?;
            let before = secrets.len();
            for variable in key_variables(account, status.as_ref()) {
                secrets.remove(&variable);
            }
            if secrets.len() != before {
                write_secrets(&dir, &secrets).map_err(worker_error)?;
            }
        }
        result.map_err(|err| {
            ErrorBody::new(
                ErrorCode::SignInFailed,
                format!("Could not sign out of {}: {err}", account.label),
            )
        })
    }

    async fn remove_account(
        self: &Arc<Self>,
        remove: AccountRemoveCommand,
    ) -> Result<(), ErrorBody> {
        let Some(account) = self.config_snapshot().account(remove.account_id).cloned() else {
            // A repeat of a remove that ran.
            return Ok(());
        };
        // Sign-ins of the account stop.
        let running: Vec<_> = self
            .sign_ins
            .lock()
            .map(|s| {
                s.iter()
                    .filter(|(_, c)| c.account_id == account.id)
                    .map(|(id, _)| *id)
                    .collect()
            })
            .unwrap_or_default();
        for id in running {
            self.cancel_sign_in(id);
        }
        self.stop_account_sessions(account.id).await;
        if let Err(err) = self.sign_out(&account).await {
            tracing::warn!(account = %account.id, error = %err, "could not sign out an account before removing it");
        }
        self.update_config(|config| {
            config.accounts.retain(|a| a.id != account.id);
            Ok(())
        })?;
        if account.profile {
            remove_profile(&self.profile_dir(&account)).map_err(worker_error)?;
        }
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.remove(&account.id);
        }
        tracing::info!(account = %account.id, "removed an account");
        self.send_inventory(false);
        Ok(())
    }

    /// Ends the open sessions of an account as lost, and waits for them to
    /// close, so the profile folder can go.
    async fn stop_account_sessions(&self, account: AccountId) {
        let records = self.store.open_sessions().unwrap_or_default();
        let ids: Vec<_> = records
            .iter()
            .filter(|r| r.start.account_id == account)
            .map(|r| r.start.session_id)
            .collect();
        let mut waiting = Vec::new();
        if let Ok(sessions) = self.sessions.lock() {
            for id in &ids {
                if let Some(handle) = sessions.get(id) {
                    let _ = handle.jobs.send(Job::AccountRemoved);
                    waiting.push(*id);
                }
            }
        }
        for _ in 0..150 {
            let open = self
                .sessions
                .lock()
                .map(|s| waiting.iter().any(|id| s.contains_key(id)))
                .unwrap_or(false);
            if !open {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Checks for tool updates once a day, and runs the installs that wait
/// until their tools are idle.
pub(crate) async fn tools_loop(core: Arc<Core>, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(Duration::from_secs(60 * 60));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if core.tools.check_updates && core.tools.check_due() {
                    core.tools.store().check_updates().await;
                    core.send_inventory(false);
                }
            }
            () = core.tools.wake.notified() => {}
            _ = shutdown.changed() => break,
        }
        for tool in core.tools.take_ready() {
            let core = core.clone();
            tokio::spawn(async move {
                // The store marks the tool as installing when the install
                // starts. Send that state soon after.
                let notify = core.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    notify.send_inventory(false);
                });
                let result = core.tools.store().install(&tool, None).await;
                match result {
                    Ok(installed) => {
                        tracing::info!(tool = %tool, version = %installed.version, "installed a managed tool");
                    }
                    Err(err) => {
                        tracing::warn!(tool = %tool, error = %err, "could not install a managed tool");
                    }
                }
                core.probe_accounts().await;
                core.send_inventory(false);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_harness_offers_its_methods() {
        assert_eq!(
            harness_methods(HarnessKind::ClaudeCode),
            vec![
                SignInMethod::ClaudePlan,
                SignInMethod::ClaudeConsole,
                SignInMethod::ApiKey {
                    variable: "ANTHROPIC_API_KEY".into()
                }
            ]
        );
        assert_eq!(harness_methods(HarnessKind::Codex).len(), 3);
        assert_eq!(
            harness_methods(HarnessKind::GeminiCli),
            vec![SignInMethod::ApiKey {
                variable: "GEMINI_API_KEY".into()
            }]
        );
        let opencode: Vec<_> = harness_methods(HarnessKind::OpenCode)
            .into_iter()
            .map(|m| match m {
                SignInMethod::ApiKey { variable } => variable,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(opencode, OPENCODE_KEY_VARIABLES);
        // A hand-written account signs in with its own command.
        let hand = AccountConfig::default_for(HarnessKind::ClaudeCode);
        assert!(sign_in_methods(&hand, None).is_empty());
        let mut agent = AccountConfig::default_for(HarnessKind::Acp);
        agent.profile = true;
        let status = AccountStatus {
            sign_in_methods: vec![SignInMethod::AcpAgent {
                method_id: "m".into(),
                name: "M".into(),
            }],
            ..AccountStatus::default()
        };
        assert_eq!(sign_in_methods(&agent, Some(&status)).len(), 1);
    }

    #[test]
    fn harness_names_parse() {
        assert_eq!(
            parse_harness("claude", None).unwrap().0,
            HarnessKind::ClaudeCode
        );
        assert_eq!(
            parse_harness("acp:goose", None).unwrap(),
            (
                HarnessKind::Acp,
                Some(AcpAccountSpec::Registry { id: "goose".into() })
            )
        );
        assert!(parse_harness("acp", None).is_err());
        assert!(parse_harness("pi", None).is_err());
        assert!(parse_harness("nope", None).is_err());
        assert!(
            parse_harness(
                "codex",
                Some(AcpAccountSpec::Registry { id: "goose".into() })
            )
            .is_err()
        );
        let mut agent = AccountConfig::default_for(HarnessKind::Acp);
        agent.acp = Some(AcpAccountSpec::Registry { id: "goose".into() });
        assert_eq!(inventory_harness(&agent), "acp:goose");
        agent.acp = Some(AcpAccountSpec::Command {
            program: "my-agent".into(),
            args: vec![],
        });
        assert_eq!(inventory_harness(&agent), "acp:custom");
        assert_eq!(
            inventory_harness(&AccountConfig::default_for(HarnessKind::GeminiCli)),
            "gemini"
        );
    }

    #[test]
    fn secret_names_and_values_are_checked() {
        assert!(check_secrets([("OPENAI_API_KEY", "sk")].into_iter()).is_ok());
        assert!(check_secrets([("BAD-NAME", "x")].into_iter()).is_err());
        assert!(check_secrets([("A", " ")].into_iter()).is_err());
        let err = check_secrets([("CODEX_HOME", "/x")].into_iter()).unwrap_err();
        assert!(err.message.contains("profile folder"));
    }
}
