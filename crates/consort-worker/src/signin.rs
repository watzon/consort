//! Sign-in flows: `sign_in.start`, `sign_in.code` and `sign_in.cancel`.
//!
//! Each sign-in runs in its own task and reports each state to the hub with
//! a `sign_in.update` event. The flows use the harness's own sign-in (see
//! `docs/harness-adapters.md`, "Sign-in"):
//!
//! - Claude plan or Console: `claude auth login` in a pseudo-terminal. The
//!   worker sends `needs_code` with the URL, then types the code that the
//!   user pastes in the app.
//! - ChatGPT: `codex app-server` with `account/login/start`. One browser
//!   sign-in runs at a time, because Codex listens on a fixed port.
//! - API key: the key comes in `env.json` with `account.create` or
//!   `account.update`, before or after the sign-in starts. The worker waits
//!   for it, probes the account again (and stores the key in Codex, which
//!   does not read it from the environment).
//! - ACP agent method: `authenticate` on a short-lived agent process.
//!
//! The worker never logs the code or a secret value.

use std::sync::Arc;
use std::time::Duration;

use consort_harness::codex::login::{ChatgptLogin, ChatgptStep};
use consort_harness::signin::claude::{ClaudeCommand, ClaudeLogin, LoginKind};
use consort_harness::{AccountConfig, AuthState, HarnessError, HarnessKind};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::ids::{AccountId, SignInId};
use consort_proto::model::{SIGN_IN_IDLE_EXPIRY_MS, SignInMethod, SignInState};
use consort_proto::worker_link::{SignInCodeCommand, SignInStartCommand, SignInUpdate};
use tokio::sync::{mpsc, watch};

use crate::core::Core;
use crate::profile::read_secrets;

/// How long `claude auth login` may take to show its URL. The first run of
/// a new Claude Code can be slow.
const URL_TIMEOUT: Duration = Duration::from_secs(90);
/// How long the flow waits for the user: the idle time after which the hub
/// expires a sign-in.
const USER_TIMEOUT: Duration = Duration::from_millis(SIGN_IN_IDLE_EXPIRY_MS as u64);
/// How often an API key sign-in looks for the saved key.
const KEY_POLL: Duration = Duration::from_millis(250);
/// How long `claude auth login` may take after the code.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(120);

/// A running sign-in.
pub(crate) struct SignInControl {
    /// The account.
    pub account_id: AccountId,
    /// Takes the code of a Claude sign-in.
    code: Option<mpsc::UnboundedSender<String>>,
    /// Turns true to cancel the sign-in.
    cancel: watch::Sender<bool>,
}

/// Waits until the sign-in is cancelled.
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|c| *c).await.is_err() {
        // The control is gone: nobody can cancel any more.
        std::future::pending::<()>().await;
    }
}

fn failed(message: impl Into<String>) -> SignInState {
    SignInState::Failed {
        message: message.into(),
    }
}

/// The text of a harness error without the adapter's prefix.
fn reason(err: &HarnessError) -> String {
    match err {
        HarnessError::Spawn(m)
        | HarnessError::SignedOut(m)
        | HarnessError::Protocol(m)
        | HarnessError::Timeout(m) => m.clone(),
        other => other.to_string(),
    }
}

