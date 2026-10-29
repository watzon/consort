//! Accounts and sign-in: secret variables, sign-in methods, the sign-in
//! entity, and ACP agent specs.
//!
//! A harness account (in the UI: an account of a provider) lives on one
//! worker, with its own profile folder. The user signs in from a client
//! through the provider's own sign-in flow. See ADR 0010.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{AccountId, SignInId, WorkerId};
use crate::time::Timestamp;

/// A sign-in expires after this many milliseconds without an update (15
/// minutes).
pub const SIGN_IN_IDLE_EXPIRY_MS: i64 = 15 * 60 * 1_000;
/// The hub deletes a sign-in this many milliseconds after its last update
/// (one day).
pub const SIGN_IN_RETENTION_MS: i64 = 24 * 60 * 60 * 1_000;

/// Secret environment variables for one account, such as an API key. On the
/// wire it is a JSON object of names to values.
///
/// The values travel once from a client through the hub to the worker. The
/// worker writes them to `env.json` in the profile folder. The hub never
/// stores them and never logs them. `Debug` shows the names only.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretEnv(BTreeMap<String, String>);

impl SecretEnv {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a variable.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.0.insert(name.into(), value.into());
    }

    /// The value of a variable.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Removes a variable and returns its value.
    pub fn remove(&mut self, name: &str) -> Option<String> {
        self.0.remove(name)
    }

    /// The variable names, sorted. Safe to show and to log.
    pub fn names(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }

    /// The names and values, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Whether there are no variables.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The number of variables.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The map of names to values.
    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    /// Returns the map of names to values.
    pub fn into_map(self) -> BTreeMap<String, String> {
        self.0
    }
}

impl fmt::Debug for SecretEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.0.keys().map(|k| (k, Redacted)))
            .finish()
    }
}

impl From<BTreeMap<String, String>> for SecretEnv {
    fn from(map: BTreeMap<String, String>) -> Self {
        Self(map)
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for SecretEnv {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

/// Shows `<redacted>` in `Debug` output in place of a secret value.
pub(crate) struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A way to sign in to an account, which a client may offer. Serialized
/// with a `kind` field.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignInMethod {
    /// Claude Code with a Claude subscription: `claude auth login
    /// --claudeai`. The user opens a URL and pastes a code.
    ClaudePlan,
    /// Claude Code with a Claude Console account: `claude auth login
    /// --console`. The user opens a URL and pastes a code.
    ClaudeConsole,
    /// Codex with ChatGPT, by device code.
    ChatgptDevice,
    /// Codex with ChatGPT, in a browser on the worker machine.
    ChatgptBrowser,
    /// An API key in a secret variable. The key arrives in the `env` of
    /// `account.create` or the `env_set` of `account.update`, so there is no
    /// flow.
    ApiKey {
        /// The variable name, for example `ANTHROPIC_API_KEY`.
        variable: String,
    },
    /// An ACP agent's own auth method, run with `authenticate`.
    AcpAgent {
        /// The ACP method ID.
        method_id: String,
        /// The method name to show.
        name: String,
    },
    /// An ACP terminal auth method that the app cannot run. The app shows
    /// the command, and the user runs it on the worker.
    Terminal {
        /// The command to run.
        command: String,
    },
}

/// Where a sign-in is. Serialized with a `kind` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignInState {
    /// The worker is starting the flow.
    Starting,
    /// Open this URL to sign in. The flow goes on by itself.
    OpenUrl {
        /// The sign-in page.
        url: String,
    },
    /// Open this URL and enter the code there.
    DeviceCode {
        /// The sign-in page.
        url: String,
        /// The code to enter on the page.
        user_code: String,
    },
    /// Open this URL, then paste the code that the page shows into the app
    /// (`sign_in.submit_code`). Claude uses this.
    NeedsCode {
        /// The sign-in page.
        url: String,
    },
    /// The worker is checking the result.
    Verifying,
    /// Signed in.
    Succeeded {
        /// Who is signed in, for example an email address, when the
        /// harness tells.
        #[serde(default)]
        detail: Option<String>,
    },
    /// The flow failed.
    Failed {
        /// What went wrong and what to do next.
        message: String,
    },
    /// The user cancelled the flow.
    Cancelled,
    /// No update came for [`SIGN_IN_IDLE_EXPIRY_MS`].
    Expired,
}

impl SignInState {
    /// Whether the flow has ended: succeeded, failed, cancelled or expired.
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            Self::Succeeded { .. } | Self::Failed { .. } | Self::Cancelled | Self::Expired
        )
    }
}

