//! Codex sign-in and sign-out through `codex app-server`.
//!
//! The worker starts `codex app-server` with the account's `CODEX_HOME` and
//! calls `account/login/start`. A ChatGPT sign-in gives a device code or a
//! page to open, and ends with an `account/login/completed` notification.
//! An API key is stored with `account/login/start` too. The adapter never
//! reads `auth.json`: `account/read` tells who is signed in.

use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;

use super::protocol::{
    AccountLoginCompleted, CancelLoginAccountParams, GetAccountParams, GetAccountResponse,
    LoginAccountParams, LoginAccountResponse, method,
};
use super::rpc::{Inbound, RpcClient};
use super::{Connection, connect};
use crate::process::HarnessProcess;
use crate::{AccountConfig, HarnessError};

/// What the user does to go on with a ChatGPT sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatgptStep {
    /// Open `url` and enter `user_code` there.
    DeviceCode {
        /// The page.
        url: String,
        /// The code.
        user_code: String,
    },
    /// Open `url` in a browser on the worker machine. Codex listens for the
    /// callback on a fixed local port.
    OpenUrl {
        /// The page.
        url: String,
    },
}

/// A running ChatGPT sign-in.
pub struct ChatgptLogin {
    client: RpcClient,
    inbound: mpsc::UnboundedReceiver<Inbound>,
    process: Option<HarnessProcess>,
    login_id: String,
}

impl std::fmt::Debug for ChatgptLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatgptLogin")
            .field("login_id", &self.login_id)
            .finish_non_exhaustive()
    }
}

/// Starts `codex app-server` for an account in a temporary folder.
async fn open(account: &AccountConfig) -> Result<Connection, HarnessError> {
    let dir = std::env::temp_dir();
    connect(account, &dir, Default::default(), None).await
}

impl ChatgptLogin {
    /// Starts a ChatGPT sign-in for the account: by device code when
    /// `device` is true, else in a browser on the worker machine.
    pub async fn start(
        account: &AccountConfig,
        device: bool,
    ) -> Result<(Self, ChatgptStep), HarnessError> {
        let conn = open(account).await?;
        Self::start_on(conn.client, conn.inbound, Some(conn.process), device).await
    }

    /// [`ChatgptLogin::start`] on a connection that is initialized. Tests
    /// pass a fake app-server and no process.
    pub(crate) async fn start_on(
        client: RpcClient,
        inbound: mpsc::UnboundedReceiver<Inbound>,
        process: Option<HarnessProcess>,
        device: bool,
    ) -> Result<(Self, ChatgptStep), HarnessError> {
        let params = if device {
            LoginAccountParams::ChatgptDeviceCode
        } else {
            LoginAccountParams::Chatgpt
        };
        let mut login = Self {
            client,
            inbound,
            process,
            login_id: String::new(),
        };
        let response: Result<LoginAccountResponse, HarnessError> = login
            .client
            .request(method::ACCOUNT_LOGIN_START, &params)
            .await;
        let step = match response {
            Ok(LoginAccountResponse::ChatgptDeviceCode {
                login_id,
                verification_url,
                user_code,
            }) => {
                login.login_id = login_id;
                ChatgptStep::DeviceCode {
                    url: verification_url,
                    user_code,
                }
            }
            Ok(LoginAccountResponse::Chatgpt { login_id, auth_url }) => {
                login.login_id = login_id;
                ChatgptStep::OpenUrl { url: auth_url }
            }
            Ok(_) => {
                login.close().await;
                return Err(HarnessError::Protocol(
                    "`account/login/start` returned an unexpected kind".to_owned(),
                ));
            }
            Err(err) => {
                login.close().await;
                return Err(err);
            }
        };
        Ok((login, step))
    }

