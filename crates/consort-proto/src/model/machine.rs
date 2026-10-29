//! Machine entities: workers, inventories, harness accounts, checkouts and
//! clients.

use serde::{Deserialize, Serialize};

use super::account::SignInMethod;
use super::common::Os;
use super::tool::ToolStatus;
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
    /// User tags, set in a client. Placement matches a tag from this list or
    /// from the inventory. Old stored workers have this as `labels`.
    #[serde(default, alias = "labels")]
    pub tags: Vec<String>,
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
    /// Every tag: the user tags and the inventory tags, sorted, without
    /// repeats.
    pub fn all_tags(&self) -> Vec<&str> {
        let mut tags: Vec<&str> = self.tags.iter().map(String::as_str).collect();
        if let Some(inventory) = &self.inventory {
            tags.extend(inventory.tags.iter().map(String::as_str));
        }
        tags.sort_unstable();
        tags.dedup();
        tags
    }

    /// Whether the worker has every tag in `required`, from the user tags or
    /// the inventory tags.
    pub fn has_tags<S: AsRef<str>>(&self, required: &[S]) -> bool {
        let tags = self.all_tags();
        required.iter().all(|t| tags.contains(&t.as_ref()))
    }
}

/// Whether a harness account is signed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    /// Signed in.
    SignedIn,
    /// Signed out. The user signs in from a client.
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
    /// True for an account that Consort created, with its own profile
    /// folder. False for an account that a user wrote in `worker.toml` by
    /// hand.
    #[serde(default)]
    pub profile: bool,
    /// The signed-in email address, when the harness tells.
    #[serde(default)]
    pub email: Option<String>,
    /// The plan name, such as "Max", when the harness tells.
    #[serde(default)]
    pub plan: Option<String>,
    /// The names of the secret variables in the profile's `env.json`. Never
    /// the values.
    #[serde(default)]
    pub secret_names: Vec<String>,
    /// The sign-in methods that a client may offer for this account.
    #[serde(default)]
    pub sign_in_methods: Vec<SignInMethod>,
    /// Whether the worker starts this account's harness with its
    /// account-synced extras, such as claude.ai connectors and synced
    /// plugins and skills. On by default. An inventory from an older worker
    /// has no field, which reads as on.
    #[serde(default = "default_true")]
    pub extras_enabled: bool,
    /// What the account brings from the provider's side, when the worker
    /// can tell. Never a secret value.
    #[serde(default)]
    pub extras: Option<ProviderExtras>,
}

fn default_true() -> bool {
    true
}

/// The most names in one group of [`ProviderExtras`]. The count can be
/// higher.
pub const PROVIDER_EXTRAS_NAMES_MAX: usize = 20;

/// One kind of provider extra: how many the account has, and some names.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExtrasGroup {
    /// How many there are.
    #[serde(default)]
    pub count: u32,
    /// Short names, at most [`PROVIDER_EXTRAS_NAMES_MAX`].
    #[serde(default)]
    pub names: Vec<String>,
}

/// What a harness account syncs from the provider's side, as the worker
/// reports it in the inventory. For Claude Code: the claude.ai MCP servers
/// (connectors), the synced plugins and the synced skills.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProviderExtras {
    /// MCP servers that the provider adds, such as claude.ai connectors.
    #[serde(default)]
    pub mcp_servers: ExtrasGroup,
    /// Plugins that the provider syncs.
    #[serde(default)]
    pub plugins: ExtrasGroup,
    /// Skills that the provider syncs.
    #[serde(default)]
    pub skills: ExtrasGroup,
    /// Extras that the worker cannot turn off for this provider, in words
    /// for the account dialog, for example "Plugins that you installed by
    /// hand in this profile".
    #[serde(default)]
    pub cannot_turn_off: Vec<String>,
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

