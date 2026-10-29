# T3 Code provider layer: how it works and whether we can reuse it

Source: shallow clone at `/tmp/t3-re/src`, commit dac8b11d (2026-10-28). Read-only study. Nothing was installed, built or run.

Labels: **FACT** means read in the code. **INFERENCE** means my reading of the code. All paths are relative to `/tmp/t3-re/src` unless stated.

## Summary

- T3 Code has a clean "driver + adapter" seam: `ProviderDriver` (builds one provider instance from config) and `ProviderAdapterV2` (opens sessions, runs turns, streams normalized events). **FACT**
- The seam is good. The packaging is not: every package is `private: true`, there is no publish step, exports point at `.ts` source, and adapters speak T3's own event-sourced "orchestration v2" data model. **FACT**
- The Claude, Codex and Antigravity adapters (the three most important ones) are NOT in packages. They live in `apps/server`. **FACT**
- Stack is Effect 4.0.5 end to end. **FACT**
- License is MIT, "Copyright (c) 2026 T3 Tools Inc." **FACT** (`LICENSE`)

---

## 1. The adapter contract

### 1.1 Two layers

Layer A, the driver (one per harness kind). File: `packages/provider-core/src/server/driver.ts`.

```ts
export interface ProviderDriver<Config, R = never, UsageR = never> {
  readonly driverKind: ProviderDriverKind;
  readonly metadata: ProviderDriverMetadata;
  readonly configSchema: Schema.Codec<Config, unknown>;
  readonly defaultConfig: () => Config;
  readonly create: (input: ProviderDriverCreateInput<Config>) =>
    Effect.Effect<ProviderInstance, ProviderDriverError, R | Scope.Scope>;
  readonly usage?: ProviderUsageReader<Config, UsageR>;
}
```
**FACT.** Comment in the file: "create calls with different instanceId/config must share no mutable state." That rule is what makes multiple accounts safe.

`create` input (`ProviderDriverCreateInput<Config>`): `instanceId`, `displayName`, `accentColor`, `environment`, `enabled`, `config`. **FACT**

`ProviderInstance` is what `create` returns. **FACT**

```ts
interface ProviderInstance {
  instanceId; driverKind; continuationIdentity; displayName; accentColor?; enabled;
  snapshot: ManagedServerProvider;          // health, auth, models, version
  snapshotForCwd?; refreshModels?; invalidateCaches?; consumeResetCredit?;
  orchestrationAdapter: ProviderAdapterV2["Service"];   // runs sessions
  textGeneration: ProviderTextGeneration;               // titles, commit messages
  auth?: ProviderAuthController;                        // sign-in flows
  acpSessionManagement?: { listSessions, logout, deleteSession, listProviders, ... };
}
```

Layer B, the adapter (session and turn control). File: `packages/provider-core/src/server/ProviderAdapter.ts` (648 lines). **FACT**

```ts
export class ProviderAdapterV2 extends Context.Service<ProviderAdapterV2, {
  readonly instanceId: ProviderInstanceId;
  readonly driver: ProviderDriverKind;
  readonly getCapabilities: () => Effect<OrchestrationV2ProviderCapabilities, ...>;
  readonly planSelectionTransition: (input) => Effect<ProviderSelectionTransitionPlan, ...>;
  readonly openSession: (input: ProviderAdapterV2OpenSessionInput)
    => Effect<ProviderAdapterV2SessionRuntime, ProviderAdapterV2Error, Scope.Scope>;
}>()
```

`openSession` input: `threadId`, `providerSessionId`, `modelSelection`, `runtimePolicy`, `resumeFromSession?`, `initialNativeThreadId?`. **FACT**

`ProviderAdapterV2SessionRuntime` is the live object. Selected members (quoted from the interface): **FACT**

```ts
readonly events: Stream.Stream<ProviderAdapterV2Event, ProviderAdapterV2Error>;
readonly subscribeEvents?: Effect<ProviderAdapterV2EventSubscription>;
readonly ensureThread: (input) => Effect<OrchestrationV2ProviderThread, ...>;
readonly resumeThread: (input) => Effect<OrchestrationV2ProviderThread, ...>;
readonly injectHistory?: (input) => Effect<boolean, ...>;
readonly startTurn: (input: ProviderAdapterV2TurnInput) => Effect<void, ...>;
readonly compactThread?: (input) => Effect<void, ...>;
readonly steerTurn: (input: ProviderAdapterV2SteerInput) => Effect<void, ...>;
readonly interruptTurn: (input: ProviderAdapterV2InterruptInput) => Effect<void, ...>;
readonly respondToRuntimeRequest: (input) => Effect<void, ...>;
readonly readThreadSnapshot: (input) => Effect<ProviderAdapterV2ThreadSnapshot, ...>;
readonly rollbackThread: (input) => Effect<ProviderAdapterV2ThreadSnapshot, ...>;
readonly forkThread: (input) => Effect<OrchestrationV2ProviderThread, ...>;
readonly stopTask?, awaitBackgroundTasks?, stopSubagent?, unloadThread?, mcpApps?, uploadFeedback?
```

Key design point: `startTurn` returns `void`. All output comes back on the `events` stream. **FACT**

Turn input (`ProviderAdapterV2TurnInput`) carries T3 concepts: `appThread`, `runId`, `attemptId`, `rootNodeId`, `providerThread`, `message`, `modelSelection`, `runtimePolicy`, `appContext`. **FACT**

