# Plan: worker accounts, sign-in, managed tools and tags

Status: approved by the user on 2026-10-29. The Paper designs are on the Desktop page of "Consort — v0.1 Design": "Settings, Workers with tags and tools", "Add account dialog", "Add account dialog, Name step" and "Sign-in dialog, Claude". This file is the contract between the build agents. If you must change a name or a shape here, change this file in the same commit and say why in `docs/DECISIONS.md`.

## What the user asked for

1. Accounts belong to a worker and are managed in Settings > Workers. There is no central Providers page.
2. One provider (harness) can have several accounts on one worker. Each account has its own profile folder, so the logins never mix.
3. Consort never picks up a login that already exists on the machine. The user signs in explicitly from the app, through the harness's own sign-in.
4. Each worker downloads and updates its own copy of each harness CLI. It does not use global installs.
5. Tags: the user creates tags on workers. A channel can have one or more tags, chosen from the same list. All work from a tagged channel (coordinator turns, tasks, routines) runs only on workers that have every tag of the channel.
6. Provider logos are used in the UI, never text alone.
7. OpenCode version 2 is the supported line.

## Terms (update `CONTEXT.md`)

- **Provider**: the UI word for a harness that Consort can install and run: Claude Code, Codex, OpenCode, Gemini CLI, an agent from the ACP registry, or a custom ACP command. Code keeps the word `harness`.
- **Account** (code: harness account): one login to one provider on one worker, with its own profile folder. Accounts with the same name on different workers count as one account for a model selection.
- **Profile folder**: `<consort home>/profiles/<name>/`. The worker chooses the name from the account name when it makes the account: the name in lowercase, each run of characters other than `a` to `z` and `0` to `9` as one `-`, no `-` at the start or the end, at most 48 characters, and `account` when nothing is left. "Claude Work" gives `claude-work`. When that folder exists or another account uses the name, the worker adds `-2`, `-3` and so on. The name is stored in `worker.toml` (`profile_dir`), so a rename of the account does not move the folder. It holds the harness's own config and login, and `env.json` for secret variables. (Changed on 2026-10-29 from `<account id>`, see `docs/DECISIONS.md`.)
- **Consort home**: `~/.consort` (`%USERPROFILE%\.consort` on Windows). `CONSORT_HOME` overrides it. `consort dev` and `scripts/dev.sh` set it to a folder inside the dev data.
- **Managed tool**: a CLI or runtime that the worker installs in `<consort home>/tools/<tool>/<version>/`.
- **Tag**: a short name on a worker, such as `work` or `gpu`. User tags are set in the app. Detected tags come from the worker: `macos`, `windows`, `linux`, `arm64`, `x64`, `ram-<n>gb`, `cpu-<n>`, `gpu-nvidia`, `gpu-amd`, `gpu-apple`. A channel's tags are requirements. Tag text: lowercase letters, digits and `-`, 1 to 32 characters.
- **Sign-in**: one run of a provider's sign-in flow for one account, started from the app.

## Credentials (update ADR 0004, add ADR 0010)

- Subscription logins (Claude plan, Claude Console, ChatGPT) stay in the profile folder on the worker, in the harness's own files or keychain entry. Consort never reads them.
- API keys and other secret variables travel once from the app through the hub to the worker, inside `account.create` or `account.update`. The worker writes them to `env.json` in the profile folder (mode 0600 on Unix). The hub never stores them and never logs them. Inventory reports only variable names, never values.
- The Claude sign-in code that the user pastes travels app to hub to worker, and the worker writes it to the stdin of `claude auth login`. Nobody stores it.

## Proto changes (`consort-proto`)

Rename labels to tags everywhere. Keep `#[serde(alias = "labels")]` on renamed fields so old stored JSON still loads.

