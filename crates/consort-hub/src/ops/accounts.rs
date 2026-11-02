//! Accounts, sign-ins and managed tools. The hub relays these requests to
//! the session host, which owns every account. A `worker_id` in these requests
//! names the row of the session host.
//!
//! A request while the session host is offline fails at once with
//! `worker_offline`. The hub never queues it: the user is at the screen and
//! needs the answer now, and a sign-in that waits for days is of no use.
//!
//! The hub never stores or logs a secret variable (`env`, `env_set`) or a
//! sign-in code. The values pass through memory from the client to the
//! worker. Logs and error messages name a variable, never its value.

use consort_proto::client_api::*;
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::Empty;
use consort_proto::ids::{AccountId, SignInId, WorkerId};
use consort_proto::model::{
    AcpAccountSpec, SIGN_IN_IDLE_EXPIRY_MS, SecretEnv, SignIn, SignInState,
};
use consort_proto::session_host_link::{
    AccountCreateCommand, AccountRemoveCommand, AccountSignOutCommand, AccountUpdateCommand,
    HubToSessionHost, SignInCancelCommand, SignInCodeCommand, SignInStartCommand, SignInUpdate,
    ToolsCheck, ToolsInstall,
};

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::{machine, sign_ins};

/// The text of a sign-in that was open when its worker went offline.
pub(crate) const SIGN_IN_WORKER_OFFLINE: &str = "The worker went offline. Start the sign-in again.";

const LABEL_MAX_CHARS: usize = 120;
const HARNESS_MAX_CHARS: usize = 40;
const ENV_MAX_VARIABLES: usize = 64;
const ENV_NAME_MAX_CHARS: usize = 128;
/// The most bytes of secret values in one request. A frame holds 1 MiB, so
/// this keeps the request far from the limit.
const ENV_MAX_BYTES: usize = 64 * 1024;
const CODE_MAX_BYTES: usize = 4 * 1024;

/// What a relayed request does, to choose the error code for a worker
/// failure that has no code of its own for the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relay {
    Account,
    SignIn,
    Tool,
}

/// Turns the error of a worker into the error that a client gets.
///
/// Codes that mean something to a client pass through, with the message of
/// the worker. A code that only means something on the link (`unauthorized`,
/// `protocol_mismatch`, `resync_required`, `stale_write`), an `internal`
/// error and a code that the hub does not know become the failure code of
/// the request: `sign_in_failed` for a sign-in, `tool_install_failed` for a
/// tool, and `internal` for an account.
fn map_worker_error(error: ErrorBody, relay: Relay) -> ErrorBody {
    match error.code {
        ErrorCode::Invalid
        | ErrorCode::NotFound
        | ErrorCode::Conflict
        | ErrorCode::PolicyDenied
        | ErrorCode::InvalidTag
        | ErrorCode::NoMatchingAccount
        | ErrorCode::WorkerOffline
        | ErrorCode::WorkerUnknown
        | ErrorCode::SignInFailed
        | ErrorCode::ToolInstallFailed
        | ErrorCode::Unsupported
        | ErrorCode::Unavailable => error,
        ErrorCode::Unauthorized
        | ErrorCode::ProtocolMismatch
        | ErrorCode::StaleWrite
        | ErrorCode::ResyncRequired
        | ErrorCode::Internal
        // Hub-side codes that a worker never sends for these requests.
        | ErrorCode::ConnectorNotAvailable
        | ErrorCode::ComposioNotConfigured
        | ErrorCode::ComposioError
        | ErrorCode::MonidNotConfigured
        | ErrorCode::MonidKeyRefused
        | ErrorCode::MonidError
        | ErrorCode::NeedsUser
        | ErrorCode::SkillBlocked
        | ErrorCode::SkillInvalid
        | ErrorCode::Unknown => {
            let code = match relay {
                Relay::Account => ErrorCode::Internal,
                Relay::SignIn => ErrorCode::SignInFailed,
                Relay::Tool => ErrorCode::ToolInstallFailed,
            };
            ErrorBody::new(code, error.message)
        }
    }
}

