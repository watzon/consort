# Plan: connectors, skills and assistant management tools

Status: designed with the user on 2026-10-29. Paper artboards (Desktop page):
- "Settings, Connectors"
- "Add connector dialog"
- "Connect Gmail dialog"
- "Connectors in a channel"
- "Settings, Skills"
- "Add skill dialog"
- "Channel connectors and skills dialog"
- "Account settings dialog"
- "Everything manages connectors and channels"

## What the user asked for

1. Consort, not the provider, is the source of MCP tools and skills by default. It has a connector marketplace through Composio and a skill marketplace.
2. The user brings a Composio key. Connecting a service happens in the app.
3. Every connector that a channel can use is listed by name in the assistant's context, so it never has to "go look in Composio". Two accounts of one service, such as two Gmail accounts, are told apart. Tools stay out of the context until the assistant lists the tools of one connector.
4. Workers must not drift out of sync, so connectors and skills live on the hub and are scoped with the tags that already exist.
5. Provider-side extras, such as claude.ai connectors, plugins and skills, have an account toggle. It is on by default.
6. The assistant can manage all of this. The Everything assistant manages everything. A channel's assistant manages its own channel.

## Model

- **Connection:** one connected account of one Composio service. It has:
  - A name, for example "Gmail Work", and an assistant handle, for example `gmail_work`. The handle is a unique slug of the name.
  - The account identity (email or login).
  - A "Use in" scope: every channel, or channels with all of the given tags.
  - An approval rule for each Composio tool kind: read, create and change, delete. The defaults are run without asking, ask every time, and ask every time. The `readOnlyHint`, `createHint`, `updateHint` and `destructiveHint` tags decide the kind.
  - A status: active, needs a new sign-in, or failed.
- **Skill:** a folder that follows the Agent Skills format. It is installed from a source (a git repo plus a path, a directory entry, or an upload) and pinned to a commit SHA plus a content hash. It has an on/off state, a "Use in" scope, a license, a list of scripts, audit results and an update state.
- **Channel exceptions:** a channel can turn off any connection or skill that its "Use in" scope gives it. It cannot turn on one that the scope does not give it; that needs a change to the item's "Use in".
- **Effective set for a session:** the connections and skills whose "Use in" matches the channel's tags, minus the channel's exceptions. The Everything channel matches only "every channel" items, unless it has tags of its own.
- **Provider extras toggle:** a per-account setting on the worker, on by default. Off means the worker starts that account's harness without its account-synced extras. For Claude Code, that means no claude.ai MCP servers and no synced plugins or skills. Find the supported switches: an env var or setting for claude.ai MCP servers, plus the plugin and skill sync settings. If something cannot be turned off, say so in the dialog.

## Connectors (Composio)

- The hub stores the Composio project key (encrypted at rest with a hub-generated key, file mode 0600) and one stable `user_id` per hub. The key never leaves the hub.
- The client uses a hand-written `reqwest` client for the Composio v3.1 HTTP API, kept to what is needed:
  - toolkits and categories
  - tools of a toolkit
  - auth configs
  - `connected_accounts/link`, get, list and delete
  - `tools/execute`
- The catalog is cached on the hub and refreshed daily. Logos are cached too.
- Connect flow:
  1. The hub creates or reuses an auth config, Composio-managed first.
  2. It calls `/connected_accounts/link` with an alias and returns the redirect URL.
  3. The app opens the URL in the system browser.
  4. The hub polls until the account is active, with a 10 minute limit.
  - Non-OAuth schemes, such as an API key, use a form in the dialog. The key goes app to hub to Composio and is not stored by Consort.
- Session context: every coordinator and task session gets a "Connectors in this channel" block. It has one line per connection, `handle: Service, identity`, and a one-line hint about the two tools.
- Hub tools, for every session that has at least one connection:
  - `list_connector_tools { connector, search? }` returns tool slugs, short descriptions and the approval kind. With a `search`, it filters, which matters for services with many tools.
  - `get_connector_tool { connector, tool }` returns the input schema.
  - `run_connector_tool { connector, tool, arguments }`: the hub checks that the connection is in the session's effective set and applies the approval rule. An "ask" posts a connector approval card that shows the arguments in a readable form; for example, an email shows To, Subject and Body. The hub then executes with the exact `connected_account_id`, logs the call (connection, tool, argument hash, Composio log ID, decision, result size, time) and returns the result. Results over the frame limit are truncated with a note.