Runtime policy: **FACT**

```ts
ProviderAdapterV2RuntimePolicy = Struct({
  runtimeMode: RuntimeMode, interactionMode: ProviderInteractionMode,
  cwd: NullOr(String), approvalPolicy?: Unknown, sandboxPolicy?: Unknown })
```

### 1.2 Other required pieces

| Need | Type | File |
|---|---|---|
| Health, auth, version, models | `ManagedServerProvider` (`getSnapshot`, `refresh`, `streamChanges`, `applyUsageLimits`, `resolveMaintenance`) returns `ServerProvider` | `provider-core/src/server/snapshot.ts`, `packages/contracts/src/server.ts` |
| Sign-in flows | `ProviderAuthController` (`start`, `complete`, `cancel`, `logout`, `subscribe`, optional `respond`) | `provider-core/src/server/auth.ts` |
| Usage and limits | `ProviderUsageReader` | `provider-core/src/server/usage.ts` (130 lines) |
| Model list | `ServerProvider.models` plus `ModelCatalog` service (`current`, `bundled`, `refreshInBackground`) | `provider-core/src/server/ModelCatalog.ts` |
| Host services | `ProviderHost` (paths, settings, credentials, attachment path resolve, background-work gate) | `provider-core/src/server/ProviderHost.ts` |
| Titles and commit text | `ProviderTextGeneration` | `provider-core/src/server/textGeneration.ts` |

All **FACT**. `ProviderHost` is the explicit dependency-injection seam for the host app. **INFERENCE:** it was made so the packages can leave `apps/server`.

`ServerProvider` (the health snapshot) has these fields: `instanceId`, `driver`, `enabled`, `installed`, `version`, `status`, `auth`, `models`, `slashCommands`, `skills`, `usageLimits`, `versionAdvisory`, `compatibilityAdvisory`, `setup{canAuthenticate,canInstall}`, `nativeSessions{canList,canLoad,canResume,canDelete}`, `runtimePaths{homePath,shadowHomePath}`. **FACT** (`packages/contracts/src/server.ts` lines 203-260)

### 1.3 Where drivers are registered

`apps/server/src/provider/builtInDrivers.ts`: **FACT**

```ts
export const BUILT_IN_DRIVERS = [ CodexDriver, ClaudeDriver, CursorDriver, GrokDriver, FxDriver,
  OpenCodeDriver, AntigravityDriver, PiDriver, MuseDriver, AcpRegistryDriver ];
```

Runtime lookup is in `apps/server/src/provider/ProviderInstanceRegistry.ts` (and `...Hydration.ts`, `...Mutator.ts`). **FACT** (file names). I did not read their bodies.

---

## 2. Per provider: transport, package, auth, limits

| Provider (driver kind) | How T3 drives it | Package / location | Auth detection | Notable limits |
|---|---|---|---|---|
| Codex (`codex`) | Spawns `codex app-server` as a child process and speaks JSON-RPC over stdio. Typed client in `effect-codex-app-server`. | Adapter `apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts` (7,735 lines), driver `apps/server/src/provider/Drivers/CodexDriver.ts`. NOT a package. | Two setup modes (`managed` or `existing`). Managed mode runs ChatGPT sign-in itself (`CodexChatGptAuth.ts`, `CodexManagedTokenBroker.ts`). Reads `auth.json` in the effective home. | Compatibility manifest: Codex `>=0.156.0` supported, `<0.149.0` broken (`apps/server/src/provider/model-manifest.json`). Codex truncates instruction entries over 4,000 bytes. |
| Claude (`claudeAgent`) | In-process `@anthropic-ai/claude-agent-sdk` `query()`, with `pathToClaudeCodeExecutable` set from `binaryPath`. Tool permission via the SDK `canUseTool` callback. | Adapter `.../Adapters/ClaudeAdapterV2.ts` (11,244 lines), driver `.../Drivers/ClaudeDriver.ts`. NOT a package. Dep `^0.3.276` in `apps/server/package.json`. | Status probe. Message: "Claude Code is not authenticated. Run `claude auth login` and try again." (`ClaudeProvider.ts:624`). Auth lives in `CLAUDE_CONFIG_DIR`. | Recommended Claude Code `>=2.1.280` (manifest). SDK mode emits no goal events (`docs/internals/providers.md`). Adapter imports many `apps/server` modules (config, serverSettings, attachmentStore, PortScanner). |
| Cursor (`cursor`) | `@cursor/sdk` 1.0.38: `Agent.create` / `Agent.resume`, then `agent.send(...)`. Local runtime option. | `packages/provider-cursor` (6.8k lines) | API key from OS keychain (`@napi-rs/keyring`), browser sign-in, or `CURSOR_API_KEY` env. Message: "Sign in with Cursor or add CURSOR_API_KEY in provider settings." | Native keyring module (`@napi-rs/keyring`) is a platform-specific dependency. |
| Grok (`grok`) | ACP over stdio: `grok agent stdio`. Built on the shared `provider-acp` adapter. Runtime mode becomes CLI flags (`--permission-mode auto`, `--always-approve`). | `packages/provider-grok` (3.8k) + `provider-acp` | Env `XAI_API_KEY`. Snapshot auth often `unknown`. | xAI-specific ACP extension (`xaiAcpExtension.ts`). |
| OpenCode (`opencode`) | Starts `opencode serve --hostname --port` and uses the HTTP SDK. 1.x uses `@opencode-ai/sdk`. 2.x uses `@opencode/client` 2.0.28. Picks runtime by probing the version. | `packages/provider-opencode` (12.6k, the biggest) | Asks the server which upstream providers are connected. Status `authStatus: "unauthenticated"` when none. | Min version `1.14.19`. MCP registration is per directory, so 1.x runs one server per thread. External servers may need a restart to pick up config. |
| Antigravity (`antigravity`) | ACP over stdio to Google's Antigravity agent. T3 installs the agent itself (immutable releases, atomic pointer, process leases). | `apps/server/src/provider/Drivers/AntigravityDriver.ts`, `.../Adapters/AntigravityAdapterV2.ts` (236 lines, thin over `makeAcpAdapterV2`), `.../provider/acp/Antigravity*.ts`. NOT a package. | Google OAuth through a browser. T3 relays the callback URL (`antigravityCallback.ts`, `AntigravityAuth.ts`). Forces file-based credential storage. | Agent has a bundled Python; broken IPv6 can stall 2 minutes per Google call. Per-instance profile; strips ambient Google credentials. |
| Muse Code (`muse`) | `@muse-code/sdk` 1.4.6, which spawns its own connection (`spawnMspConnection`). | `packages/provider-muse` (4.3k) | Needs `muse login` on the host. Snapshot auth `unknown`. | **INFERENCE:** the SDK protocol is Muse-specific, not ACP. Windows launcher handling written by hand (`.cmd` under `cmd.exe`). |
| Pi (`pi`) | Spawns `pi --mode rpc`, JSON over stdio. Own client `rpc.ts`. Uses `get_state` etc. | `packages/provider-pi` (5.9k) | Uses the user's own Pi install and settings. `PI_CODING_AGENT_DIR` env for usage reads. | `enabled` defaults to false. Pi has no native MCP, so T3 generates a Pi extension (see 5). |
| fx (`fx`) | ACP over stdio: `fx acp`. | `packages/provider-fx` (1.3k, the smallest) | Status probe; models from `fx status`. | Only two runtime modes: `approval-required` and `auto`. |
| ACP Registry (`acpRegistry`) | Generic ACP agents over stdio, discovered from a public registry. | `packages/provider-acp-registry` (5.4k) + `provider-acp` | Per-agent. Files for `claudeAcp`, `copilotAcp`, `devinAcp`, `droidAcp`, plus generic `auth.ts`. | Registry URL: `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`. Some ACP agents drop HTTP MCP servers (see 5). |