    /// Waits for `account/login/completed` of this sign-in. Fails with the
    /// reason that Codex gives, or when the time runs out.
    pub async fn wait(&mut self, limit: Duration) -> Result<(), HarnessError> {
        let wait = async {
            while let Some(message) = self.inbound.recv().await {
                match message {
                    Inbound::Notification { method, params }
                        if method == method::ACCOUNT_LOGIN_COMPLETED =>
                    {
                        let done: AccountLoginCompleted = serde_json::from_value(params)
                            .map_err(|e| HarnessError::Protocol(e.to_string()))?;
                        if done
                            .login_id
                            .as_deref()
                            .is_some_and(|id| id != self.login_id)
                        {
                            continue;
                        }
                        return if done.success {
                            Ok(())
                        } else {
                            Err(HarnessError::Protocol(done.error.unwrap_or_else(|| {
                                "Codex did not finish the sign-in".to_owned()
                            })))
                        };
                    }
                    Inbound::Request { id, .. } => {
                        // No server request belongs to a sign-in.
                        let _ = self.client.respond_error(&id, -32601, "not supported");
                    }
                    Inbound::Notification { .. } => {}
                }
            }
            Err(HarnessError::Closed)
        };
        tokio::time::timeout(limit, wait)
            .await
            .map_err(|_| HarnessError::Timeout("the sign-in took too long".to_owned()))?
    }

    /// Who is signed in now.
    pub async fn account(&self) -> Result<GetAccountResponse, HarnessError> {
        self.client
            .request(
                method::ACCOUNT_READ,
                &GetAccountParams {
                    refresh_token: false,
                },
            )
            .await
    }

    /// Cancels the sign-in and stops Codex.
    pub async fn cancel(mut self) {
        if !self.login_id.is_empty() {
            let _: Result<Value, _> = tokio::time::timeout(
                Duration::from_secs(5),
                self.client.request(
                    method::ACCOUNT_LOGIN_CANCEL,
                    &CancelLoginAccountParams {
                        login_id: self.login_id.clone(),
                    },
                ),
            )
            .await
            .unwrap_or_else(|_| Err(HarnessError::Timeout("cancel".to_owned())));
        }
        self.close().await;
    }

    /// Stops Codex.
    pub async fn close(&mut self) {
        if let Some(mut process) = self.process.take() {
            process.shutdown(Duration::from_secs(1)).await;
        }
    }
}

/// Reads who is signed in.
pub async fn read_account(account: &AccountConfig) -> Result<GetAccountResponse, HarnessError> {
    let mut conn = open(account).await?;
    let result = conn
        .client
        .request(
            method::ACCOUNT_READ,
            &GetAccountParams {
                refresh_token: false,
            },
        )
        .await;
    conn.process.shutdown(Duration::from_secs(1)).await;
    result
}

/// Stores an API key in the account's Codex home with
/// `account/login/start`. Codex does not read `OPENAI_API_KEY` from its
/// environment for its account (tested with codex-cli 0.165.0).
pub async fn api_key_login(account: &AccountConfig, key: &str) -> Result<(), HarnessError> {
    let mut conn = open(account).await?;
    let result = api_key_login_on(&conn.client, key).await;
    conn.process.shutdown(Duration::from_secs(1)).await;
    result
}

pub(crate) async fn api_key_login_on(client: &RpcClient, key: &str) -> Result<(), HarnessError> {
    let response: LoginAccountResponse = client
        .request(
            method::ACCOUNT_LOGIN_START,
            &LoginAccountParams::ApiKey {
                api_key: key.to_owned(),
            },
        )
        .await?;
    match response {
        LoginAccountResponse::ApiKey => Ok(()),
        _ => Err(HarnessError::Protocol(
            "`account/login/start` returned an unexpected kind".to_owned(),
        )),
    }
}