/// The shortest secret value that the hub hides in an error message. A
/// shorter text would garble messages, and no API key is that short.
const HIDE_MIN_CHARS: usize = 4;

/// Replaces every secret value in `text` with `<redacted>`.
fn hide_secrets(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_owned();
    for secret in secrets
        .iter()
        .filter(|s| s.chars().count() >= HIDE_MIN_CHARS)
    {
        out = out.replace(secret.as_str(), "<redacted>");
    }
    out
}

fn secret_values(env: &SecretEnv) -> Vec<String> {
    env.iter().map(|(_, value)| value.to_owned()).collect()
}

fn clean_label(label: &str) -> Result<String> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > LABEL_MAX_CHARS {
        return Err(HubError::invalid(
            "The account name needs 1 to 120 characters.",
        ));
    }
    Ok(label.to_owned())
}

fn check_harness(harness: &str, acp: Option<&AcpAccountSpec>) -> Result<String> {
    let harness = harness.trim();
    let valid = !harness.is_empty()
        && harness.chars().count() <= HARNESS_MAX_CHARS
        && harness
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid {
        return Err(HubError::invalid(
            "The harness needs 1 to 40 lowercase letters, digits, - or _, for example claude.",
        ));
    }
    match (harness == "acp", acp) {
        (true, None) => Err(HubError::invalid(
            "An acp account needs the ACP agent: a registry ID or a command.",
        )),
        (false, Some(_)) => Err(HubError::invalid(
            "Only an acp account can have an ACP agent.",
        )),
        (true, Some(AcpAccountSpec::Registry { id })) if id.trim().is_empty() => {
            Err(HubError::invalid("The ACP registry ID is empty."))
        }
        (true, Some(AcpAccountSpec::Command { program, .. })) if program.trim().is_empty() => {
            Err(HubError::invalid("The ACP command needs a program."))
        }
        _ => Ok(harness.to_owned()),
    }
}

fn check_variable_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let valid = name.chars().count() <= ENV_NAME_MAX_CHARS
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        // The message names the variable. A name is not a secret.
        Err(HubError::invalid(format!(
            "The variable name {name:?} is not valid. Use letters, digits and _, and start with a letter or _."
        )))
    }
}

/// Checks the names and the size of secret variables. The messages hold
/// names only.
fn check_env(env: &SecretEnv) -> Result<()> {
    if env.len() > ENV_MAX_VARIABLES {
        return Err(HubError::invalid(
            "An account can have at most 64 secret variables.",
        ));
    }
    let mut bytes = 0;
    for (name, value) in env.iter() {
        check_variable_name(name)?;
        bytes += name.len() + value.len();
    }
    if bytes > ENV_MAX_BYTES {
        return Err(HubError::invalid(
            "The secret variables are too large. Send at most 64 KiB.",
        ));
    }
    Ok(())
}

impl Hub {
    /// The worker, which must be paired and connected. A worker that does
    /// not exist or was removed gives `worker_unknown`. A worker that is not
    /// connected gives `worker_offline`.
    async fn require_online_worker(&self, worker_id: WorkerId) -> Result<()> {
        let worker = self
            .store
            .read(move |conn| machine::worker_or_404(conn, worker_id))
            .await?;
        if worker.revoked_at.is_some() {
            return Err(HubError::new(
                ErrorCode::WorkerUnknown,
                format!("The worker {} was removed.", worker.name),
            ));
        }
        if worker.kind != consort_proto::model::MachineKind::SessionHost {
            return Err(HubError::invalid(
                "A worker holds no accounts or harness tools. They are on the hub machine.",
            ));
        }
        if !self.links.is_online(worker_id) {
            return Err(HubError::Api(ErrorBody::new(
                ErrorCode::WorkerOffline,
                "The session host on the hub machine is offline. Check its service, then try again.",
            )));
        }
        Ok(())
    }