Other facts:

- **FACT:** `packages/provider-acp/src/server/AcpSessionRuntime.ts` calls `session/new`, `session/load`, `session/resume`, `session/prompt`. It refuses agents that advertise neither `session/load` nor `session/resume`.
- **FACT:** Grok, fx, ACP Registry and Antigravity all call `makeAcpAdapterV2` from `provider-acp`. So ACP is the cheapest way to add a harness.
- **FACT:** `effect-acp` (24.7k lines, 19.5k generated) and `effect-codex-app-server` (61.2k lines, 58.2k generated) are typed protocol clients, no T3 logic.
- **INFERENCE:** The Claude Agent SDK launches a Claude Code subprocess underneath. Evidence: the `pathToClaudeCodeExecutable` option. "In-process" means the SDK is in T3's process, not the agent loop.

---

## 3. Normalized runtime event model

Defined in `packages/contracts/src/orchestrationV2.ts` (4,150 lines), `packages/contracts/src/providerRuntime.ts` (137 lines) and `packages/provider-core/src/server/ProviderAdapter.ts`. **FACT**

### 3.1 Adapter output stream: `ProviderAdapterV2Event`

A union of `type` values (each also has a `driver` field). **FACT**

```
app_thread.created        provider_session.updated   provider_thread.updated
provider_turn.updated     node.updated               subagent.updated
message.updated           turn_item.updated          command_output.delta
runtime_request.updated   plan.updated               turn.terminal (completed|interrupted|cancelled)
turn.terminal (failed, with failure + retry + threadDisposition)
```

**INFERENCE:** The model is "upsert the latest state of an entity", not a fine-grained delta protocol. Only `command_output.delta` is a pure delta (comment: "Unpersisted deltas go only to clients showing the row"). Assistant text streams as `turn_item.updated` with `streaming: true`. `apps/server/src/orchestration-v2/Adapters/ProviderTextDeltaCoalescer.ts` batches text before it becomes an update.

### 3.2 Turn item kinds: `OrchestrationV2TurnItem` (lines 1551-1771)

| Concern | Item `type` |
|---|---|
| Text | `user_message`, `assistant_message` (`text`, `streaming`) |
| Reasoning | `reasoning` (`text`, `streaming`) |
| Tool calls | `command_execution` (`input`, `output`, `exitCode`, `monitor`), `file_search`, `web_search`, `dynamic_tool`, `subagent` |
| File changes | `file_change` (`fileName`, `additions`, `deletions`, `diffStr`, `oldStr`, `newStr`, `changes`) |
| Plans and todos | `proposed_plan` (markdown), `todo_list` (steps with `pending`/`running`/`completed`) |
| Approvals | `approval_request` (`requestKind`, `prompt`, `options`, `fullAccessOverride`) |
| User questions | `user_input_request` (`questions[]`: header, question, options, `multiSelect`, `allowCustomAnswer`) |
| Errors and state | `error`, `system_notice`, `notification`, `run_interrupt_request`, `run_interrupt_result` |
| Context management | `compaction`, `handoff`, `fork`, `thread_created`, `checkpoint` |
| Secrets and links | `secret_request`, `link_request` |