- An expired connection shows "Needs a new sign-in" in Settings and in the assistant's list. `run_connector_tool` then fails with a clear error that tells the assistant to ask the user to reconnect.

## Skills

- Sources:
  - Smithery public registry search.
  - Anthropic's skills repo and Claude plugin `marketplace.json` files that the user adds.
  - A git URL (with an optional path).
  - A zip upload.
  - skills.sh is a link plus pasting an `owner/repo`, because its API needs a Vercel OIDC token.
- Install: fetch at a commit and validate the format (name, description, size and file count limits). Refuse path traversal, symlinks that leave the folder, and absolute paths. Store the files content-addressed on the hub. Record the license and the scripts. Show audit results where a directory gives them.
  - A skill whose audit failed is blocked.
  - A skill with scripts stays off until the user approves it.
  - Claude-specific `hooks` in a skill are removed.
- Updates: check sources daily. A new commit becomes "New version to review" with a file diff. Only approval moves the pin. Running sessions keep their version.
- Delivery: the worker gets the bundles for a session's effective set from the hub (cached by hash) and writes them into a per-session folder:
  - Claude Code: a generated skills-only plugin directory (`CLAUDE_CODE_PLUGIN_DIRS` or the SDK `plugins` option).
  - Codex: `$CODEX_HOME/skills` of a per-session overlay, or `.agents/skills` in the session cwd.
  - Gemini CLI: `$GEMINI_CLI_HOME/.gemini/skills`.
  - OpenCode: the `skills` path list in `OPENCODE_CONFIG_CONTENT`. Also isolate `HOME` for OpenCode so that `~/.claude/skills` and `~/.agents/skills` do not leak in.
  - Other ACP agents: `list_skills` and `read_skill` hub tools plus a one-line prompt hint.
  - Verify delivery where the harness reports it, and show which skills a session got.

## Assistant management tools

Every management tool is a hub tool with an audience, like the existing tools. The hub enforces the scope, not the prompt.

**Rule for risk.** A management action runs at once and posts a short activity line when it:
- adds something new and narrow, or
- makes access smaller or approvals stricter.

It asks the user first with a management approval card when it:
- deletes or archives something,
- widens access (more channels or workers, or "every channel"),
- makes approvals looser,
- turns on a skill with scripts,
- changes worker or channel tags, or
- turns provider extras on.

The assistant never handles a secret. A sign-in or a key always goes through a card that the user completes in the app or the browser.

**Everything assistant (audience `EverythingCoordinator`), in addition to the tools it has now:**
- Channels:
  - `create_channel { name, purpose, group?, tags?, model?, task_model? }`
  - `update_channel { channel, name?, purpose?, instructions?, group?, tags?, model?, task_model?, repos? }`
  - `archive_channel { channel }`, which asks first.
- Models: `set_default_model { model selection }`, the Everything default.
- Workers:
  - `set_worker_tags { worker, tags }`, which asks first.
  - `install_tool { worker, tool }`
- Accounts:
  - `add_account { worker, provider, name }`, which posts a sign-in card.
  - `start_sign_in { worker, account, method }`, which posts a sign-in card.
  - `set_account_extras { worker, account, enabled }`, which asks first when it turns them on.
- Connectors:
  - `search_connector_catalog { search, category? }`
  - `list_connections`
  - `connect_service { service, name, use_in?, approvals? }`, which posts a connect card. The user continues in the browser.
  - `update_connection { connection, name?, use_in?, approvals? }`, which follows the risk rule.
  - `disconnect { connection }`, which asks first.
- Skills:
  - `search_skills { search, source? }`
  - `list_installed_skills`
  - `add_skill { source, path?, use_in? }`, which posts an install card with the safety facts.
  - `update_skill { skill, enabled?, use_in?, accept_new_version? }`, which follows the risk rule.
  - `remove_skill { skill }`, which asks first.

