//! Machine entities: workers, inventories, harness accounts, checkouts and
//! clients.

use serde::{Deserialize, Serialize};

use super::common::Os;
use super::work::ApprovalMode;
use crate::auth::PublicKey;
use crate::ids::{AccountId, CheckoutId, ClientId, RepoId, WorkerId};
use crate::time::Timestamp;

/// Whether a worker is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    /// The worker link is open.
    Online,
    /// The worker link is closed, or no frame arrived for 60 seconds.
    Offline,
}

/// A machine that runs sessions for the hub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Worker {
    /// The ID.
    pub id: WorkerId,
    /// The name that the user set.
    pub name: String,
    /// The hostname.
    pub hostname: String,
    /// The operating system.
    pub os: Os,
    /// The CPU architecture, for example `aarch64`.
    pub arch: String,
    /// The worker's Ed25519 public key.
    pub public_key: PublicKey,
    /// The worker's software version.
    pub version: String,
    /// The protocol version agreed in `hello`.
    pub protocol_version: u32,
    /// Labels that the user set in a client. Placement matches a label from
    /// this list or from the inventory.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Whether the user paused the worker. Placement skips a paused worker.
    #[serde(default)]
    pub paused: bool,
    /// Online or offline.
    pub state: WorkerState,
    /// When the hub last got a frame from the worker.
    pub last_seen_at: Option<Timestamp>,
    /// The newest inventory.
    pub inventory: Option<Inventory>,
    /// When the worker paired.
    pub paired_at: Timestamp,
    /// When the user revoked the worker.
    pub revoked_at: Option<Timestamp>,
}

impl Worker {
    /// Every label: the user's labels and the inventory labels, without
    /// repeats.
    pub fn all_labels(&self) -> Vec<&str> {
        let mut labels: Vec<&str> = self.labels.iter().map(String::as_str).collect();
        if let Some(inventory) = &self.inventory {
            labels.extend(inventory.labels.iter().map(String::as_str));
        }
        labels.sort_unstable();
        labels.dedup();
        labels
    }
}

/// Whether a harness account is signed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    /// Signed in.
    SignedIn,
    /// Signed out. The user signs in on the worker machine.
    SignedOut,
    /// The harness reported an error.
    Error,
    /// The worker could not tell.
    Unknown,
}

/// One choice of a select option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionChoice {
    /// The value to send.
    pub value: String,
    /// The text to show.
    pub label: String,
}

/// One option that a model accepts. Serialized with a `kind` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OptionDescriptor {
    /// One value from a list, for example effort.
    Select {
        /// The option ID, the key in model options.
        id: String,
        /// The text to show.
        label: String,
        /// The choices.
        choices: Vec<OptionChoice>,
        /// The default value.
        default: Option<String>,
    },
    /// On or off.
    Boolean {
        /// The option ID.
        id: String,
        /// The text to show.
        label: String,
        /// The default value.
        #[serde(default)]
        default: bool,
    },
}

impl OptionDescriptor {
    /// The option ID.
    pub fn id(&self) -> &str {
        match self {
            Self::Select { id, .. } | Self::Boolean { id, .. } => id,
        }
    }
}

/// A model that a harness account offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// The model ID, as the harness names it.
    pub id: String,
    /// The name to show.
    pub name: String,
    /// The options.
    #[serde(default)]
    pub options: Vec<OptionDescriptor>,
}

/// What a harness session can do. An adapter reports a feature only if it
/// has a test for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    /// The session accepts new input while a turn runs.
    pub steer: bool,
    /// The session can continue after the harness process restarts.
    pub resume: bool,
    /// The session can branch from an earlier turn.
    pub fork: bool,
    /// Turn input can hold images.
    pub images: bool,
    /// The harness has a plan-only mode.
    pub plan_mode: bool,
    /// The harness reports token counts.
    pub usage: bool,
    /// The harness accepts an HTTP MCP server with a bearer token.
    pub mcp_http: bool,
    /// The approval modes that the adapter can enforce.
    pub approval_modes: Vec<ApprovalMode>,
}

impl Capabilities {
    /// The most permissive mode, at most `requested`, that the adapter
    /// supports. `None` if it supports none of them, for example Ask on Pi.
    pub fn best_mode_up_to(&self, requested: ApprovalMode) -> Option<ApprovalMode> {
        self.approval_modes
            .iter()
            .copied()
            .filter(|m| *m <= requested)
            .max()
    }
}