### 3.3 Other normalized types

- Token usage: `ThreadTokenUsageSnapshot` (`usedTokens`, `maxTokens`, input, cached, output, reasoning, `cost`) and `TurnTokenUsage` (`usageStatus: complete | partial | unavailable`, `hasSubagents`). **FACT** (`providerRuntime.ts`)
- Provider turn status: `pending | running | completed | interrupted | failed | cancelled`. **FACT**
- Session status: `starting | ready | running | waiting | stopped | error`. **FACT** (`ProviderAdapterV2SessionStatus`)
- Failures: `OrchestrationV2ProviderFailure` plus `OrchestrationV2ProviderRetry`. **FACT**
- Tool lifecycle kinds: `command_execution | file_change | mcp_tool_call | dynamic_tool_call | collab_agent_tool_call | web_search | image_view`. **FACT**
- Subagents are first-class: `OrchestrationV2Subagent` and `subagent.updated`. Comment: "Provider-native children cannot receive messages; T3 delegate_task children can." **FACT** (`orchestrationV2.ts:510`)

---

## 4. Approvals and user input

### 4.1 Flow (Claude as the example) **FACT**

1. The SDK calls `canUseTool(toolName, toolInput, {toolUseID})`. The adapter's `canUseToolEffect` (`ClaudeAdapterV2.ts:9435`) makes a pending record with a `Deferred`.
2. The adapter emits `runtime_request.updated` (an `OrchestrationV2RuntimeRequest`) and a turn item `approval_request` or `user_input_request`. Special cases: `AskUserQuestion` becomes a question card, `ExitPlanMode` becomes a plan approval.
3. The server stores the request. The client shows it.
4. The client sends a decision command. Dispatch first persists `status: "resolved"` (comment in `RuntimeRequestService.ts`: "Dispatch persists resolution before this effect runs").
5. `RuntimeRequestServiceV2.respond` (`apps/server/src/orchestration-v2/RuntimeRequestService.ts`) re-reads the request, checks it is `live` and bound to the same `providerSessionId`, finds the session, and calls `session.respondToRuntimeRequest({requestId, decision, feedback, answers})`.
6. `ClaudeAdapterV2.ts:10967` completes the `Deferred` (`pending.decision` or `pending.answers`). The SDK callback returns `{behavior: "allow" | "deny"}`.

Request response modes (`responseCapability`): **FACT**

- `live`: harness is waiting right now. Needs the same live session.
- `message`: answer is sent as a new user message (Codex async questions, per `docs/internals/providers.md`).
- `not_resumable`: gone after restart.

Decisions: `accept | acceptForSession | acceptAlways | decline | cancel`. Request kinds: `command | file-read | file-change | mcp-elicitation | permission`. User answers: `Record<string, unknown>` keyed by question id. **FACT** (`packages/contracts/src/providerPolicy.ts`)

### 4.2 Runtime modes **FACT** (`providerPolicy.ts`)

```ts
RuntimeMode = ["approval-required", "auto-accept-edits", "auto", "full-access"]  // default "full-access"
ProviderInteractionMode = ["default", "plan"]
```

There is no "read-only" runtime mode. Read-only is a sandbox policy. Mapping examples:

| T3 mode | Codex (`CodexAdapterV2.ts:684`) | Claude (`permissionModeForClaudeRuntimePolicy`) |
|---|---|---|
| `approval-required` | `approvalPolicy: "untrusted"`, sandbox `readOnly` | `default`, or `dontAsk` when approval is `never` |
| `auto-accept-edits` | `on-request`, `workspaceWrite` | `acceptEdits` |
| `auto` | `on-request`, `approvalsReviewer: "auto_review"`, `workspaceWrite` | **INFERENCE:** `default` with a T3 review step |
| `full-access` | `never`, `dangerFullAccess` | `bypassPermissions` |
| plan interaction | n/a | `plan` |

- An explicit `approvalPolicy` / `sandboxPolicy` in the runtime policy overrides the mode defaults. **FACT**
- Grok maps modes to CLI args. Pi gets `T3_PI_RUNTIME_MODE` env for its extension. fx supports only `approval-required` and `auto`. **FACT**
- Capability `runtimePolicy.enforcement: "native" | "client-boundary"` records whether the harness enforces the policy or T3 does at the client boundary. **FACT**
- `fullAccessOverride: "ask_rule" | "provider_check"` explains why a prompt appears even in full access. **FACT**

---

## 5. MCP injection and session auth

### 5.1 T3's MCP server **FACT**