    /// Sends a command to a worker and waits for the answer. The command is
    /// not queued.
    ///
    /// `secrets` are values that the command carries. If the error of the
    /// worker repeats one, the hub hides it before the client sees it.
    async fn relay(
        &self,
        worker_id: WorkerId,
        command: HubToSessionHost,
        relay: Relay,
        secrets: &[String],
    ) -> Result<()> {
        self.require_online_worker(worker_id).await?;
        match self.worker_rpc(worker_id, command).await {
            Ok(_) => Ok(()),
            Err(HubError::Api(mut error)) => {
                error.message = hide_secrets(&error.message, secrets);
                Err(HubError::Api(map_worker_error(error, relay)))
            }
            Err(other) => Err(other),
        }
    }

    pub(crate) async fn account_create(
        &self,
        _caller: &Caller,
        request: AccountCreate,
    ) -> Result<AccountCreateResult> {
        let label = clean_label(&request.label)?;
        let harness = check_harness(&request.harness, request.acp.as_ref())?;
        check_env(&request.env)?;
        // The hub chooses the account ID, so the client can use it at once.
        let account_id = AccountId::new();
        tracing::info!(
            worker_id = %request.worker_id,
            account_id = %account_id,
            harness = %harness,
            variables = ?request.env.names(),
            "the hub asks a worker to create an account"
        );
        let secrets = secret_values(&request.env);
        let command = HubToSessionHost::AccountCreate(AccountCreateCommand {
            account_id,
            harness,
            label,
            acp: request.acp,
            env: request.env,
        });
        self.relay(request.worker_id, command, Relay::Account, &secrets)
            .await?;
        Ok(AccountCreateResult { account_id })
    }

    pub(crate) async fn account_update(
        &self,
        _caller: &Caller,
        request: AccountUpdate,
    ) -> Result<Empty> {
        let label = request.label.as_deref().map(clean_label).transpose()?;
        check_env(&request.env_set)?;
        for name in &request.env_remove {
            check_variable_name(name)?;
            if request.env_set.get(name).is_some() {
                return Err(HubError::invalid(format!(
                    "The variable {name} is both set and removed. Choose one."
                )));
            }
        }
        if label.is_none() && request.env_set.is_empty() && request.env_remove.is_empty() {
            return Err(HubError::invalid(
                "The update changes nothing. Give a new name or a variable to set or remove.",
            ));
        }
        tracing::info!(
            worker_id = %request.worker_id,
            account_id = %request.account_id,
            set = ?request.env_set.names(),
            removed = ?request.env_remove,
            "the hub asks a worker to update an account"
        );
        let secrets = secret_values(&request.env_set);
        let command = HubToSessionHost::AccountUpdate(AccountUpdateCommand {
            account_id: request.account_id,
            label,
            env_set: request.env_set,
            env_remove: request.env_remove,
        });
        self.relay(request.worker_id, command, Relay::Account, &secrets)
            .await?;
        Ok(Empty {})
    }

    pub(crate) async fn account_remove(
        &self,
        _caller: &Caller,
        request: AccountRemove,
    ) -> Result<Empty> {
        let command = HubToSessionHost::AccountRemove(AccountRemoveCommand {
            account_id: request.account_id,
        });
        self.relay(request.worker_id, command, Relay::Account, &[])
            .await?;
        Ok(Empty {})
    }

    pub(crate) async fn account_sign_out(
        &self,
        _caller: &Caller,
        request: AccountSignOut,
    ) -> Result<Empty> {
        let command = HubToSessionHost::AccountSignOut(AccountSignOutCommand {
            account_id: request.account_id,
        });
        self.relay(request.worker_id, command, Relay::Account, &[])
            .await?;
        Ok(Empty {})
    }

    pub(crate) async fn worker_tools_check(
        &self,
        _caller: &Caller,
        request: WorkerToolsCheck,
    ) -> Result<Empty> {
        self.relay(
            request.worker_id,
            HubToSessionHost::ToolsCheck(ToolsCheck {}),
            Relay::Tool,
            &[],
        )
        .await?;
        Ok(Empty {})
    }