/// One harness account on a worker, as the worker reports it. Never holds a
/// credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessAccount {
    /// The ID, unique per worker.
    pub account_id: AccountId,
    /// `claude`, `codex`, `pi`, `opencode` or `acp:<agent-id>`.
    pub harness: String,
    /// The label, for example "Claude Personal". Two workers with the same
    /// label share one login.
    pub label: String,
    /// The installed harness version.
    pub harness_version: Option<String>,
    /// Whether the account is signed in.
    pub auth_state: AuthState,
    /// More about the sign-in state, such as the plan name or the command to
    /// sign in.
    pub auth_detail: Option<String>,
    /// The models.
    #[serde(default)]
    pub models: Vec<ModelInfo>,
    /// What sessions of this account can do.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Whether the user enabled the account in `worker.toml`.
    pub enabled: bool,
}

impl HarnessAccount {
    /// Whether placement can use this account for `model`.
    pub fn can_run(&self, model: &str) -> bool {
        self.enabled
            && self.auth_state == AuthState::SignedIn
            && self.models.iter().any(|m| m.id == model)
    }
}

/// A local MCP server that a worker can reach. Never holds a URL with a
/// secret or a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalMcpServerInfo {
    /// The name, for example `home-assistant`.
    pub name: String,
    /// What it is for.
    pub description: Option<String>,
    /// Its tool names.
    #[serde(default)]
    pub tools: Vec<String>,
}

/// A checkout as the worker reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutInfo {
    /// The git remote URL.
    pub repo_url: String,
    /// The path on the worker.
    pub path: String,
    /// True when the worker made the clone itself.
    #[serde(default)]
    pub managed: bool,
}

/// The limits that a worker sets on what the hub may ask it to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerPolicy {
    /// The highest approval mode that sessions on this worker may use.
    pub max_approval_mode: ApprovalMode,
    /// Folders where sessions may run. Empty means only `workspaces/`.
    #[serde(default)]
    pub allowed_roots: Vec<String>,
    /// Whether sessions may run shell commands.
    pub allow_shell: bool,
}

impl Default for WorkerPolicy {
    fn default() -> Self {
        Self {
            max_approval_mode: ApprovalMode::Ask,
            allowed_roots: Vec::new(),
            allow_shell: true,
        }
    }
}

/// How busy a worker is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerLoad {
    /// Sessions that are not ended or lost.
    pub active_sessions: u32,
}

/// What a worker has: its harness accounts, local MCP servers, checkouts,
/// policy and load. The worker sends a full inventory after `hello` and again
/// when any part changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// The hash of the rest of the inventory. See [`Inventory::compute_hash`].
    #[serde(default)]
    pub hash: String,
    /// The hostname.
    pub hostname: String,
    /// The operating system.
    pub os: Os,
    /// The CPU architecture.
    pub arch: String,
    /// Labels from `worker.toml`.
    #[serde(default)]
    pub labels: Vec<String>,
    /// The harness accounts.
    #[serde(default)]
    pub accounts: Vec<HarnessAccount>,
    /// The local MCP servers.
    #[serde(default)]
    pub local_mcp_servers: Vec<LocalMcpServerInfo>,
    /// The checkouts.
    #[serde(default)]
    pub checkouts: Vec<CheckoutInfo>,
    /// The worker policy.
    pub policy: WorkerPolicy,
    /// The load.
    #[serde(default)]
    pub load: WorkerLoad,
}

/// FNV-1a, 64 bit. A change detector, not a security hash.
struct Fnv1a(u64);

impl std::io::Write for Fnv1a {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for b in buf {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Inventory {
    /// The hash of every field except `hash`: `fnv1a64:` and 16 hex
    /// characters. The worker sets it before it sends the inventory. The hub
    /// can skip a write when the hash did not change.
    pub fn compute_hash(&self) -> String {
        let mut copy = self.clone();
        copy.hash = String::new();
        let mut hasher = Fnv1a(0xcbf2_9ce4_8422_2325);
        // Writing into the hasher cannot fail, and the inventory has only
        // string map keys, so serialization cannot fail.
        let _ = serde_json::to_writer(&mut hasher, &copy);
        format!("fnv1a64:{:016x}", hasher.0)
    }

    /// Returns the inventory with `hash` set.
    pub fn with_hash(mut self) -> Self {
        self.hash = self.compute_hash();
        self
    }

    /// The account with this ID.
    pub fn account(&self, account_id: AccountId) -> Option<&HarnessAccount> {
        self.accounts.iter().find(|a| a.account_id == account_id)
    }

    /// The first account with this label that can run `model`.
    pub fn account_for(&self, label: &str, model: &str) -> Option<&HarnessAccount> {
        self.accounts
            .iter()
            .find(|a| a.label == label && a.can_run(model))
    }
}

/// A copy of a repo on one worker that the hub knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkout {
    /// The ID.
    pub id: CheckoutId,
    /// The repo.
    pub repo_id: RepoId,
    /// The worker.
    pub worker_id: WorkerId,
    /// The path on the worker.
    pub path: String,
    /// True when the worker made the clone itself.
    pub managed: bool,
    /// When the worker last reported it.
    pub last_seen_at: Timestamp,
}

/// The kind of client app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    /// The desktop app.
    Desktop,
    /// The iOS app.
    Ios,
    /// The Android app.
    Android,
}