- Server: `apps/server/src/mcp/McpHttpServer.ts`, endpoint `/mcp` (comment: "`/mcp` is outside environment auth").
- Tool code: `apps/server/src/mcp/OrchestratorMcpService.ts` and siblings (11.9k lines in `mcp/`). NOT in a package.
- Tools seen: `delegate_task`, `task_status`, `task_cancel`, `orchestrator_capabilities`, `t3_thread_launch`, `t3_thread_send`, `t3_thread_read`, `t3_thread_update`, `t3_thread_interrupt`, `t3_thread_send_attachments`, `preview_*`, `device_*`, `html_render`, `request_secret`, scheduling tools.
- Capability scoping: `McpCapability` set per token (for example `preview`, `device`). Read-only sandbox gets a narrower allow list for Claude.
- The system prompt text for these tools is `T3_CODE_ORCHESTRATION_INSTRUCTIONS` in `packages/provider-core/src/server/orchestrationInstructions.ts`. It also covers other machines: `t3_environment_links`, an `environmentId` argument.

### 5.2 Session authentication **FACT** (`apps/server/src/mcp/McpSessionRegistry.ts`)

- `issue({threadId, providerInstanceId, capabilities, browserToolsAvailable, subagent})` makes 32 random bytes, base64url.
- Only the SHA-256 hash is stored. The raw token goes out as `authorizationHeader: "Bearer <token>"`.
- Tokens have a liveness window. Idle tokens stop working. Revoke by provider session, thread, or all.
- The `McpProviderSessionConfig` handed to adapters (`provider-core/src/server/mcpSession.ts`): `environmentId`, `threadId`, `providerSessionId`, `providerInstanceId`, `endpoint`, `authorizationHeader`, `browserToolsAvailable`, `capabilities`, `agentDeviceEnvironment`.
- `McpProviderSessions` (provider-core) is a per-thread in-memory map of that config. Adapters call `read(threadId)`.

### 5.3 How each harness gets it **FACT**

| Harness | Mechanism | Evidence |
|---|---|---|
| Claude | SDK option `mcpServers: {"t3-code": {type:"http", url, headers:{Authorization}, timeout}}` plus `allowedTools` wildcard | `ClaudeAdapterV2.ts:1700` |
| Codex | Thread config override `mcp_servers.t3-code = {url, tool_timeout_sec, http_headers:{Authorization}}`. Comment: "Override only T3's server so launch-argument MCP servers survive." | `CodexAdapterV2.ts:1340` |
| Cursor | SDK `mcpServers: {"t3-code": {type:"http", url, headers}}` | `provider-cursor/src/server/adapter.ts:204` |
| OpenCode | `client.mcp.add({name:"t3-code", config:{type:"remote", url, headers, oauth:false}})`. 2.x: one entry `t3-code-<thread>` per thread, other threads' entries denied by session permission rules | `provider-opencode/src/server/adapter.ts:957`, `docs/internals/providers.md` |
| Muse | SDK config `{url, headers:{Authorization}}` | `provider-muse/src/server/adapter.ts:1830` |
| Pi | No native MCP. T3 writes a TypeScript extension (source string `PI_T3_MCP_EXTENSION_SOURCE`) and launches `pi --mode rpc --extension <path>` with env `T3_MCP_URL` and `T3_MCP_BEARER_TOKEN`. Inherited values of those vars are deleted first. | `provider-pi/src/server/mcpInjection.ts`, `mcpExtensionSource.ts` |
| ACP family (Grok, fx, ACP Registry, Antigravity) | stdio bridge. `mcpServers: [{name:"t3-code", command:<T3 binary>, args:["acp-mcp-bridge"], env:[T3_ACP_MCP_ENDPOINT, T3_ACP_MCP_AUTHORIZATION]}]`. Comment: "codex-acp 1.2.0 and pi-acp drop injected HTTP servers; bridge through stdio instead." There is also an "MCP over ACP" bridge (`mcpOverAcpBridge.ts`). | `provider-acp/src/server/adapter.ts:593`, `apps/server/src/mcp/AcpMcpStdioBridge.ts`, `apps/server/src/cli/acpMcpBridge.ts` |

**INFERENCE:** The ACP bridge needs the T3 server binary on the same machine as the harness (`T3_ACP_MCP_NODE`, `T3_ACP_MCP_ENTRYPOINT`). For a hub/worker design, the worker would need its own copy of the bridge, and the endpoint must be reachable from the worker.

---

## 6. Provider instances and accounts

### 6.1 Config schema **FACT**

`packages/contracts/src/providerInstance.ts`:

```ts
ProviderInstanceConfig = Struct({
  driver: ProviderDriverKind,
  displayName?: string, accentColor?: string,
  environment?: Array<{ name, value, sensitive, valueRedacted? }>,   // env vars
  enabled?: boolean,
  config?: Unknown })   // decoded by the driver's own configSchema
```

- `ProviderInstanceId` and `ProviderDriverKind` are branded slugs: `^[a-zA-Z][a-zA-Z0-9_-]*$`, max 64 chars.
- Stored in server settings: `providerInstances: Record<ProviderInstanceId, ProviderInstanceConfig>` (`packages/contracts/src/settings.ts:1298`).
- Default instance id equals the driver kind: `defaultInstanceIdForDriver`. So "codex" is the default and "codex_work" would be a second instance.
- Mutations: `create | upsert | remove`.
- Unknown-driver envelopes "round-trip verbatim across versions" (comment). Good forward-compat design.
- Every model choice carries the instance: `ModelSelection = {instanceId, model, options}` (`modelSelection.ts`). A thread therefore binds to an account, not just a harness.