/// What a worker has: its tags, harness accounts, managed tools, local MCP
/// servers, checkouts, policy and load. The worker sends a full inventory after `hello` and again
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
    /// Detected tags (such as `macos`, `arm64` or `ram-32gb`) and the tags in
    /// `worker.toml`. Old inventories have this as `labels`.
    #[serde(default, alias = "labels")]
    pub tags: Vec<String>,
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
    /// The managed tools.
    #[serde(default)]
    pub tools: Vec<ToolStatus>,
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
    use crate::model::{ToolId, ToolState};

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
            profile: true,
            email: Some("chris@example.com".into()),
            plan: Some("Max".into()),
            secret_names: vec!["ANTHROPIC_API_KEY".into()],
            sign_in_methods: vec![
                SignInMethod::ClaudePlan,
                SignInMethod::ApiKey {
                    variable: "ANTHROPIC_API_KEY".into(),
                },
            ],
            extras_enabled: true,
            extras: None,
        }
    }

    fn inventory() -> Inventory {
        Inventory {
            hash: String::new(),
            hostname: "bigboi".into(),
            os: Os::Windows,
            arch: "x86_64".into(),
            tags: vec!["gpu".into()],
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
            tools: vec![ToolStatus {
                tool: ToolId::claude_code(),
                version: Some("2.1.0".into()),
                latest: None,
                state: ToolState::Ready,
                detail: None,
            }],
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

    fn worker(tags: &[&str], inventory_tags: Option<&[&str]>) -> Worker {
        Worker {
            id: WorkerId::new(),
            name: "bigboi".into(),
            hostname: "bigboi".into(),
            os: Os::Windows,
            arch: "x86_64".into(),
            public_key: PublicKey::from_bytes([7; 32]),
            version: "0.1.0".into(),
            protocol_version: 1,
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            paused: false,
            state: WorkerState::Online,
            last_seen_at: None,
            inventory: inventory_tags.map(|t| Inventory {
                tags: t.iter().map(|t| (*t).to_owned()).collect(),
                ..inventory()
            }),
            paired_at: Timestamp::from_millis(1),
            revoked_at: None,
        }
    }

    #[test]
    fn all_tags_merges_user_and_inventory_tags() {
        let w = worker(&["work", "gpu"], Some(&["gpu", "windows", "x64"]));
        assert_eq!(w.all_tags(), ["gpu", "windows", "work", "x64"]);
        assert!(w.has_tags(&["work", "x64"]));
        assert!(!w.has_tags(&["work", "linux"]));
        assert!(w.has_tags::<&str>(&[]));
        let bare = worker(&["work"], None);
        assert_eq!(bare.all_tags(), ["work"]);
    }

    /// Stored JSON from before the rename still loads: `labels` reads as
    /// `tags`, and an inventory without the new fields reads with defaults.
    #[test]
    fn old_labels_json_loads_as_tags() {
        let mut value = serde_json::to_value(worker(&["work"], Some(&["gpu"]))).unwrap();
        let obj = value.as_object_mut().unwrap();
        let tags = obj.remove("tags").unwrap();
        obj.insert("labels".into(), tags);
        let inv = obj.get_mut("inventory").unwrap().as_object_mut().unwrap();
        let inv_tags = inv.remove("tags").unwrap();
        inv.insert("labels".into(), inv_tags);
        inv.remove("tools");
        for account in inv.get_mut("accounts").unwrap().as_array_mut().unwrap() {
            let account = account.as_object_mut().unwrap();
            for key in [
                "profile",
                "email",
                "plan",
                "secret_names",
                "sign_in_methods",
                "extras_enabled",
                "extras",
            ] {
                account.remove(key);
            }
        }
        let old: Worker = serde_json::from_value(value).unwrap();
        assert_eq!(old.tags, ["work"]);
        let inv = old.inventory.unwrap();
        assert_eq!(inv.tags, ["gpu"]);
        assert!(inv.tools.is_empty());
        let account = &inv.accounts[0];
        assert!(!account.profile);
        assert_eq!(account.email, None);
        assert!(account.sign_in_methods.is_empty());
        // An older worker reports no extras toggle, which reads as on.
        assert!(account.extras_enabled);
        assert_eq!(account.extras, None);
        // The new name is what goes out.
        let out = serde_json::to_value(worker(&["work"], None)).unwrap();
        assert!(out.get("labels").is_none());
        assert_eq!(out["tags"], serde_json::json!(["work"]));
    }

    #[test]
    fn new_account_fields_round_trip() {
        let a = account("Claude Personal", AuthState::SignedIn);
        let value = serde_json::to_value(&a).unwrap();
        assert_eq!(value["profile"], true);
        assert_eq!(value["email"], "chris@example.com");
        assert_eq!(value["plan"], "Max");
        assert_eq!(
            value["secret_names"],
            serde_json::json!(["ANTHROPIC_API_KEY"])
        );
        assert_eq!(
            value["sign_in_methods"],
            serde_json::json!([{"kind": "claude_plan"}, {"kind": "api_key", "variable": "ANTHROPIC_API_KEY"}])
        );
        assert_eq!(serde_json::from_value::<HarnessAccount>(value).unwrap(), a);
        let inv = inventory();
        let value = serde_json::to_value(&inv).unwrap();
        assert_eq!(value["tools"][0]["tool"], "claude-code");
        assert_eq!(serde_json::from_value::<Inventory>(value).unwrap(), inv);
    }

    #[test]
    fn provider_extras_round_trip() {
        let mut a = account("Claude Personal", AuthState::SignedIn);
        a.extras_enabled = false;
        a.extras = Some(ProviderExtras {
            mcp_servers: ExtrasGroup {
                count: 3,
                names: vec!["Gmail".into(), "Google Drive".into(), "Linear".into()],
            },
            plugins: ExtrasGroup::default(),
            skills: ExtrasGroup {
                count: 25,
                names: vec!["pdf".into()],
            },
            cannot_turn_off: vec!["Plugins that you installed by hand in this profile".into()],
        });
        let value = serde_json::to_value(&a).unwrap();
        assert_eq!(value["extras_enabled"], false);
        assert_eq!(value["extras"]["mcp_servers"]["count"], 3);
        assert_eq!(serde_json::from_value::<HarnessAccount>(value).unwrap(), a);
        let empty: ProviderExtras = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, ProviderExtras::default());
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