- `Worker.labels` becomes `Worker.tags` (user tags, set by the app). `Worker::all_labels` becomes `all_tags` (user tags plus `Inventory.tags`).
- `Inventory.labels` becomes `Inventory.tags`: the detected tags plus any `tags` from `worker.toml`.
- `PlacementRequire.labels` becomes `tags`. `WorkerSummary.labels` becomes `tags`.
- `worker.set_labels` becomes `worker.set_tags` (`WorkerSetTags { worker_id, tags }`). The hub validates the tag text.
- `Channel` gets `tags: Vec<String>` (`#[serde(default)]`). `channel.create` and `channel.update` take `tags`.

New inventory fields:

- `HarnessAccount` gets `profile: bool` (true for an account that Consort created with a profile folder), `email: Option<String>`, `plan: Option<String>`, `secret_names: Vec<String>` (names only), and `sign_in_methods: Vec<SignInMethod>` (what the app may offer for this account).
- `Inventory` gets `tools: Vec<ToolStatus>`.
- `ToolStatus { tool: ToolId, version: Option<String>, latest: Option<String>, state: ToolState, detail: Option<String> }`.
- `ToolId` is a string newtype with these values: `node`, `claude-code`, `claude-agent-acp`, `codex`, `opencode`, `gemini-cli`, and `acp:<registry id>`.
- `ToolState`: `missing`, `installing`, `ready`, `update_available`, `failed`.

Sign-in types:

- `SignInMethod` (snake_case tag): `claude_plan`, `claude_console`, `chatgpt_device`, `chatgpt_browser`, `api_key { variable: String }`, `acp_agent { method_id: String, name: String }`, `terminal { command: String }` (an ACP terminal method that the app cannot run; the app shows the command).
- `SignIn` is a new feed entity (`EntityKind::SignIn`, `EntityPayload::SignIn`). Fields: `id: SignInId` (prefix `sgn_`), `worker_id`, `account_id`, `method`, `state: SignInState`, `created_at`, `updated_at`, `expires_at`.
- The snapshot of scope `all` also lists the sign-ins that have not ended, in `sign_ins` (DECISIONS row 314), so a client that loads during a sign-in shows it.
- `SignInState` (snake_case tag):
  - `starting`
  - `open_url { url }`
  - `device_code { url, user_code }`
  - `needs_code { url }` (Claude: open the URL, then paste the code)
  - `verifying`
  - `succeeded { detail }`
  - `failed { message }`
  - `cancelled`
  - `expired`

Client API requests (all are `ClientRequest` variants with results):

- `account.create { worker_id, harness, label, acp: Option<AcpAccountSpec>, env: BTreeMap<String, String> }` returns `{ account_id }`. `AcpAccountSpec` is `registry { id }` or `command { program, args }`.
- `account.update { worker_id, account_id, label: Option<String>, env_set: BTreeMap<String,String>, env_remove: Vec<String> }`.
- `account.remove { worker_id, account_id }`. The worker signs out where it can and deletes the profile folder.
- `account.sign_out { worker_id, account_id }`.
- `sign_in.start { worker_id, account_id, method }` returns `{ sign_in: SignIn }`.
- `sign_in.submit_code { sign_in_id, code }`.
- `sign_in.cancel { sign_in_id }`.
- `worker.tools_check { worker_id }`: check for updates now.
- `worker.tool_install { worker_id, tool: ToolId }`: install or update one tool.
- `acp_registry.list {}` returns `{ agents: Vec<AcpRegistryAgent> }` with `id, name, description, version, icon_url, supported: bool` (supported means a distribution exists for at least one platform that the worker code can install: `binary` with sha256, or `npx`).

Worker link:

- Hub to worker requests: `account.create`, `account.update`, `account.remove`, `account.sign_out`, `sign_in.start { sign_in_id, account_id, method }`, `sign_in.code { sign_in_id, code }`, `sign_in.cancel { sign_in_id }`, `tools.check {}`, `tools.install { tool }`.
- Worker to hub event: `sign_in.update { sign_in_id, state }`. Inventory events stay as they are and now carry tools and the new account fields.

New error codes: `sign_in_failed`, `tool_install_failed`, `invalid_tag`. `worker_offline` already exists. Account requests to an offline worker fail with it.

Step 1 notes (the Rust names, as built; see `docs/DECISIONS.md` rows 260 to 269):