/// Ends a message with a period.
fn sentence(text: &str) -> String {
    let text = text.trim();
    if text.ends_with(['.', '!', '?']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

/// Whether a method fits the harness of an account.
fn method_fits(kind: HarnessKind, method: &SignInMethod) -> bool {
    match method {
        SignInMethod::ClaudePlan | SignInMethod::ClaudeConsole => kind == HarnessKind::ClaudeCode,
        SignInMethod::ChatgptDevice | SignInMethod::ChatgptBrowser => kind == HarnessKind::Codex,
        SignInMethod::ApiKey { variable } => crate::profile::valid_variable_name(variable),
        SignInMethod::AcpAgent { .. } | SignInMethod::Terminal { .. } => matches!(
            kind,
            HarnessKind::Acp | HarnessKind::OpenCode | HarnessKind::GeminiCli
        ),
    }
}

impl Core {
    /// Sends the state of a sign-in to the hub.
    fn send_sign_in(&self, sign_in_id: SignInId, state: SignInState) {
        let update = SignInUpdate { sign_in_id, state };
        if let Err(err) = self.link.send_event(&update) {
            tracing::debug!(sign_in = %sign_in_id, error = %err, "could not send a sign-in update");
        }
    }

    /// Starts a sign-in. It runs in its own task, and this returns at once.
    pub(crate) fn start_sign_in(
        self: &Arc<Self>,
        start: SignInStartCommand,
    ) -> Result<(), ErrorBody> {
        let account = self
            .config_snapshot()
            .account(start.account_id)
            .cloned()
            .ok_or_else(|| {
                ErrorBody::new(
                    ErrorCode::NotFound,
                    format!("This worker has no account {}.", start.account_id),
                )
            })?;
        if !account.profile {
            return Err(ErrorBody::new(
                ErrorCode::Invalid,
                "This account is written in worker.toml by hand. Sign in with the harness's own command on the worker.",
            ));
        }
        if !method_fits(account.harness, &start.method) {
            return Err(ErrorBody::new(
                ErrorCode::Invalid,
                format!(
                    "{} cannot sign in with this method. Choose another method.",
                    account.harness.display_name()
                ),
            ));
        }
        let (cancel, cancel_rx) = watch::channel(false);
        let (code_tx, code_rx) = mpsc::unbounded_channel();
        {
            let mut sign_ins = match self.sign_ins.lock() {
                Ok(s) => s,
                Err(p) => p.into_inner(),
            };
            if sign_ins.contains_key(&start.sign_in_id) {
                // The hub sent the command again. The flow runs already.
                return Ok(());
            }
            let takes_code = matches!(
                start.method,
                SignInMethod::ClaudePlan | SignInMethod::ClaudeConsole
            );
            sign_ins.insert(
                start.sign_in_id,
                SignInControl {
                    account_id: account.id,
                    code: takes_code.then_some(code_tx),
                    cancel,
                },
            );
        }
        tracing::info!(sign_in = %start.sign_in_id, account = %account.id, method = ?start.method, "a sign-in starts");
        let core = self.clone();
        tokio::spawn(async move {
            let id = start.sign_in_id;
            let state = core
                .run_sign_in(id, &account, start.method, code_rx, cancel_rx)
                .await;
            let succeeded = matches!(state, SignInState::Succeeded { .. });
            match &state {
                SignInState::Failed { message } => {
                    tracing::info!(sign_in = %id, %message, "a sign-in failed");
                }
                other => tracing::info!(sign_in = %id, state = ?other, "a sign-in ended"),
            }
            if let Ok(mut sign_ins) = core.sign_ins.lock() {
                sign_ins.remove(&id);
            }
            core.send_sign_in(id, state);
            if succeeded {
                // A new login ends the expiry that a turn found.
                core.clear_login_expired(account.id);
                core.reprobe(account.id).await;
            } else {
                core.send_inventory(false);
            }
        });
        Ok(())
    }

    /// Gives the code that the user pasted to a Claude sign-in. The code is
    /// not stored or logged.
    pub(crate) fn sign_in_code(&self, command: SignInCodeCommand) -> Result<(), ErrorBody> {
        let sender = self
            .sign_ins
            .lock()
            .ok()
            .and_then(|s| s.get(&command.sign_in_id).map(|c| c.code.clone()));
        match sender {
            None => Err(ErrorBody::new(
                ErrorCode::NotFound,
                "This sign-in is not running any more. Start the sign-in again.",
            )),
            Some(None) => Err(ErrorBody::new(
                ErrorCode::Invalid,
                "This sign-in does not take a code.",
            )),
            Some(Some(sender)) => sender.send(command.code).map_err(|_| {
                ErrorBody::new(
                    ErrorCode::NotFound,
                    "This sign-in is not running any more. Start the sign-in again.",
                )
            }),
        }
    }

    /// Cancels a sign-in. A sign-in that is not running is no error.
    pub(crate) fn cancel_sign_in(&self, sign_in_id: SignInId) {
        if let Ok(sign_ins) = self.sign_ins.lock()
            && let Some(control) = sign_ins.get(&sign_in_id)
        {
            control.cancel.send_replace(true);
        }
    }

    /// Cancels every sign-in, when the worker stops.
    pub(crate) fn cancel_all_sign_ins(&self) {
        if let Ok(sign_ins) = self.sign_ins.lock() {
            for control in sign_ins.values() {
                control.cancel.send_replace(true);
            }
        }
    }

    /// Runs one sign-in to its final state.
    async fn run_sign_in(
        self: &Arc<Self>,
        id: SignInId,
        account: &AccountConfig,
        method: SignInMethod,
        code: mpsc::UnboundedReceiver<String>,
        mut cancel: watch::Receiver<bool>,
    ) -> SignInState {
        match method {
            SignInMethod::ClaudePlan => {
                self.claude_sign_in(id, account, LoginKind::Plan, code, cancel)
                    .await
            }
            SignInMethod::ClaudeConsole => {
                self.claude_sign_in(id, account, LoginKind::Console, code, cancel)
                    .await
            }
            SignInMethod::ChatgptDevice => self.chatgpt_sign_in(id, account, true, cancel).await,
            SignInMethod::ChatgptBrowser => self.chatgpt_sign_in(id, account, false, cancel).await,
            SignInMethod::ApiKey { variable } => {
                tokio::select! {
                    state = self.api_key_sign_in(id, account, &variable) => state,
                    () = cancelled(&mut cancel) => SignInState::Cancelled,
                }
            }
            SignInMethod::AcpAgent { method_id, .. } => {
                self.send_sign_in(id, SignInState::Verifying);
                tokio::select! {
                    state = self.acp_sign_in(account, &method_id) => state,
                    () = cancelled(&mut cancel) => SignInState::Cancelled,
                }
            }
            SignInMethod::Terminal { command } => failed(format!(
                "Consort cannot run this sign-in. Run `{command}` in a terminal on the worker, then check the account again."
            )),
        }
    }

    /// The account ready to run, or the failed state.
    async fn ready_account(
        &self,
        account: &AccountConfig,
    ) -> Result<(AccountConfig, Option<crate::tools::ToolLease>), SignInState> {
        let Some(adapter) = self.harnesses.get(&account.harness).cloned() else {
            return Err(failed(format!(
                "This worker build has no adapter for {}.",
                account.harness.display_name()
            )));
        };
        self.prepare_for_run(account, adapter.as_ref())
            .await
            .map_err(|err| failed(err.message))
    }

    async fn claude_sign_in(
        &self,
        id: SignInId,
        account: &AccountConfig,
        kind: LoginKind,
        mut code: mpsc::UnboundedReceiver<String>,
        mut cancel: watch::Receiver<bool>,
    ) -> SignInState {
        let prepared = tokio::select! {
            ready = self.ready_account(account) => ready,
            () = cancelled(&mut cancel) => return SignInState::Cancelled,
        };
        let (prepared, _lease) = match prepared {
            Ok(ready) => ready,
            Err(state) => return state,
        };
        let command = match ClaudeCommand::for_account(&prepared) {
            Ok(command) => command,
            Err(err) => {
                return failed(format!(
                    "Cannot run Claude Code: {}",
                    sentence(&reason(&err))
                ));
            }
        };
        let cwd = self.profile_dir(account);
        let mut login = match ClaudeLogin::start(&command, kind, &cwd) {
            Ok(login) => login,
            Err(err) => {
                return failed(format!(
                    "Cannot start `claude auth login`: {}",
                    sentence(&reason(&err))
                ));
            }
        };
        let url = tokio::select! {
            url = login.url(URL_TIMEOUT) => url,
            () = cancelled(&mut cancel) => return SignInState::Cancelled,
        };
        let url = match url {
            Ok(url) => url,
            Err(err) => {
                return failed(format!(
                    "Claude Code did not show a sign-in page: {} Try again.",
                    sentence(&reason(&err))
                ));
            }
        };
        self.send_sign_in(id, SignInState::NeedsCode { url });
        let pasted = tokio::select! {
            pasted = code.recv() => pasted,
            () = cancelled(&mut cancel) => return SignInState::Cancelled,
            () = tokio::time::sleep(USER_TIMEOUT) => return SignInState::Expired,
        };
        let Some(pasted) = pasted else {
            return SignInState::Cancelled;
        };
        let typed = login.send_code(&pasted);
        drop(pasted);
        if let Err(err) = typed {
            return failed(format!(
                "Could not give the code to Claude Code: {} Start the sign-in again.",
                sentence(&reason(&err))
            ));
        }
        self.send_sign_in(id, SignInState::Verifying);
        let exit = tokio::select! {
            exit = login.wait(VERIFY_TIMEOUT) => exit,
            () = cancelled(&mut cancel) => return SignInState::Cancelled,
        };
        match exit {
            Ok(true) => {}
            Ok(false) => {
                let why = login
                    .last_line()
                    .map(|l| sentence(&l))
                    .unwrap_or_else(|| "Claude Code did not accept the code.".to_owned());
                return failed(format!("{why} Start the sign-in again to get a new code."));
            }
            Err(err) => {
                return failed(format!(
                    "{} Start the sign-in again.",
                    sentence(&reason(&err))
                ));
            }
        }
        match command.status().await {
            Ok(status) if status.logged_in => SignInState::Succeeded {
                detail: status.detail(),
            },
            Ok(_) => {
                failed("Claude Code finished, but it is not signed in. Start the sign-in again.")
            }
            Err(err) => failed(format!(
                "Could not check the sign-in: {} Start the sign-in again.",
                sentence(&reason(&err))
            )),
        }
    }

    async fn chatgpt_sign_in(
        &self,
        id: SignInId,
        account: &AccountConfig,
        device: bool,
        mut cancel: watch::Receiver<bool>,
    ) -> SignInState {
        // Codex takes the browser callback on a fixed port, so one browser
        // sign-in runs at a time.
        let _browser = if device {
            None
        } else {
            match self.browser_sign_in.try_lock() {
                Ok(guard) => Some(guard),
                Err(_) => {
                    return failed(
                        "Another ChatGPT sign-in in a browser runs on this worker. Finish or cancel it, then try again.",
                    );
                }
            }
        };
        let prepared = tokio::select! {
            ready = self.ready_account(account) => ready,
            () = cancelled(&mut cancel) => return SignInState::Cancelled,
        };
        let (prepared, _lease) = match prepared {
            Ok(ready) => ready,
            Err(state) => return state,
        };
        let started = tokio::select! {
            started = ChatgptLogin::start(&prepared, device) => started,
            () = cancelled(&mut cancel) => return SignInState::Cancelled,
        };
        let (mut login, step) = match started {
            Ok(started) => started,
            Err(err) => {
                return failed(format!(
                    "Codex could not start the sign-in: {} Try again.",
                    sentence(&reason(&err))
                ));
            }
        };
        self.send_sign_in(
            id,
            match step {
                ChatgptStep::DeviceCode { url, user_code } => {
                    SignInState::DeviceCode { url, user_code }
                }
                ChatgptStep::OpenUrl { url } => SignInState::OpenUrl { url },
            },
        );
        let done = tokio::select! {
            done = login.wait(USER_TIMEOUT) => Some(done),
            () = cancelled(&mut cancel) => None,
        };
        match done {
            None => {
                login.cancel().await;
                SignInState::Cancelled
            }
            Some(Err(HarnessError::Timeout(_))) => {
                login.cancel().await;
                SignInState::Expired
            }
            Some(Err(err)) => {
                login.close().await;
                failed(format!(
                    "The ChatGPT sign-in failed: {} Start the sign-in again.",
                    sentence(&reason(&err))
                ))
            }
            Some(Ok(())) => {
                self.send_sign_in(id, SignInState::Verifying);
                let info = login.account().await;
                login.close().await;
                match info {
                    Ok(info) => {
                        let status = consort_harness::codex::account_status(&prepared, info);
                        if status.auth_state == AuthState::SignedIn {
                            SignInState::Succeeded {
                                detail: status.email.or(status.auth_detail),
                            }
                        } else {
                            failed(
                                "Codex finished, but it is not signed in. Start the sign-in again.",
                            )
                        }
                    }
                    Err(_) => SignInState::Succeeded { detail: None },
                }
            }
        }
    }

    async fn api_key_sign_in(
        self: &Arc<Self>,
        id: SignInId,
        account: &AccountConfig,
        variable: &str,
    ) -> SignInState {
        // The app can start the sign-in before it saves the key with
        // `account.update`. Wait for the key, as long as the hub waits.
        let dir = self.profile_dir(account);
        let waited = tokio::time::timeout(USER_TIMEOUT, async {
            loop {
                match read_secrets(&dir) {
                    Ok(secrets) => {
                        if let Some(key) = secrets.get(variable) {
                            return Ok(key.clone());
                        }
                    }
                    Err(err) => return Err(format!("Cannot read the saved keys: {err}")),
                }
                tokio::time::sleep(KEY_POLL).await;
            }
        })
        .await;
        let key = match waited {
            Ok(Ok(key)) => key,
            Ok(Err(message)) => return failed(message),
            Err(_) => return SignInState::Expired,
        };
        let key = key.as_str();
        self.send_sign_in(id, SignInState::Verifying);
        if account.harness == HarnessKind::Codex
            && variable == "OPENAI_API_KEY"
            && let Some(err) = self.codex_api_key(account.id, key).await
        {
            return failed(format!(
                "Codex did not take the key: {} Check the key, then save it again.",
                sentence(&err)
            ));
        }
        self.probe_result(account).await
    }

    async fn acp_sign_in(
        self: &Arc<Self>,
        account: &AccountConfig,
        method_id: &str,
    ) -> SignInState {
        let (prepared, _lease) = match self.ready_account(account).await {
            Ok(ready) => ready,
            Err(state) => return state,
        };
        if let Err(err) =
            consort_harness::acp::authenticate(account.harness, &prepared, method_id).await
        {
            return failed(format!(
                "The agent did not sign in: {} Try again.",
                sentence(&reason(&err))
            ));
        }
        drop(_lease);
        self.probe_result(account).await
    }

    /// Probes the account and turns the result into a final state.
    async fn probe_result(self: &Arc<Self>, account: &AccountConfig) -> SignInState {
        let Some(status) = self.reprobe(account.id).await else {
            return failed("The account was removed.");
        };
        if status.auth_state == AuthState::SignedIn {
            SignInState::Succeeded {
                detail: status.email.or(status.plan).or(status.auth_detail),
            }
        } else {
            failed(status.auth_detail.unwrap_or_else(|| {
                "The account is not signed in. Check the key, then save it again.".to_owned()
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_fit_their_harness() {
        assert!(method_fits(
            HarnessKind::ClaudeCode,
            &SignInMethod::ClaudePlan
        ));
        assert!(!method_fits(HarnessKind::Codex, &SignInMethod::ClaudePlan));
        assert!(method_fits(
            HarnessKind::Codex,
            &SignInMethod::ChatgptBrowser
        ));
        assert!(method_fits(
            HarnessKind::OpenCode,
            &SignInMethod::ApiKey {
                variable: "FIREWORKS_API_KEY".into()
            }
        ));
        assert!(!method_fits(
            HarnessKind::OpenCode,
            &SignInMethod::ApiKey {
                variable: "BAD-NAME".into()
            }
        ));
        assert!(method_fits(
            HarnessKind::Acp,
            &SignInMethod::AcpAgent {
                method_id: "x".into(),
                name: "X".into()
            }
        ));
        assert_eq!(sentence("a"), "a.");
        assert_eq!(sentence("a."), "a.");
    }
}