Codex driver config (`CodexSettings`, `settings.ts:619`): `enabled`, `setupMode` (`managed | existing`), `binaryPath`, `homePath` (CODEX_HOME), `shadowHomePath`, `launchArgs`, `customModels`. Form order: `binaryPath, homePath, shadowHomePath, launchArgs`. The `shadowHomePath` description: "Account-specific Codex home. Keeps auth.json separate while sharing state from CODEX_HOME." **FACT**

Claude (`ClaudeSettings`, `settings.ts:698`): `enabled`, `binaryPath`, `homePath` (CLAUDE_CONFIG_DIR), `customModels`, plus idle-compaction fields. Pi: `enabled`, `binaryPath`, `launchArgs`, `customModels`. **FACT**

Secret fields use the `settingsSecret` annotation: stored apart and shown to clients only as "configured". Drivers read shared credentials through `ProviderHost.credentials(namespace, bindingId)`. **FACT**

`supportsMultipleInstances` in `ProviderDriverMetadata` lets a driver reject a second instance of a global resource. **FACT**

### 6.2 How accounts are isolated

- **Environment variables:** `mergeProviderInstanceEnvironment` (`provider-core/src/server/instanceEnvironment.ts`) overlays the instance env on `process.env`. `CODEX_HOME` and `CLAUDE_CONFIG_DIR` get `~` expansion. **FACT**
- **Codex shadow home:** `apps/server/src/provider/Drivers/CodexHomeLayout.ts`. **FACT**
  - `mode: "direct"` when `shadowHomePath` is empty. `mode: "authOverlay"` otherwise.
  - In overlay mode, T3 symlinks shared entries from the shared home into the shadow home: `sessions, archived_sessions, sqlite, shell_snapshots, worktrees, skills, plugins, cache, logs, mcp-oauth-locks, thread-writer-locks`.
  - Private (must be real files, never symlinks): `auth.json`, `auth.lock`, `models_cache.json`.
  - Errors exist for path conflict, entry conflict, private-entry symlink.
  - `continuationKey = codex:home:<sharedHome>`. Two accounts sharing one home share a "continuation group", so a thread can continue under the other account. **INFERENCE** on purpose; the key is a FACT.
  - Managed mode defaults the shadow home to `<stateDir>/providers/codex/<instanceId>/shadow` (`CodexManagedHome.ts`). **FACT**
- **Claude:** the SDK reads `CLAUDE_CONFIG_DIR` from process env, so the adapter sets it per query (`ClaudeAdapterV2.ts:864-870, 4306`). **FACT**
- **Antigravity:** per-instance profile directory. Forces file credential store (the macOS keychain entry would be shared). Removes ambient Google credentials from the launch env. Links only `~/.gemini` skill dirs back. **FACT** (`docs/internals/providers.md`, `antigravityAuthSupport.ts`)
- **Cursor:** credentials in keychain/credential store keyed per binding; refuses browser sign-in when `CURSOR_API_KEY` is set. **FACT** (`provider-cursor/src/server/auth.ts`)
- **OpenCode:** 2.x runs one server per instance. **FACT**

### 6.3 Per-machine and per-project settings

- **FACT:** `ProjectSettingsOverrides.providerInstances` lets a project override an instance (for example `enabled`) (`settings.ts:1057`). Keys are the instance id.
- **FACT:** T3 settings live in each server process (one per machine). `ProviderHost.settings` reads that server's settings. `binaryPath` is resolved on that machine.
- **INFERENCE:** So "per-machine" in T3 means "each T3 server has its own `providerInstances` map". T3 has no central schema that merges per-machine overrides. For our hub/worker design, we would store account definitions on the hub and send each worker a resolved config.
- **FACT:** Prompt text refers to linked environments (`t3_environment_links`, `environmentId`), and the repo has `apps/server/src/relay` and `peer`. I did not study them.

---

## 7. Model catalog and per-turn options

- `ServerProvider.models: ServerProviderModel[]`, each with `capabilities: ModelCapabilities`. **FACT**
- Option system (`packages/contracts/src/model.ts`): **FACT**

```ts
ProviderOptionDescriptor = Union([
  { type:"select", id, label, options:[{id,label,isDefault}], currentValue?, promptInjectedValues? },
  { type:"boolean", id, label, currentValue? } ])
ModelCapabilities = { optionDescriptors?: ProviderOptionDescriptor[] }
ModelSelection    = { instanceId, model, options?: Array<{id, value: string|boolean}> }
```

- The UI is generic: the harness says which knobs a model has, the client draws them. No hard-coded "effort" field in the contract. **FACT**
- Option ids in use: Claude `effort`, `contextWindow` (`ClaudeModelCatalog.ts:220,250`); Codex `reasoningEffort`, `serviceTier` (`CodexProvider.ts:203-212`); Grok and Muse `reasoningEffort`; Pi `thinking`; fx has an effort descriptor. **FACT**
- **INFERENCE:** Codex "fast mode" is the `serviceTier` option. I did not read its choices.
- Sources of model lists: static `apps/server/src/provider/model-manifest.json` (bundled, also holds version compatibility ranges), live probes (Codex `model/list`, OpenCode catalog, Cursor SDK catalog, ACP `session/new` model lists), plus user `customModels`. `ModelCatalog.refreshInBackground` refreshes. **FACT**
- Defaults per driver: `DEFAULT_MODEL_BY_PROVIDER` in `model.ts`. Some defaults are placeholder slugs that are never sent to the harness (`antigravity-default`, Muse `default`, Grok `grok-build`). **FACT**

### Capability flags