/// One run of a provider's sign-in flow for one account, started from a
/// client. A feed entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignIn {
    /// The ID.
    pub id: SignInId,
    /// The worker of the account.
    pub worker_id: WorkerId,
    /// The account.
    pub account_id: AccountId,
    /// The method.
    pub method: SignInMethod,
    /// Where the flow is.
    pub state: SignInState,
    /// When the sign-in started.
    pub created_at: Timestamp,
    /// When the state last changed.
    pub updated_at: Timestamp,
    /// When the sign-in expires if no update comes.
    pub expires_at: Timestamp,
}

/// The ACP agent of an `acp:` account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcpAccountSpec {
    /// An agent from the ACP registry. The worker installs it as a managed
    /// tool.
    Registry {
        /// The registry ID.
        id: String,
    },
    /// A command that the user gives. The worker runs it as is.
    Command {
        /// The program.
        program: String,
        /// The arguments.
        #[serde(default)]
        args: Vec<String>,
    },
}

/// An agent in the ACP registry, for the Add account dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpRegistryAgent {
    /// The registry ID.
    pub id: String,
    /// The name.
    pub name: String,
    /// What it is.
    #[serde(default)]
    pub description: String,
    /// The newest version.
    pub version: String,
    /// The icon.
    #[serde(default)]
    pub icon_url: Option<String>,
    /// True when a distribution exists for at least one platform that the
    /// worker can install: `binary` with a SHA-256, or `npx`.
    pub supported: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_env_is_a_plain_object_and_debug_hides_values() {
        let mut env = SecretEnv::new();
        env.insert("ANTHROPIC_API_KEY", "sk-ant-secret");
        env.insert("OTHER", "hunter2");
        let value = serde_json::to_value(&env).unwrap();
        assert_eq!(
            value,
            json!({"ANTHROPIC_API_KEY": "sk-ant-secret", "OTHER": "hunter2"})
        );
        assert_eq!(serde_json::from_value::<SecretEnv>(value).unwrap(), env);
        let debug = format!("{env:?}");
        assert!(!debug.contains("sk-ant-secret"), "{debug}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(debug.contains("ANTHROPIC_API_KEY"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
        assert_eq!(env.names(), ["ANTHROPIC_API_KEY", "OTHER"]);
        assert_eq!(env.get("OTHER"), Some("hunter2"));
        assert_eq!(env.len(), 2);
        let collected: SecretEnv = [("A", "1")].into_iter().collect();
        assert_eq!(collected.iter().collect::<Vec<_>>(), [("A", "1")]);
    }

    #[test]
    fn sign_in_methods_round_trip() {
        let cases = [
            (SignInMethod::ClaudePlan, json!({"kind": "claude_plan"})),
            (
                SignInMethod::ClaudeConsole,
                json!({"kind": "claude_console"}),
            ),
            (
                SignInMethod::ChatgptDevice,
                json!({"kind": "chatgpt_device"}),
            ),
            (
                SignInMethod::ChatgptBrowser,
                json!({"kind": "chatgpt_browser"}),
            ),
            (
                SignInMethod::ApiKey {
                    variable: "OPENAI_API_KEY".into(),
                },
                json!({"kind": "api_key", "variable": "OPENAI_API_KEY"}),
            ),
            (
                SignInMethod::AcpAgent {
                    method_id: "oauth".into(),
                    name: "Sign in with Google".into(),
                },
                json!({"kind": "acp_agent", "method_id": "oauth", "name": "Sign in with Google"}),
            ),
            (
                SignInMethod::Terminal {
                    command: "goose configure".into(),
                },
                json!({"kind": "terminal", "command": "goose configure"}),
            ),
        ];
        for (method, wire) in cases {
            assert_eq!(serde_json::to_value(&method).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<SignInMethod>(wire).unwrap(),
                method
            );
        }
    }

    #[test]
    fn sign_in_states_round_trip() {
        let cases = [
            (SignInState::Starting, json!({"kind": "starting"}), false),
            (
                SignInState::OpenUrl {
                    url: "https://a".into(),
                },
                json!({"kind": "open_url", "url": "https://a"}),
                false,
            ),
            (
                SignInState::DeviceCode {
                    url: "https://a".into(),
                    user_code: "ABCD-1234".into(),
                },
                json!({"kind": "device_code", "url": "https://a", "user_code": "ABCD-1234"}),
                false,
            ),
            (
                SignInState::NeedsCode {
                    url: "https://a".into(),
                },
                json!({"kind": "needs_code", "url": "https://a"}),
                false,
            ),
            (SignInState::Verifying, json!({"kind": "verifying"}), false),
            (
                SignInState::Succeeded {
                    detail: Some("chris@example.com".into()),
                },
                json!({"kind": "succeeded", "detail": "chris@example.com"}),
                true,
            ),
            (
                SignInState::Failed {
                    message: "The code is wrong. Try again.".into(),
                },
                json!({"kind": "failed", "message": "The code is wrong. Try again."}),
                true,
            ),
            (SignInState::Cancelled, json!({"kind": "cancelled"}), true),
            (SignInState::Expired, json!({"kind": "expired"}), true),
        ];
        for (state, wire, is_final) in cases {
            assert_eq!(serde_json::to_value(&state).unwrap(), wire);
            assert_eq!(serde_json::from_value::<SignInState>(wire).unwrap(), state);
            assert_eq!(state.is_final(), is_final, "{state:?}");
        }
        let bare: SignInState = serde_json::from_value(json!({"kind": "succeeded"})).unwrap();
        assert_eq!(bare, SignInState::Succeeded { detail: None });
    }

    #[test]
    fn sign_in_round_trips() {
        let sign_in = SignIn {
            id: "sgn_00000000-0000-0000-0000-000000000001".parse().unwrap(),
            worker_id: "wrk_00000000-0000-0000-0000-000000000002".parse().unwrap(),
            account_id: "acc_00000000-0000-0000-0000-000000000003".parse().unwrap(),
            method: SignInMethod::ClaudePlan,
            state: SignInState::NeedsCode {
                url: "https://claude.ai/oauth".into(),
            },
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(2),
            expires_at: Timestamp::from_millis(2 + SIGN_IN_IDLE_EXPIRY_MS),
        };
        let value = serde_json::to_value(&sign_in).unwrap();
        assert_eq!(value["id"], "sgn_00000000-0000-0000-0000-000000000001");
        assert_eq!(value["method"], json!({"kind": "claude_plan"}));
        assert_eq!(value["state"]["kind"], "needs_code");
        assert_eq!(serde_json::from_value::<SignIn>(value).unwrap(), sign_in);
    }

    #[test]
    fn acp_specs_and_registry_agents_round_trip() {
        let registry = AcpAccountSpec::Registry { id: "goose".into() };
        assert_eq!(
            serde_json::to_value(&registry).unwrap(),
            json!({"kind": "registry", "id": "goose"})
        );
        let command = AcpAccountSpec::Command {
            program: "my-agent".into(),
            args: vec!["--acp".into()],
        };
        let wire = json!({"kind": "command", "program": "my-agent", "args": ["--acp"]});
        assert_eq!(serde_json::to_value(&command).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<AcpAccountSpec>(wire).unwrap(),
            command
        );
        let no_args: AcpAccountSpec =
            serde_json::from_value(json!({"kind": "command", "program": "x"})).unwrap();
        assert_eq!(
            no_args,
            AcpAccountSpec::Command {
                program: "x".into(),
                args: vec![]
            }
        );

        let agent = AcpRegistryAgent {
            id: "goose".into(),
            name: "Goose".into(),
            description: "An agent.".into(),
            version: "1.2.0".into(),
            icon_url: Some("https://cdn.example/goose.svg".into()),
            supported: true,
        };
        let value = serde_json::to_value(&agent).unwrap();
        assert_eq!(value["icon_url"], "https://cdn.example/goose.svg");
        assert_eq!(
            serde_json::from_value::<AcpRegistryAgent>(value).unwrap(),
            agent
        );
    }
}