/// A push notification service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushProvider {
    /// Apple Push Notification service.
    Apns,
    /// Firebase Cloud Messaging.
    Fcm,
    /// Expo push service.
    Expo,
}

/// A client's push registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRegistration {
    /// The service.
    pub provider: PushProvider,
    /// The device token.
    pub token: String,
}

/// An app that the user uses to talk to the assistant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    /// The ID.
    pub id: ClientId,
    /// The name, for example "MacBook Pro".
    pub name: String,
    /// The kind.
    pub kind: ClientKind,
    /// The client's Ed25519 public key.
    pub public_key: PublicKey,
    /// The push service. The hub does not send the token to other clients.
    pub push_provider: Option<PushProvider>,
    /// When the client paired.
    pub paired_at: Timestamp,
    /// When the client last connected.
    pub last_seen_at: Option<Timestamp>,
    /// When the user revoked the client.
    pub revoked_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(label: &str, auth_state: AuthState) -> HarnessAccount {
        HarnessAccount {
            account_id: AccountId::new(),
            harness: "claude".into(),
            label: label.into(),
            harness_version: Some("2.1.0".into()),
            auth_state,
            auth_detail: None,
            models: vec![ModelInfo {
                id: "opus".into(),
                name: "Opus".into(),
                options: vec![OptionDescriptor::Select {
                    id: "effort".into(),
                    label: "Effort".into(),
                    choices: vec![OptionChoice {
                        value: "high".into(),
                        label: "High".into(),
                    }],
                    default: Some("high".into()),
                }],
            }],
            capabilities: Capabilities {
                steer: true,
                approval_modes: vec![ApprovalMode::Auto, ApprovalMode::FullAccess],
                ..Capabilities::default()
            },
            enabled: true,
        }
    }

    fn inventory() -> Inventory {
        Inventory {
            hash: String::new(),
            hostname: "bigboi".into(),
            os: Os::Windows,
            arch: "x86_64".into(),
            labels: vec!["gpu".into()],
            accounts: vec![
                account("Claude Work", AuthState::SignedOut),
                account("Claude Personal", AuthState::SignedIn),
            ],
            local_mcp_servers: vec![],
            checkouts: vec![CheckoutInfo {
                repo_url: "git@github.com:watzon/sayso.git".into(),
                path: "C:/Users/chris/Code/sayso".into(),
                managed: false,
            }],
            policy: WorkerPolicy::default(),
            load: WorkerLoad { active_sessions: 2 },
        }
    }

    #[test]
    fn inventory_hash_ignores_its_own_field_and_tracks_changes() {
        let a = inventory().with_hash();
        assert!(a.hash.starts_with("fnv1a64:"));
        assert_eq!(a.hash.len(), "fnv1a64:".len() + 16);
        assert_eq!(a.compute_hash(), a.hash);
        let mut b = a.clone();
        b.load.active_sessions = 3;
        assert_ne!(b.compute_hash(), a.hash);
        let round: Inventory = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
        assert_eq!(round, a);
    }

    #[test]
    fn account_lookup() {
        let inv = inventory();
        let found = inv.account_for("Claude Personal", "opus").unwrap();
        assert_eq!(found.label, "Claude Personal");
        assert!(inv.account_for("Claude Work", "opus").is_none());
        assert!(inv.account_for("Claude Personal", "haiku").is_none());
        assert_eq!(inv.account(found.account_id), Some(found));
    }

    #[test]
    fn capabilities_mode_choice() {
        let caps = account("x", AuthState::SignedIn).capabilities;
        assert_eq!(caps.best_mode_up_to(ApprovalMode::Ask), None);
        assert_eq!(
            caps.best_mode_up_to(ApprovalMode::FullAccess),
            Some(ApprovalMode::FullAccess)
        );
        assert_eq!(
            caps.best_mode_up_to(ApprovalMode::Auto),
            Some(ApprovalMode::Auto)
        );
        let empty: Capabilities = serde_json::from_str("{}").unwrap();
        assert!(!empty.steer);
    }

    #[test]
    fn option_descriptor_shape() {
        let d = OptionDescriptor::Boolean {
            id: "thinking".into(),
            label: "Thinking".into(),
            default: false,
        };
        assert_eq!(
            serde_json::to_value(&d).unwrap(),
            serde_json::json!({"kind": "boolean", "id": "thinking", "label": "Thinking", "default": false})
        );
        assert_eq!(d.id(), "thinking");
    }
}