**A channel's assistant (audience `ChannelCoordinator`), only for its own channel:**
- `update_this_channel { purpose?, instructions?, model?, task_model?, repos? }`
- `set_channel_tags { tags }`, which asks first.
- `set_channel_connector { connection, enabled }` and `set_channel_skill { skill, enabled }`, for items its scope gives it.
- `list_connections` and `list_installed_skills`, showing only what the channel can see, plus what is off in it.
- `request_access { connection | skill }`: posts a card that asks the user to widen the item's "Use in" to this channel.
- `connect_service` and `add_skill`, scoped to this channel by default ("Use in: channels with this channel's tags", or this channel only).

**Task sessions:** no management tools. They keep `list_connector_tools`, `get_connector_tool` and `run_connector_tool` for their channel's effective set.

**Cards** (new message kinds in the timeline, rendered like approvals):
- Connect card: service, name, "Use in", approvals, then Continue in browser, Change settings, Not now.
- Sign-in card: the account sign-in flow inline, the same states as the sign-in dialog.
- Install card: skill, source, license, scripts, audits, then Add or Not now.
- Management approval card: what changes and its effect, for example "GitHub Work becomes available in 4 more channels", then Allow or Don't.

## Order

1. Proto and docs contract.
2. In parallel:
   - Hub: Composio client, connections, the connector hub tools, approvals and audit, management tools and cards.
   - Worker: skill delivery per harness, the provider extras toggle, and OpenCode HOME isolation.
   - Desktop: Settings > Connectors, Settings > Skills, dialogs, the channel dialog, the account dialog and the new cards.
3. Skills sources and install pipeline (hub).
4. A live end-to-end run with the user's Composio key, if the user provides one through the app.

## Step 1 notes

The contract as built (the Rust names in `consort-proto`, and the wire names). See `docs/DECISIONS.md` rows 343 to 358, ADR 0011, protocol.md and data-model.md.

**IDs** (`consort_proto::ids`): `ConnectionId` (`con_`), `SkillId` (`skl_`), `ConnectorCallId` (`ccl_`, an audit record).

**Error codes** (`ErrorCode`): `ConnectorNotAvailable` (`connector_not_available`), `ComposioNotConfigured` (`composio_not_configured`), `ComposioError` (`composio_error`), `NeedsUser` (`needs_user`), `SkillBlocked` (`skill_blocked`), `SkillInvalid` (`skill_invalid`).

**Connector model** (`consort_proto::model`, file `model/connector.rs`):

- `Connection { id, name, handle, service, service_name, logo_url, identity, use_in, approvals, status, last_used_at, created_at, updated_at }`, a feed entity (`EntityKind::Connection`, `"connection"`, no channel). `Connection::is_available_in(&Channel)`.
- `UseIn` (`kind`): `EveryChannel` (`every_channel`, the default), `Tags { tags }` (`tags`), `Channels { channel_ids }` (`channels`). `matches`, `matches_channel`, `normalized`, `added_channels(old, channels)` (for the risk rule).
- `ToolKind`: `Read`, `Change`, `Delete`, with `ToolKind::from_hints(read_only, create, update, destructive)`. `ApprovalRule`: `RunWithoutAsking`, `AskOncePerTask`, `AskEveryTime`, with `strictness()`. `ToolApprovals { read, change, delete }`, `Default` is the plan's defaults, `rule_for(kind)`, `is_looser_than(old)`.
- `ConnectionStatus` (`kind`): `Connecting { redirect_url, expires_at }`, `Active`, `NeedsSignIn { message }`, `Failed { message }`. This is how connect attempts are delivered: there is no attempt entity. `CONNECT_ATTEMPT_TIMEOUT_MS` (10 minutes), `CATALOG_REFRESH_MS` (one day).
- Handles: `connection_handle(name)`, `unique_connection_handle(name, taken)`, `validate_connection_name`, `CONNECTION_NAME_MAX_CHARS` (80), `CONNECTION_HANDLE_MAX_CHARS` (40).
- Catalog: `ConnectorService { slug, name, description, logo_url, categories, tool_count, auth_schemes, connected_count }`, `AuthScheme { kind, managed, fields }`, `AuthSchemeKind` (`oauth2`, `oauth1`, `api_key`, `bearer_token`, `basic`, `no_auth`, `other`), `CredentialField { name, label, description, secret, required }`, `ConnectorCategory { slug, name, service_count }`, `ConnectorToolSummary { slug, description, kind }`.
- Audit: `ConnectorCall { id, connection_id, channel_id, session_id, task_id, tool, tool_kind, arguments_sha256, decision, outcome, error, composio_log_id, result_bytes, duration_ms, at }`, `CallDecision` (`ran_without_asking`, `allowed_by_user`, `allowed_earlier_in_task`, `denied_by_user`, `expired`), `CallOutcome` (`succeeded`, `failed`, `not_run`), `canonical_json(&Value)` for the hash.
- Composio: `ComposioStatus { key_present, key_hint, plan, usage, error, checked_at }`, `ComposioUsage { used, limit, resets_at }`.
- Context: `ConnectorContextEntry { handle, service_name, identity, needs_sign_in }` (`From<&Connection>`), `render_connectors_block(&[..])`. `DisplayField { label, value }` for card arguments.