    pub(crate) async fn worker_tool_install(
        &self,
        _caller: &Caller,
        request: WorkerToolInstall,
    ) -> Result<Empty> {
        if !request.tool.is_known() {
            return Err(HubError::invalid(format!(
                "The tool {} is not known. Use node, claude-code, claude-agent-acp, codex, opencode, gemini-cli or acp:<registry id>.",
                request.tool
            )));
        }
        let command = HubToSessionHost::ToolsInstall(ToolsInstall { tool: request.tool });
        self.relay(request.worker_id, command, Relay::Tool, &[])
            .await?;
        Ok(Empty {})
    }

    // Sign-ins ---------------------------------------------------------------

    pub(crate) async fn sign_in_start(
        &self,
        _caller: &Caller,
        request: SignInStart,
    ) -> Result<SignInResult> {
        // Check the worker before the hub stores a record that could never
        // run.
        self.require_online_worker(request.worker_id).await?;
        let sign_in = {
            let (worker_id, account_id, method) =
                (request.worker_id, request.account_id, request.method);
            self.store
                .write(move |tx| {
                    let now = tx.now();
                    let sign_in = SignIn {
                        id: SignInId::new(),
                        worker_id,
                        account_id,
                        method,
                        state: SignInState::Starting,
                        created_at: now,
                        updated_at: now,
                        expires_at: now.plus_millis(SIGN_IN_IDLE_EXPIRY_MS),
                    };
                    tx.save_sign_in(&sign_in)?;
                    Ok(sign_in)
                })
                .await?
        };
        let command = HubToSessionHost::SignInStart(SignInStartCommand {
            sign_in_id: sign_in.id,
            account_id: sign_in.account_id,
            method: sign_in.method.clone(),
        });
        if let Err(error) = self
            .relay(sign_in.worker_id, command, Relay::SignIn, &[])
            .await
        {
            // The worker did not start the flow, so no update will come.
            self.end_sign_in(
                sign_in.id,
                SignInState::Failed {
                    message: error.to_error_body().message,
                },
                false,
            )
            .await?;
            return Err(error);
        }
        // The worker can have sent an update already. Return what is stored.
        let id = sign_in.id;
        let sign_in = self
            .store
            .read(move |conn| sign_ins::sign_in_or_404(conn, id))
            .await?;
        Ok(SignInResult { sign_in })
    }

    pub(crate) async fn sign_in_submit_code(
        &self,
        _caller: &Caller,
        request: SignInSubmitCode,
    ) -> Result<Empty> {
        let code = request.code.trim().to_owned();
        if code.is_empty() || code.len() > CODE_MAX_BYTES {
            return Err(HubError::invalid("The code is empty or too long."));
        }
        let id = request.sign_in_id;
        let sign_in = self
            .store
            .read(move |conn| sign_ins::sign_in_or_404(conn, id))
            .await?;
        if sign_in.state.is_final() {
            return Err(HubError::conflict(
                "This sign-in has ended. Start the sign-in again.",
            ));
        }
        if !matches!(sign_in.state, SignInState::NeedsCode { .. }) {
            return Err(HubError::conflict(
                "This sign-in does not wait for a code now. Open the sign-in page first.",
            ));
        }
        // The code goes to the worker and nowhere else.
        let secrets = vec![code.clone()];
        let command = HubToSessionHost::SignInCode(SignInCodeCommand {
            sign_in_id: sign_in.id,
            code,
        });
        self.relay(sign_in.worker_id, command, Relay::SignIn, &secrets)
            .await?;
        Ok(Empty {})
    }