- ADR 0009 was taken by "Themes change color only", so the new ADR is 0010.
- `env` and `env_set` are `SecretEnv`, a newtype over `BTreeMap<String, String>` with the same JSON shape. Its `Debug` shows names only. `SignInSubmitCode` and the worker's `SignInCodeCommand` hide `code` in `Debug`.
- `SignInMethod`, `SignInState` and `AcpAccountSpec` are tagged with a `kind` field. `succeeded { detail }` has `detail: Option<String>`.
- Client results: `account.create` gives `AccountCreateResult { account_id }`, `sign_in.start` gives `SignInResult { sign_in }`, `acp_registry.list` gives `AcpRegistryListResult { agents }`. The rest give `Empty`.
- Worker link bodies: `AccountCreateCommand { account_id, harness, label, acp, env }` (the hub chooses the account ID), `AccountUpdateCommand`, `AccountRemoveCommand`, `AccountSignOutCommand`, `SignInStartCommand`, `SignInCodeCommand`, `SignInCancelCommand`, `ToolsCheck {}`, `ToolsInstall { tool }`. Each answers `Empty`. The event is `SignInUpdate { sign_in_id, state }`.
- Tag helpers in `consort_proto::model`: `validate_tag`, `is_valid_tag`, `normalize_tags` (trim, check, drop repeats) and `InvalidTag`, which maps to `invalid_tag`. Tags are ASCII only.
- `ErrorBody::not_built(type)` makes the `unsupported` error. Constants: `SIGN_IN_IDLE_EXPIRY_MS`, `SIGN_IN_RETENTION_MS`, `TAG_MAX_CHARS`.

## Hub (`consort-hub`)

- Store user tags on the worker record (they already are, as labels). `worker.set_tags` validates and writes, and emits a feed upsert.
- Store channel tags. `channel.update` with tags emits a channel upsert.
- Placement: for a coordinator turn, a task or a routine run in a channel with tags, a worker qualifies only if `worker.all_tags()` contains every channel tag. This is added to any `require.tags` from the coordinator. When no online worker qualifies, the work waits. The wait reason names the tags, for example: "Waiting for an online worker with the tag work. Add the tag in Settings > Workers." When no worker at all has the tags, the reason says so too.
- Coordinator context: the channel's tags appear in the coordinator's channel information, and `list_workers` shows tags. The coordinator may still add `require.tags` for a task.
- Relay the account and sign-in requests to the worker, and answer with the worker's result. Never write `env` values to the database or to logs. Redact them in any `Debug` output.
- `SignIn` records live in a new table (migration 4: `sign_ins(id, worker_id, data)`). Each state change from the worker becomes a feed upsert. A sign-in expires after 15 minutes without an update. Delete sign-in rows older than one day.
- `acp_registry.list`: fetch `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`, cache it for one hour, and parse it with the registry types from `consort-tools`.

## Managed tools (new crate `consort-tools`)

The crate owns downloads, verification, install layout and launch specs. It does no hub I/O.

```rust
pub struct ToolStore { /* root: <consort home>/tools, http client, platform */ }
impl ToolStore {
    pub fn new(root: PathBuf) -> Result<Self, ToolError>;
    pub fn installed(&self, tool: &ToolId) -> Option<Installed>; // newest ready version
    pub async fn latest(&self, tool: &ToolId) -> Result<String, ToolError>;
    pub async fn install(&self, tool: &ToolId, version: Option<&str>) -> Result<Installed, ToolError>;
    pub fn status(&self) -> Vec<ToolStatus>;               // from disk plus the last update check
    pub async fn check_updates(&self) -> Vec<ToolStatus>;
    pub fn launch(&self, tool: &ToolId) -> Result<LaunchSpec, ToolError>; // program, args, env
}
```