**Skill model** (`model/skill.rs`):

- `Skill { id, name, description, source, commit, content_hash, license, scripts, audits, enabled, use_in, update, blocked_reason, hooks_removed, file_count, size_bytes, installed_at, updated_at }`, a feed entity (`EntityKind::Skill`, `"skill"`). `has_scripts()`, `is_available_in(&Channel)`, `bundle_ref()`.
- `SkillSource` (`kind`): `Git { url, path, git_ref }` (wire `ref`), `Directory { directory, entry_id, url, path }`, `Upload { file_name }`. `SkillDirectoryKind`: `smithery`, `anthropic`, `marketplace`, `skills_sh`.
- `SkillUpdateState` (`kind`): `UpToDate`, `Available { commit, content_hash, changes, scripts_changed, found_at }`, `CheckFailed { message }`. `SkillFileChange { path, change }`, `SkillFileChangeKind` (`added`, `modified`, `removed`).
- `SkillAudit { source, status, summary, url }`, `AuditStatus` (`passed`, `warning`, `failed`, `unknown`).
- Files and bundles: `SkillFileInfo { path, sha256, size, executable }`, `SkillBundleRef { skill_id, name, content_hash }`, `SkillBundle { skill_id, name, content_hash, files }` with `is_consistent()`, `skill_content_hash(&[SkillFileInfo])`, `validate_skill_files`, `validate_skill_name`, `validate_skill_description`.
- Directory: `SkillDirectorySource { id, kind, name, url, removable }`, `SkillDirectoryEntry { entry_id, directory, source_id, name, description, author, url, repo_url, path, license, installs, audits, installed_skill_id }`.
- Context: `SkillContextEntry { name, description }`, `render_skills_block(&[..], native)`.
- Limits: `SKILL_NAME_MAX_CHARS`, `SKILL_DESCRIPTION_MAX_CHARS`, `SKILL_MAX_FILES` (500), `SKILL_MAX_FILE_BYTES` (5 MiB), `SKILL_MAX_TOTAL_BYTES` (20 MiB), `SKILL_UPLOAD_MAX_BYTES` (25 MiB), `SKILL_READ_MAX_BYTES` (512 KiB), `SKILL_MANIFEST_FILE`, `SKILL_UPDATE_CHECK_MS`.

**Channels and accounts:**

- `Channel.disabled_connections: Vec<ConnectionId>` and `Channel.disabled_skills: Vec<SkillId>` (serde default, empty).
- `HarnessAccount.extras_enabled: bool` (missing reads as true) and `HarnessAccount.extras: Option<ProviderExtras>`. `ProviderExtras { mcp_servers, plugins, skills, cannot_turn_off }`, `ExtrasGroup { count, names }`, `PROVIDER_EXTRAS_NAMES_MAX` (20).
- `client_api::AccountUpdate.extras_enabled: Option<bool>` and `worker_link::AccountUpdateCommand.extras_enabled: Option<bool>`.