    pub(crate) async fn sign_in_cancel(
        &self,
        _caller: &Caller,
        request: SignInCancel,
    ) -> Result<Empty> {
        let id = request.sign_in_id;
        let sign_in = self
            .store
            .read(move |conn| sign_ins::sign_in_or_404(conn, id))
            .await?;
        if sign_in.state.is_final() {
            return Ok(Empty {});
        }
        // Best effort. A worker that is gone has no flow left to stop, and
        // the user cancels in any case.
        let command = HubToSessionHost::SignInCancel(SignInCancelCommand {
            sign_in_id: sign_in.id,
        });
        if let Err(error) = self
            .relay(sign_in.worker_id, command, Relay::SignIn, &[])
            .await
        {
            tracing::info!(sign_in_id = %sign_in.id, error = %error, "a worker did not confirm the cancel of a sign-in");
        }
        self.end_sign_in(sign_in.id, SignInState::Cancelled, true)
            .await?;
        Ok(Empty {})
    }

    /// Sets a final state on a sign-in that has not ended and adds a feed
    /// upsert. `overwrite_open` is true when the caller ends the sign-in
    /// whatever its state; false changes only a sign-in that is still
    /// `starting`.
    async fn end_sign_in(
        &self,
        id: SignInId,
        state: SignInState,
        overwrite_open: bool,
    ) -> Result<()> {
        self.store
            .write(move |tx| {
                let Some(mut sign_in) = sign_ins::sign_in(tx.db(), id)? else {
                    return Ok(());
                };
                let changes = if overwrite_open {
                    !sign_in.state.is_final()
                } else {
                    sign_in.state == SignInState::Starting
                };
                if changes {
                    sign_in.state = state;
                    sign_in.updated_at = tx.now();
                    tx.save_sign_in(&sign_in)?;
                }
                Ok(())
            })
            .await
    }

    /// Applies a `sign_in.update` event of a worker. An update for an
    /// unknown sign-in, for a sign-in of another worker, or for a sign-in
    /// that has ended changes nothing.
    pub(crate) async fn sign_in_update(
        &self,
        worker_id: WorkerId,
        update: SignInUpdate,
    ) -> Result<()> {
        self.store
            .write(move |tx| {
                let Some(mut sign_in) = sign_ins::sign_in(tx.db(), update.sign_in_id)? else {
                    tracing::debug!(sign_in_id = %update.sign_in_id, "a worker sent an update for a sign-in that the hub does not know");
                    return Ok(());
                };
                if sign_in.worker_id != worker_id {
                    tracing::warn!(sign_in_id = %update.sign_in_id, worker_id = %worker_id, "a worker sent an update for the sign-in of another worker");
                    return Ok(());
                }
                if sign_in.state.is_final() {
                    return Ok(());
                }
                let now = tx.now();
                sign_in.state = update.state;
                sign_in.updated_at = now;
                sign_in.expires_at = now.plus_millis(SIGN_IN_IDLE_EXPIRY_MS);
                tx.save_sign_in(&sign_in)
            })
            .await
    }