- Install into a temporary folder next to the target, verify, then rename, so a failed install leaves nothing behind. Keep the previous version until the new one is ready. Remove versions older than the previous one.
- Sources and checks:
  - **Node 24 LTS**: `nodejs.org/dist`, checked against `SHASUMS256.txt`. On musl Linux, use `unofficial-builds.nodejs.org`.
  - **Claude Code**: `https://downloads.claude.ai/claude-code-releases/<version>/<platform>/claude[.exe]`. Read the version from `stable`. Verify the SHA-256 from `manifest.json`, and verify `manifest.json.sig` with the embedded Anthropic GPG key (fingerprint `31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF 1A7E CACE`). If a pure-Rust OpenPGP check is not practical, verify the SHA-256 only and record the gap in DECISIONS.
  - **claude-agent-acp**: `npm install --omit=optional --ignore-scripts --prefix <dir> @agentclientprotocol/claude-agent-acp@<version>`, using the managed Node's own npm. The launch is `node <dir>/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js`, with `CLAUDE_CODE_EXECUTABLE` pointing to the managed `claude`.
  - **Codex**: GitHub release `rust-v<version>`, asset `codex-package-<triple>.tar.gz` (musl triples on Linux), checked against `codex-package_SHA256SUMS`. The launch is `<dir>/bin/codex app-server`.
  - **OpenCode v2**: the `opencode-v2` entry of the ACP registry (binary archive URL and sha256 per platform). Fall back to `https://opencode.ai/update/api/latest/cli/npm` for the version. The launch is `opencode acp`.
  - **Gemini CLI**: `npm install @google/gemini-cli@<version>` with the managed Node. The launch is `node <dir>/node_modules/@google/gemini-cli/bundle/gemini.js --acp`. Check whether the flag is `--acp` or `--experimental-acp` for the installed version.
  - **ACP registry agents**: a `binary` distribution with `sha256`, or `npx` through the managed Node and npm. A `binary` without `sha256` is refused. `uvx` is unsupported for now.
- Turn off self-updates: `DISABLE_AUTOUPDATER=1` for Claude Code, `check_for_update_on_startup = false` in the Codex profile `config.toml`, `OPENCODE_DISABLE_AUTOUPDATE=true` for OpenCode, and `general.enableAutoUpdate: false` in the Gemini profile `settings.json`.
- Use `reqwest` with rustls. Tests use a local HTTP server with fixture archives and checksums. No test downloads from the internet. Gate a real-download smoke test behind `#[ignore]`.

## Worker and harness (`consort-worker`, `consort-harness`, `consort-cli`)

- Consort home: resolve once at start (`CONSORT_HOME`, else `~/.consort`). `consort dev` sets it to `<data>/home` unless `CONSORT_HOME` is set.
- Accounts in `worker.toml`: a created account is written as `{ id, harness, label, profile = true, profile_dir }`, plus `acp` for ACP agents. In the inventory, an ACP registry agent has the harness `acp:<registry id>` and a custom ACP command has `acp:custom`. Its home folders are derived from the profile folder:
  - Claude: `CLAUDE_CONFIG_DIR=<profile>/claude`.
  - Codex: `CODEX_HOME=<profile>/codex`, with `cli_auth_credentials_store = "file"`.
  - Gemini: `GEMINI_CLI_HOME=<profile>/gemini`.
  - OpenCode: `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME` under `<profile>/xdg/`.
  - ACP agents: `HOME` is left alone. Only the env is set.
- Remove account auto-discovery. A new worker has no accounts. Accounts that a user wrote by hand in `worker.toml` still load.
- Launch every harness through `ToolStore::launch`. If the tool is missing, install it first. While it installs, the account status reads "Installing Claude Code". A session that starts then waits for the install.
- Merge `env.json` into the session env, after the profile variables.
- Sign-in flows:
  - **Claude plan or Console**: spawn the managed `claude auth login --claudeai` (or `--console`) in a pseudo-terminal (`portable-pty`, which also works on Windows) with the profile env, `DISABLE_AUTOUPDATER=1`, and a `BROWSER` setting that stops it from opening a browser on the worker. Read the output until the URL appears, then send `needs_code { url }`. On `sign_in.code`, write the code and a newline. Success means the process exits 0 and `claude auth status --json` reports `loggedIn`. Test against a fake `claude` script.
  - **ChatGPT**: start `codex app-server` with the profile `CODEX_HOME` and call `account/login/start` with `chatgptDeviceCode` (send `device_code { url, user_code }`) or `chatgpt` (send `open_url { url }`). Wait for `account/login/completed`. Allow one browser login per worker at a time, because the callback port is fixed.
  - **API key**: the key arrives in `env` at create time, so there is no flow. The probe checks it.
  - **ACP agent method**: call `authenticate { methodId }` on a short-lived agent process with the profile env.
  - **Sign out**: Claude runs `claude auth logout`. Codex calls `account/logout`. An API key account deletes the key from `env.json`. ACP agents call `logout` if the agent offers it.