**Hub cards** (`model/waiting.rs`): new `ApprovalDetails` variants `Connector(Box<ConnectorCard>)`, `Management(Box<ManagementCard>)`, `ConnectService(Box<ConnectServiceCard>)`, `SignIn(Box<SignInCard>)`, `SkillInstall(Box<SkillInstallCard>)`, with the wire kinds `connector`, `management`, `connect_service`, `sign_in`, `skill_install` and the same `ApprovalKind` variants (`ApprovalKind::as_str`, `is_hub_card`). Helpers: `ApprovalDetails::is_hub_card`, `ApprovalDetails::hub_card_options` (Allow and Don't, Continue in browser and Not now, Sign in and Not now, Add and Not now), `Approval::is_hub_card`, `hub_card_request_id(approval_id)` (`hub:<approval id>`), `HUB_CARD_REQUEST_PREFIX`, `HUB_CARD_ALLOW` (`allow`), `HUB_CARD_DENY` (`deny`). The hub already reads a hub card kind from a worker as `other` (`link/events.rs`, `worker_approval_details`).

**Turn input:** `WakeEvent::CardAnswered { id: ApprovalId, kind: ApprovalKind, outcome: ApprovalStatus, text }`, rendered as `<card_answered id="apr_..." kind="management" outcome="allowed">text</card_answered>`.

**Client requests** (`ClientRequest` variant, body type, wire name, response):

| Variant and body | Wire name | Response |
|---|---|---|
| `ComposioSetKey { api_key }` (redacted `Debug`) | `composio.set_key` | `ComposioStatusResult { status }` |
| `ComposioRemoveKey {}` | `composio.remove_key` | `ComposioStatusResult` |
| `ComposioGetStatus {}` | `composio.status` | `ComposioStatusResult` |
| `ConnectorCatalogSearch { search, category, cursor, limit }` | `connector_catalog.search` | `ConnectorCatalogSearchResult { services, next_cursor }` |
| `ConnectorCatalogCategories {}` | `connector_catalog.categories` | `ConnectorCatalogCategoriesResult { categories }` |
| `ConnectorCatalogService { service }` | `connector_catalog.service` | `ConnectorServiceDetail { service, tools }` |
| `ConnectionConnect { service, name, use_in, approvals, auth_scheme, credentials: SecretEnv, approval_id }` | `connection.connect` | `ConnectAttempt { connection, redirect_url, expires_at }` |
| `ConnectionUpdate { connection_id, name, use_in, approvals }` | `connection.update` | `ConnectionResult { connection }` |
| `ConnectionDisconnect { connection_id }` | `connection.disconnect` | `Empty` |
| `ConnectionReconnect { connection_id, credentials: SecretEnv }` | `connection.reconnect` | `ConnectAttempt` |
| `ConnectionCalls { connection_id, before, limit }` | `connection.calls` | `ConnectionCallsResult { calls, has_more }` |
| `ChannelSetConnectionEnabled { channel_id, connection_id, enabled }` | `channel.set_connection_enabled` | `ChannelResult` |
| `ChannelSetSkillEnabled { channel_id, skill_id, enabled }` | `channel.set_skill_enabled` | `ChannelResult` |
| `SkillDirectorySearch { search, source, cursor, limit }` | `skill_directory.search` | `SkillDirectorySearchResult { entries, next_cursor, unavailable }` |
| `SkillDirectorySources {}` | `skill_directory.sources` | `SkillDirectorySourcesResult { sources }` |
| `SkillDirectoryAddSource { url }` | `skill_directory.add_source` | `SkillDirectorySourcesResult` |
| `SkillDirectoryRemoveSource { id }` | `skill_directory.remove_source` | `SkillDirectorySourcesResult` |
| `SkillPreview { source: SkillInstallSource }` | `skill.preview` | `SkillPreviewResult { name, description, commit, content_hash, license, scripts, audits, files, has_hooks, blocked_reason, installed_skill_id }` |
| `SkillInstall { source, use_in, enabled, approval_id }` | `skill.install` | `SkillResult { skill }` |
| `SkillUpdate { skill_id, enabled, use_in, accept_new_version }` | `skill.update` | `SkillResult` |
| `SkillRemove { skill_id }` | `skill.remove` | `Empty` |
| `SkillFiles { skill_id, version: SkillVersion }` | `skill.files` | `SkillFilesResult { content_hash, files }` |
| `SkillReadFile { skill_id, path, version }` | `skill.read_file` | `SkillFileContent { path, sha256, size, content, binary, truncated }` |

`SkillInstallSource` (`kind`): `Directory { source, entry_id }`, `Git { url, path, git_ref }` (wire `ref`), `Upload { blob_sha256, size, file_name }`. `SkillVersion`: `installed` (default), `pending`. Page limits: `CATALOG_PAGE_MAX`, `CONNECTOR_CALLS_PAGE_MAX`, `SKILL_DIRECTORY_PAGE_MAX`. The bodies live in `client_api/connectors.rs` and `client_api/skills.rs`, and are re-exported from `client_api`.

**Snapshot and feed:** `Snapshot.connections: Vec<Connection>` and `Snapshot.skills: Vec<Skill>` (scope `all`, left out when empty). `EntityPayload::Connection` and `EntityPayload::Skill`, with the entity ID as the feed ID and no channel.

**Worker link:**

- `SessionStart.extensions: SessionExtensions { connectors: Vec<ConnectorContextEntry>, skills: Vec<SkillBundleRef>, skill_context: Vec<SkillContextEntry> }`, left out when empty. `SessionExtensions::prompt_blocks(native_skills)` gives the context text.
- `WorkerToHub::SkillBundleGet(SkillBundleGet { content_hash })`, wire `skill_bundle.get`, response `SkillBundle`.
- No new hub command.

**Hub tools** (`HubTool` variant, wire name, args type, result type, audience):

| Variant | Name | Args | Result | Audience |
|---|---|---|---|---|
| `ListConnectorTools` | `list_connector_tools` | `ListConnectorToolsArgs { connector, search }` | `ListConnectorToolsResult { connector, service_name, tools: Vec<ConnectorToolInfo>, has_more }` | every session |
| `GetConnectorTool` | `get_connector_tool` | `GetConnectorToolArgs { connector, tool }` | `GetConnectorToolResult { connector, tool (flattened), input_schema }` | every session |
| `RunConnectorTool` | `run_connector_tool` | `RunConnectorToolArgs { connector, tool, arguments }` | `RunConnectorToolResult` (`status`: `ran`, `waiting_for_user`, `denied`) | every session |
| `ListSkills` | `list_skills` | `NoArgs` | `ListSkillsResult { skills }` | every session |
| `ReadSkill` | `read_skill` | `ReadSkillArgs { skill, path }` | `ReadSkillResult { skill, path, content, truncated, files }` | every session |
| `ListConnections` | `list_connections` | `NoArgs` | `ListConnectionsResult { connections, off_in_this_channel }` | coordinators |
| `ListInstalledSkills` | `list_installed_skills` | `NoArgs` | `ListInstalledSkillsResult { skills, off_in_this_channel }` | coordinators |
| `ConnectService` | `connect_service` | `ConnectServiceArgs { service, name, use_in, approvals }` | `ManagementResult` | coordinators |
| `AddSkill` | `add_skill` | `AddSkillArgs { source, path, use_in }` | `ManagementResult` | coordinators |
| `CreateChannel` | `create_channel` | `CreateChannelArgs { name, purpose, group, tags, model, task_model }` | `ManagementResult` | Everything |
| `UpdateChannel` | `update_channel` | `UpdateChannelArgs { channel, name, purpose, instructions, group, tags, model, task_model, repos }` | `ManagementResult` | Everything |
| `ArchiveChannel` | `archive_channel` | `ArchiveChannelArgs { channel }` | `ManagementResult` | Everything |
| `SetDefaultModel` | `set_default_model` | `SetDefaultModelArgs { model }` | `ManagementResult` | Everything |
| `SetWorkerTags` | `set_worker_tags` | `SetWorkerTagsArgs { worker, tags }` | `ManagementResult` | Everything |
| `InstallTool` | `install_tool` | `InstallToolArgs { worker, tool }` | `ManagementResult` | Everything |
| `AddAccount` | `add_account` | `AddAccountArgs { worker, provider, name }` | `ManagementResult` | Everything |
| `StartSignIn` | `start_sign_in` | `StartSignInArgs { worker, account, method }` | `ManagementResult` | Everything |
| `SetAccountExtras` | `set_account_extras` | `SetAccountExtrasArgs { worker, account, enabled }` | `ManagementResult` | Everything |
| `SearchConnectorCatalog` | `search_connector_catalog` | `SearchConnectorCatalogArgs { search, category }` | `SearchConnectorCatalogResult { services }` | Everything |
| `UpdateConnection` | `update_connection` | `UpdateConnectionArgs { connection, name, use_in, approvals }` | `ManagementResult` | Everything |
| `Disconnect` | `disconnect` | `DisconnectArgs { connection }` | `ManagementResult` | Everything |
| `SearchSkills` | `search_skills` | `SearchSkillsArgs { search, source }` | `SearchSkillsResult { entries }` | Everything |
| `UpdateSkill` | `update_skill` | `UpdateSkillArgs { skill, enabled, use_in, accept_new_version }` | `ManagementResult` | Everything |
| `RemoveSkill` | `remove_skill` | `RemoveSkillArgs { skill }` | `ManagementResult` | Everything |
| `UpdateThisChannel` | `update_this_channel` | `UpdateThisChannelArgs { purpose, instructions, model, task_model, repos }` | `ManagementResult` | channel coordinator |
| `SetChannelTags` | `set_channel_tags` | `SetChannelTagsArgs { tags }` | `ManagementResult` | channel coordinator |
| `SetChannelConnector` | `set_channel_connector` | `SetChannelConnectorArgs { connection, enabled }` | `ManagementResult` | channel coordinator |
| `SetChannelSkill` | `set_channel_skill` | `SetChannelSkillArgs { skill, enabled }` | `ManagementResult` | channel coordinator |
| `RequestAccess` | `request_access` | `RequestAccessArgs { connection, skill }` | `ManagementResult` | channel coordinator |

`ManagementResult { status: ChangeStatus, summary, approval_id, channel_id, connection_id, skill_id, account_id }`, with `ChangeStatus` `applied`, `waiting_for_user`, `refused`, and the constructors `ManagementResult::applied`, `waiting_for_user(approval_id, ..)`, `refused`. `ConnectorToolInfo { slug, description, kind, approval }`. Constants: `CONNECTOR_TOOLS_LIST_MAX` (200), `CONNECTOR_RESULT_MAX_BYTES` (256 KiB), `CONNECTOR_APPROVAL_WAIT_MS` (5 minutes). The new audience constant is `CHANNEL_COORDINATOR_ONLY` (private, like the others). The args and results live in `tools/connectors.rs` and `tools/management.rs`, and are re-exported from `tools`.

**Placeholders for step 2.** Each part answers `unsupported` ("Not built yet: <name>.") until it is built. Remove the entry when you build it:

- Hub client requests: the or-pattern at the end of `consort-hub/src/api/mod.rs`.
- Hub tools: `consort-hub/src/tools/mod.rs`, the `NOT_BUILT` list (which also keeps the tools out of `tool.list`) and the matching arm of `dispatch_tool`. Also make `tool.list` show the connector tools only when the session's channel has a connection, and `list_skills` and `read_skill` only for a harness with no native skills.
- `skill_bundle.get`: `consort-hub/src/net/worker_ws.rs`.
- The extras toggle: `consort-worker/src/accounts.rs` (`update_account` refuses `extras_enabled`), and `core.rs` reports `extras_enabled: true` and `extras: None`.
- The worker ignores `SessionStart.extensions` today. The hub sends an empty set, so nothing changes yet.
- The desktop shows hub cards with the generic approval card (`consort-client/src/reduce.rs`, `approval_detail`) until it has its own cards.