Two levels. **FACT**

1. Coarse flags on `ServerProvider` (for UI): `showInteractionModeToggle`, `reportsContextWindow`, `supportedRuntimeModes`, `supportsConversationRollback`, `supportsTextGeneration`, `setup`, `nativeSessions`, `configurableProviders`.
2. Deep `OrchestrationV2ProviderCapabilities` from `getCapabilities()` with 11 groups: `sessions`, `threads`, `turns`, `streaming`, `tools`, `approvals`, `planning`, `subagents`, `context`, `checkpointing`, `identity`, `runtimePolicy`. Examples: `supportsActiveSteering`, `supportsQueuedMessages`, `terminalStatusQuality: strong|weak|none`, `streamsReasoning`, `supportsMcpTools`, `approvalCallbacksAreLiveOnly`, `canForkFromTurn`, `supportsFullThreadHandoff`, `maxRecommendedHandoffChars`. Spec: `docs/orchestration-v2/provider-capability-system.md` (284 lines).

Switching a thread between providers uses a portable "handoff summary" (`ContextHandoffService.ts`, `supportsProviderSwitchingViaHandoff`). **FACT**

---

## 8. Reusability

### 8.1 Dependencies **FACT**

- `effect: 4.0.5` and `@effect/platform-node: 4.0.5` in `pnpm-workspace.yaml` catalog. This is Effect 4 (APIs used: `Context.Service`, `Schema.Literals`, `Schema.TaggedError`, `effect/process`, `effect/http`). Not compatible with Effect 3 code.
- Each provider package depends on `effect`, `@t3tools/contracts`, `@t3tools/shared`, `@t3tools/provider-core`, plus:
  - cursor: `@cursor/sdk` 1.0.38, `@napi-rs/keyring`, `yaml`
  - muse: `@muse-code/sdk` 1.4.6
  - opencode: `@opencode-ai/sdk`, `@opencode/client|protocol|schema` 2.0.28
  - acp: `diff`; acp-registry: `@noble/hashes`
  - effect-acp, effect-codex-app-server: `effect` only
- `apps/server`: `@anthropic-ai/claude-agent-sdk ^0.3.276`, `effect-codex-app-server`.
- Build tooling: `vite-plus`, TypeScript via `@tsc-rs/typescript`, bun scripts.

### 8.2 Packaging **FACT**

- Every provider package has `"private": true`, no `version`, `workspace:*` deps, and exports like `"./server/driver": {"types": "./src/server/driver.ts", "import": "./src/server/driver.ts"}`. They ship TypeScript source, no build output, no publish config.
- Reuse requires vendoring (copy) or a workspace submodule with a TS-capable loader.

### 8.3 Size (non-test lines, from `find ... | wc -l`) **FACT**

| Piece | Lines | Notes |
|---|---:|---|
| `provider-core` | 7,395 | driver/adapter contracts, host, MCP session, snapshot, auth |
| `provider-acp` | 14,233 | shared ACP adapter and runtime |
| `provider-acp-registry` | 5,374 | |
| `provider-cursor` | 6,813 | |
| `provider-fx` | 1,340 | |
| `provider-grok` | 3,820 | |
| `provider-muse` | 4,295 | |
| `provider-opencode` | 12,604 | |
| `provider-pi` | 5,925 | |
| `provider-testing` | 520 | fake CLI, replay transcripts |
| **Provider packages subtotal** | **~62,300** | |
| `effect-acp` | 24,734 | 19,457 generated |
| `effect-codex-app-server` | 61,213 | 58,239 generated |
| `contracts` | 25,685 | `orchestrationV2.ts` alone is 4,150 |
| `shared` | ~25,200 | 24 modules imported by providers (HostProcess, model, shell, observability, AgentScope, ...) |
| `apps/server/src/provider/` | 19,437 | Claude/Codex/Antigravity drivers, auth, status, usage |
| `apps/server/.../Adapters/` | ~19,300 | `ClaudeAdapterV2` 11,244, `CodexAdapterV2` 7,735, Antigravity 236 |
| `apps/server/src/mcp/` | 11,876 | T3 MCP server and toolkits |
| `apps/server/src/orchestration-v2/` | 88,843 | event store, projections, session manager, ingestor |

### 8.4 Coupling

- **FACT:** The 8 provider packages import nothing from `apps/server`. Only two test files mention it. The `ProviderHost` service is the seam.
- **FACT:** But the packages import `@t3tools/contracts` at 173 sites. The adapter contract is written in T3 domain types (`OrchestrationV2AppThread`, `RunId`, `NodeId`, `CheckpointId`, `ThreadId`). You cannot use the adapters without also taking `contracts` and a host that understands those types.
- **FACT:** Claude and Codex adapters import many `apps/server` modules (`config.ts`, `serverSettings.ts`, `attachmentStore.ts`, `preview/PortScanner.ts`, `provider/*`). They need extraction work before they can move.
- **FACT:** Session management lives in `apps/server/src/orchestration-v2/` (`ProviderSessionManager.ts`, `ProviderEventIngestor.ts`, `ProviderRuntimeRecoveryService.ts`, `ProviderSwitchService.ts`, `RuntimeRequestService.ts`). That is the code that consumes the event stream.
- **FACT:** Node-only: child processes, symlinks, `node:crypto`, native keyring module. Windows paths are handled in several places (PATHEXT, `taskkill`).
- **FACT:** Tests are a plus. `provider-testing` has `fakeCli`, `replayTranscript`, `replayGate`, `TestProviderHost`. About 31k test lines exist across the provider packages (not counting effect-acp and effect-codex-app-server).