    /// Ends the sign-ins that got no update for 15 minutes, and deletes the
    /// ones that got their last update more than a day ago. The maintenance
    /// loop calls it.
    pub(crate) async fn expire_sign_ins(&self) -> Result<()> {
        let (expired, deleted) = self
            .store
            .write(|tx| Ok((tx.expire_sign_ins()?.len(), tx.delete_old_sign_ins()?)))
            .await?;
        if expired > 0 {
            tracing::info!(expired, "sign-ins expired without an update");
        }
        if deleted > 0 {
            tracing::debug!(deleted, "removed old sign-ins");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use consort_proto::client_api::EntityPayload;
    use consort_proto::model::SignInMethod;
    use consort_proto::time::Timestamp;

    use super::*;
    use crate::hub::test_hub;

    fn secrets(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }

    #[test]
    fn hidden_secrets_leave_short_words_alone() {
        let text = "The key sk-abc-123 is bad, not abc.";
        assert_eq!(
            hide_secrets(text, &secrets(&["sk-abc-123", "abc"])),
            "The key <redacted> is bad, not abc."
        );
    }

    #[test]
    fn worker_errors_map_to_the_code_of_the_request() {
        let codes = |relay| {
            [
                ErrorCode::Unauthorized,
                ErrorCode::Internal,
                ErrorCode::Unknown,
                ErrorCode::Invalid,
                ErrorCode::NotFound,
            ]
            .map(|code| map_worker_error(ErrorBody::new(code, "m"), relay).code)
        };
        assert_eq!(
            codes(Relay::Account),
            [
                ErrorCode::Internal,
                ErrorCode::Internal,
                ErrorCode::Internal,
                ErrorCode::Invalid,
                ErrorCode::NotFound
            ]
        );
        assert_eq!(codes(Relay::SignIn)[..3], [ErrorCode::SignInFailed; 3]);
        assert_eq!(codes(Relay::Tool)[..3], [ErrorCode::ToolInstallFailed; 3]);
        // The message of the worker stays.
        let mapped = map_worker_error(
            ErrorBody::new(ErrorCode::Internal, "The disk is full."),
            Relay::Tool,
        );
        assert_eq!(mapped.message, "The disk is full.");
    }

    fn record(age_ms: i64, state: SignInState, hub_worker: WorkerId) -> SignIn {
        let at = Timestamp::now().plus_millis(-age_ms);
        SignIn {
            id: SignInId::new(),
            worker_id: hub_worker,
            account_id: AccountId::new(),
            method: SignInMethod::ClaudePlan,
            state,
            created_at: at,
            updated_at: at,
            expires_at: at.plus_millis(SIGN_IN_IDLE_EXPIRY_MS),
        }
    }

    #[tokio::test]
    async fn the_maintenance_check_expires_idle_sign_ins_and_deletes_old_ones() {
        let (hub, _dir) = test_hub().await;
        let worker = WorkerId::new();
        let idle = record(
            SIGN_IN_IDLE_EXPIRY_MS + 1_000,
            SignInState::Verifying,
            worker,
        );
        let fresh = record(1_000, SignInState::Verifying, worker);
        let old = record(
            consort_proto::model::SIGN_IN_RETENTION_MS + 1_000,
            SignInState::Cancelled,
            worker,
        );
        let ids = (idle.id, fresh.id, old.id);
        hub.store
            .write(move |tx| {
                tx.save_sign_in(&idle)?;
                tx.save_sign_in(&fresh)?;
                tx.save_sign_in(&old)
            })
            .await
            .unwrap();
        let mut feed = hub.store.subscribe();

        hub.expire_sign_ins().await.unwrap();

        let stored = hub.store.read(sign_ins::sign_ins).await.unwrap();
        assert_eq!(stored.len(), 2, "the old row is deleted");
        let state_of = |id: SignInId| stored.iter().find(|s| s.id == id).unwrap().state.clone();
        assert_eq!(state_of(ids.0), SignInState::Expired);
        assert_eq!(state_of(ids.1), SignInState::Verifying);
        // One upsert for the expired sign-in and one delete for the old one.
        let first = feed.recv().await.unwrap();
        assert_eq!(first.entity_id, ids.0.to_string());
        assert!(matches!(
            first.change,
            consort_proto::client_api::FeedChange::Upsert(EntityPayload::SignIn(_))
        ));
        let second = feed.recv().await.unwrap();
        assert_eq!(second.entity_id, ids.2.to_string());
        assert!(matches!(
            second.change,
            consort_proto::client_api::FeedChange::Delete(_)
        ));
    }

    #[tokio::test]
    async fn the_maintenance_loop_runs_the_sign_in_check() {
        // Drive the real loop: a sign-in with no update for 15 minutes
        // expires on a tick.
        let (hub, _dir) = test_hub().await;
        let stale = record(
            SIGN_IN_IDLE_EXPIRY_MS + 1_000,
            SignInState::Starting,
            WorkerId::new(),
        );
        let id = stale.id;
        hub.store
            .write(move |tx| tx.save_sign_in(&stale))
            .await
            .unwrap();
        let task = crate::maintenance::spawn(&hub);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let state = hub
                .store
                .read(move |conn| sign_ins::sign_in_or_404(conn, id))
                .await
                .unwrap()
                .state;
            if state == SignInState::Expired {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "{state:?}");
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        hub.shutdown.cancel();
        task.await.unwrap();
    }
}