/// Signs out with `account/logout`.
pub async fn logout(account: &AccountConfig) -> Result<(), HarnessError> {
    let mut conn = open(account).await?;
    let result: Result<Value, HarnessError> = conn
        .client
        .request(method::ACCOUNT_LOGOUT, &Value::Null)
        .await;
    conn.process.shutdown(Duration::from_secs(1)).await;
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// A fake app-server for sign-in: it answers `account/login/start`, then
    /// sends `account/login/completed`, and answers `account/read`.
    async fn run_server(
        stream: tokio::io::DuplexStream,
        log: Arc<Mutex<Vec<Value>>>,
        success: bool,
    ) {
        let (read, mut write) = tokio::io::split(stream);
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let message: Value = serde_json::from_str(&line).unwrap();
            log.lock().unwrap().push(message.clone());
            let id = message["id"].clone();
            let mut out = Vec::new();
            match message["method"].as_str().unwrap_or("") {
                "account/login/start" => match message["params"]["type"].as_str() {
                    Some("chatgptDeviceCode") => {
                        out.push(json!({"id": id, "result": {"type": "chatgptDeviceCode",
                            "loginId": "l1", "verificationUrl": "https://auth.example/device",
                            "userCode": "ABCD-1234"}}));
                        // Another sign-in ends first; this one must wait.
                        out.push(json!({"method": "account/login/completed",
                            "params": {"loginId": "other", "success": false, "error": "no"}}));
                        out.push(json!({"method": "account/login/completed",
                            "params": {"loginId": "l1", "success": success,
                                       "error": (!success).then_some("The code expired.")}}));
                    }
                    Some("chatgpt") => {
                        out.push(json!({"id": id, "result": {"type": "chatgpt",
                            "loginId": "l2", "authUrl": "https://auth.example/authorize"}}));
                    }
                    Some("apiKey") => out.push(json!({"id": id, "result": {"type": "apiKey"}})),
                    _ => out.push(json!({"id": id, "error": {"code": -1, "message": "bad"}})),
                },
                "account/read" => out.push(json!({"id": id, "result": {
                    "account": {"type": "chatgpt", "email": "a@example.com", "planType": "plus"},
                    "requiresOpenaiAuth": true}})),
                "account/login/cancel" => out.push(json!({"id": id, "result": {}})),
                _ => {}
            }
            for message in out {
                let mut text = message.to_string();
                text.push('\n');
                write.write_all(text.as_bytes()).await.unwrap();
            }
        }
    }

    fn fake(
        success: bool,
    ) -> (
        RpcClient,
        mpsc::UnboundedReceiver<Inbound>,
        Arc<Mutex<Vec<Value>>>,
    ) {
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let log = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(run_server(server_side, log.clone(), success));
        let (read, write) = tokio::io::split(client_side);
        let (client, inbound) = RpcClient::start(write, read);
        (client, inbound, log)
    }

    #[tokio::test]
    async fn a_device_code_sign_in_waits_for_its_own_completion() {
        let (client, inbound, log) = fake(true);
        let (mut login, step) = ChatgptLogin::start_on(client, inbound, None, true)
            .await
            .unwrap();
        assert_eq!(
            step,
            ChatgptStep::DeviceCode {
                url: "https://auth.example/device".into(),
                user_code: "ABCD-1234".into()
            }
        );
        login.wait(Duration::from_secs(5)).await.unwrap();
        let account = login.account().await.unwrap();
        assert!(matches!(
            account.account,
            Some(super::super::protocol::Account::Chatgpt { email: Some(e), .. }) if e == "a@example.com"
        ));
        let sent = log.lock().unwrap().clone();
        assert_eq!(sent[0]["params"], json!({"type": "chatgptDeviceCode"}));
    }

    #[tokio::test]
    async fn a_failed_completion_gives_the_reason() {
        let (client, inbound, _log) = fake(false);
        let (mut login, _) = ChatgptLogin::start_on(client, inbound, None, true)
            .await
            .unwrap();
        let err = login.wait(Duration::from_secs(5)).await.unwrap_err();
        assert!(err.to_string().contains("The code expired."), "{err}");
    }

    #[tokio::test]
    async fn a_browser_sign_in_gives_the_url_and_can_be_cancelled() {
        let (client, inbound, log) = fake(true);
        let (login, step) = ChatgptLogin::start_on(client, inbound, None, false)
            .await
            .unwrap();
        assert_eq!(
            step,
            ChatgptStep::OpenUrl {
                url: "https://auth.example/authorize".into()
            }
        );
        login.cancel().await;
        let sent = log.lock().unwrap().clone();
        assert_eq!(sent[0]["params"], json!({"type": "chatgpt"}));
        assert_eq!(sent[1]["method"], "account/login/cancel");
        assert_eq!(sent[1]["params"], json!({"loginId": "l2"}));
    }

    #[tokio::test]
    async fn an_api_key_goes_in_the_login_request_and_not_in_debug_output() {
        let (client, _inbound, log) = fake(true);
        api_key_login_on(&client, "sk-test-secret").await.unwrap();
        let sent = log.lock().unwrap().clone();
        assert_eq!(
            sent[0]["params"],
            json!({"type": "apiKey", "apiKey": "sk-test-secret"})
        );
        let debug = format!(
            "{:?}",
            LoginAccountParams::ApiKey {
                api_key: "sk-test-secret".into()
            }
        );
        assert!(!debug.contains("sk-test-secret"), "{debug}");
    }
}