### 8.5 Can another TypeScript project vendor `provider-core` + providers?

Yes for the ACP-based and SDK-based drivers, with a lot of baggage. **INFERENCE**

Minimum bring-along set for non-Claude/Codex providers:
`provider-core` + `provider-acp` + `effect-acp` + the chosen `provider-*` + `contracts` + `shared` + Effect 4. That is roughly 135k lines, 80% in `contracts`, `shared` and generated protocol code. Then you must write your own `ProviderHost` and your own consumer of `ProviderAdapterV2Event` (the equivalent of `orchestration-v2`).

For Claude and Codex you also need to extract `ClaudeAdapterV2` (11k), `CodexAdapterV2` (7.7k) and their `apps/server` imports, or write new ones.

---

## Reuse options

### Option A: Vendor the packages (copy into our repo, keep MIT notice)

- **What you get:** drivers for Cursor, Grok, fx, Muse, Pi, OpenCode, ACP Registry, plus `provider-core` contracts, plus the shadow-home logic, MCP injection code, and test harness.
- **What you must also bring:** `contracts`, `shared`, `effect-acp`, Effect 4.0.5 and its Node platform package. Extract Claude, Codex and Antigravity adapters from `apps/server`. Write a `ProviderHost` and an event consumer.
- **Effort:** high. Estimate 6 to 10 engineer-weeks to get Claude + Codex + 2 ACP agents running in our shape. **INFERENCE**
- **Risks:**
  - Constant drift. The repo moves daily (the model manifest was updated the same day as this commit). There is no semver or changelog contract for the packages.
  - The adapter input types assume T3's run/node/checkpoint model. Our "main chat + channels + tasks" model must map onto it or we fork the types.
  - Effect 4 is a hard stack choice for the whole worker.
  - Third-party SDK licenses (`@cursor/sdk`, `@muse-code/sdk`, `@opencode/*`, Claude Agent SDK terms) are not covered by T3's MIT. I did not check them.
  - Cursor and Muse SDKs may require their own account terms. **INFERENCE**

### Option B: Fork the whole app

- **What you get:** everything, including orchestration-v2, MCP server, auth, relay/peer, web and mobile apps.
- **Effort:** low to start, high to diverge. Our hub/worker split and project channels conflict with T3's single-server-per-machine design. **INFERENCE**
- **Risks:** you inherit a very large product codebase you did not choose (orchestration-v2 alone is 88.8k non-test lines). Merge pain from upstream. Their product (threads, worktrees, PRs) is not ours.

### Option C: Reimplement using T3 as the reference (recommended starting point)

- **What to copy as design:**
  - The two-layer `ProviderDriver` / `ProviderAdapter` split with a per-instance `create` that shares no state.
  - A small event vocabulary (section 3). Our version can be about 10 event types.
  - Capability flags as data, not `if (provider === ...)`.
  - `ProviderInstanceConfig` shape: `{driver, displayName, environment[], enabled, config}` with the driver owning its `config` schema.
  - Codex shadow-home layout and `continuationKey`.
  - Per-session MCP token: random token, hashed at rest, liveness window, scoped capabilities.
  - ACP stdio MCP bridge for agents that drop HTTP MCP.
  - Pi extension trick for harnesses without MCP.
  - Option descriptors for model knobs.
  - The `docs/internals/providers.md` "protocol traps" list.
- **What to reuse directly (no T3 domain types):** `effect-acp` and `effect-codex-app-server` protocol clients (if we accept Effect 4), or use the official ACP and Codex schemas and a thin client in our own stack.
- **Effort:** medium. Start with ACP (covers Grok, fx, Antigravity, ACP Registry, Pi via adapter, Claude/Codex via ACP bridges) + Claude Agent SDK + Codex app-server (Pi through RPC or `pi-acp`; `provider-acp-registry/src/server/claudeAcp.ts` shows a Claude-over-ACP route). Estimate 4 to 6 engineer-weeks for 3 harnesses with accounts and approvals. **INFERENCE**
- **Risks:** you carry the protocol-trap learning cost yourself. Mitigate by reading `docs/internals/providers.md` and the adapter tests, and by porting `provider-testing` replay fixtures.

### Suggested path **INFERENCE**

1. Take Option C for the contract and event model, shaped for hub and worker. Put adapters on the worker, the event store and MCP-style tool server on the hub.
2. Vendor `effect-acp` (or a slimmer ACP client) as the first transport, since ACP covers the most harnesses per line of code. Pi has an ACP route too (a T3 comment names `pi-acp`), but T3 itself drives Pi through `--mode rpc`.
3. Port Claude and Codex adapters by reading `ClaudeAdapterV2` and `CodexAdapterV2`, not by copying them.
4. Copy the shadow-home and per-instance env logic nearly verbatim (small, self-contained, MIT).

## Not checked (limits of this study)

- Bodies of `ProviderInstanceRegistry*.ts`, relay and peer layers, and web client settings forms.
- Exact choices behind Codex `serviceTier` ("fast mode").
- License terms of third-party SDKs.
- Whether the `orchestration-v2` event store could be dropped in as a library (88k lines, tied to SQLite persistence).