- Probes: for Claude, use `claude auth status --json` (email, plan, expired) before falling back to the ACP probe. Report `email` and `plan` in the inventory.
- Detected tags: OS, architecture, RAM rounded to whole GB, logical CPUs, and GPU vendor where cheap to detect (`nvidia-smi` on PATH, Apple silicon, `/sys/class/drm` on Linux). Unknown means no tag.
- Tools: check for updates once a day and on `tools.check`. Install on `tools.install`. Report status in the inventory. Never update a tool while a session uses it. Wait until it is idle.

## Client and desktop (`consort-client`, `consort-desktop`)

Build the four Paper screens. Use the logos in `crates/consort-desktop/assets/providers/`. Registry agents load the `icon_url`. Copy changes from the Paper text:

- Worker detail:
  - Under the header, a "Tags" row with removable user tags and "Add tag", with completions from every worker tag. Under it a "Detected" row.
  - An "Accounts" section, with each account's logo, name, plan or problem, status and one action (Sign out, Sign in, Sign in again). The row menu has Rename, Sign out and Remove. The header has an "Add account" button.
  - An "Installed tools" section, with each tool's logo, version and state ("Up to date", "Update to X", "Installing", "Install now"), and a "Check now" button.
  - The "Limits" section stays.
- An API-key account row says "API key saved on <worker>." (the design says "The hub keeps it". That is wrong now: the key lives on the worker.)
- Add account dialog:
  - Step 1 is the provider grid, the ACP registry search (from `acp_registry.list`) and "Custom command" (program and arguments).
  - Step 2 is the name, with buttons for names already used for that provider on other workers, and the profile folder line.
  - Step 3 is sign-in: the method tabs (from `sign_in_methods`), then the flow for that method.
- Sign-in flows in the dialog:
  - `needs_code`: "Open sign-in page", "Copy link", a code field and "Send code".
  - `device_code`: the code in large type, "Open sign-in page" and "Copy code".
  - `open_url`: "Open sign-in page".
  - `api_key`: a password field and "Save".
  - `verifying`: a spinner.
  - `succeeded`: "Signed in as <detail>" and "Done".
  - `failed`: the message and "Try again".
- New channel dialog and channel settings: a "Worker tags" field above "Run tasks on". It follows the Paper artboard "New channel dialog with tags":
  - Each selected tag is a removable chip. "Add tag" opens a menu of every worker tag.
  - Unused tags appear after "Also on workers:", and a click adds one.
  - The note under the field reads "All work in this channel runs only on workers that have every tag here. N workers match." When no worker matches, it says so and suggests tagging a worker.
  - Show the channel's tags as chips in the channel header.
- Remove every "set in worker.toml" notice that the new UI replaces.
- Fix the existing model selection bug: the client sends the model name where the proto wants the model id, and drops options. Keep the id in the selection list and display the name.

## Order and owners

1. Contract: proto changes, renames across crates so the workspace compiles, docs (`CONTEXT.md`, ADR 0010, ADR 0004 note, `protocol.md`, `data-model.md`, `harness-adapters.md`, `architecture.md`). New hub requests return a clear `unsupported` error until step 2 lands.
2. In parallel, with disjoint crates: `consort-tools`, hub, worker and harness, client and desktop.
3. Integration and a real end-to-end run on the Mac (and bigboi if possible), then a review cycle.
