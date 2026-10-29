# Driving many AI coding-agent harnesses from Rust

Research date: 2026-10-28. Read-only research. No login, no credential files read, no prompt sent to a model.

Labels: **FACT** = read in a primary source on 2026-10-28 (URL given). **INFERENCE** = my conclusion from facts. **UNVERIFIED** = could not confirm; test before you rely on it.

Local checks (help output and schema generation only): `claude` 2.1.312, `codex-cli` 0.165.0, `pi` 1.2.0, `opencode` 2.2.0, `gemini` 0.46.0.

---

## 1. ACP (Agent Client Protocol)

### 1.1 Version and status

- FACT: Stable protocol is **v1**. A **v2 draft** was published on 2026-07-20. v2 schemas are tagged `schema-v2.0.0-alpha.N`. The docs say v1 stays supported and v1-only peers "will remain common for some time". Do not ship v2 by default.
  - https://agentclientprotocol.com/announcements/acp-v2-draft.md
  - https://agentclientprotocol.com/protocol/v2/overview.md
- FACT: Spec repo `agentclientprotocol/agent-client-protocol`, Apache-2.0, 4.5k stars. JSON Schema in `schema/v1` and `schema/v2`. https://github.com/agentclientprotocol/agent-client-protocol
- FACT: Official SDKs: Rust (`agent-client-protocol`), TypeScript (`@agentclientprotocol/sdk`), Python, Kotlin, Java. https://agentclientprotocol.com/libraries/rust.md

### 1.2 Methods and features (v1)

FACT, from https://agentclientprotocol.com/protocol/v1/overview.md and the pages listed in https://agentclientprotocol.com/llms.txt:

| Area | Methods / notifications |
|---|---|
| Agent methods (baseline) | `initialize`, `authenticate`, `session/new`, `session/prompt` |
| Agent methods (optional) | `session/load` (needs `loadSession`), `session/resume` (needs `sessionCapabilities.resume`), `session/list`, `session/close`, `session/delete`, `logout`, `session/set_mode`, `session/set_config_option` |
| Cancel | `session/cancel` (notification). Pending permission requests must be answered `cancelled`. |
| Client methods | `session/request_permission` (baseline), `fs/read_text_file`, `fs/write_text_file`, `terminal/create`, `terminal/output`, `terminal/release`, `terminal/wait_for_exit`, `terminal/kill`, `elicitation/create` |
| Notifications | `session/update` (message chunks, thought chunks, tool calls and updates, plan, mode and config updates, available commands, usage), `elicitation/complete` |

Details:

- **MCP injection.** `session/new` takes `cwd` and `mcpServers`. `stdio` is required for all agents. `http` (fields `url`, `headers` as name/value list) and `sse` are optional. They are allowed only when the agent sets `agentCapabilities.mcpCapabilities.http` or `.sse`. So a bearer token goes in `headers: [{name:"Authorization", value:"Bearer ..."}]`. https://agentclientprotocol.com/protocol/v1/session-setup.md
- **Load vs resume.** `session/load` replays the full history as `session/update` notifications, then answers. `session/resume` does not replay. It restores context and reconnects MCP servers. Both take `sessionId`, `cwd`, `mcpServers`. Optional `additionalDirectories` exists on new, load and resume. Same page.
- **Config options (model, effort, mode).** `session/set_config_option {sessionId, configId, value}`. The agent returns the full option list. Option types: `select`, and `boolean` (the client must opt in). Standard categories: `mode`, `model`, `model_config`, `thought_level`. The agent can push `config_option_update`. Config options "supersede" Session Modes, and modes "will be removed in a future version". https://agentclientprotocol.com/protocol/v1/session-config-options.md
- **Usage and cost.** `usage_update` session notification is **stable** since 2026-06-05. It carries context-window use and size, and an optional cumulative cost. https://agentclientprotocol.com/announcements/session-usage-stabilized.md
- **Per-turn token usage.** Still a **Draft RFD**. It would add an optional `usage` object to `PromptResponse` (`totalTokens`, `inputTokens`, `outputTokens`, optional `thoughtTokens`, `cachedReadTokens`, `cachedWriteTokens`). Rust feature flag would be `unstable_end_turn_token_usage`. https://agentclientprotocol.com/rfds/end-turn-token-usage.md
- **Plans, tool calls, slash commands, terminals, fs.** All have v1 pages (agent-plan, tool-calls, slash-commands, terminals, file-system). Tool call names are stabilized.
- **Elicitation (structured user questions).** Stabilized. https://agentclientprotocol.com/announcements/elicitation-stabilized.md
- **Steer / queue while a turn runs.** FACT: not in the v1 spec. v2 draft changes the prompt lifecycle: "`session/update` notifications can proceed freely at any point in the session" and agents can signal idle. INFERENCE: v2 is the standard place where queueing will land. Today it is an extension (see 2.1).
- **Fork, subagents, proxy chains, MCP-over-ACP, client system prompt, custom LLM endpoint:** RFDs, not stable. https://agentclientprotocol.com/rfds/session-fork.md, https://agentclientprotocol.com/rfds/custom-llm-endpoint.md

### 1.3 Remote (non-stdio) transport

- FACT: Only **stdio** is standard. Messages are newline-delimited JSON. Agents must not write non-ACP output to stdout. https://agentclientprotocol.com/protocol/v1/transports.md
- FACT: "Streamable HTTP & WebSocket Transport" RFD moved to **Active** on 2026-07-02 (a Transports Working Group exists). Design: single `/acp` endpoint, HTTP/2, POST for client messages, GET SSE for server messages, or a WebSocket upgrade. Headers `Acp-Connection-Id` and `Acp-Session-Id`. Resumability deferred to v2. Reference implementation "in progress". https://agentclientprotocol.com/rfds/streamable-http-websocket-transport.md
- INFERENCE: For our hub-to-worker link, do not wait for ACP remote transport. The worker owns the stdio child process. The hub talks to the worker with our own protocol.

### 1.4 Official Rust crate

- FACT: `agent-client-protocol` **3.6.0** (2026-10-28), Apache-2.0, 5.7M downloads, repo https://github.com/agentclientprotocol/rust-sdk (230 stars). https://crates.io/crates/agent-client-protocol , https://docs.rs/agent-client-protocol/latest/agent_client_protocol/
- FACT, churn: 2.2.0 on 2026-09-18, 3.0.0 on 2026-10-06, then 3.1 to 3.6 in three weeks. Pin an exact version.
- FACT, workspace crates: `agent-client-protocol`, `-http`, `-rmcp`, `-derive`, `-conductor`, `-polyfill`, `-trace-viewer`, `-cookbook`, `-test`, `yopo`. The schema crate is `agent-client-protocol-schema`.
- FACT, feature flags (none by default): `process` (gives `AcpAgent`, `AcpAgentConfig`: spawn an agent subprocess), `stdio`, `schemars`, `unstable_protocol_v2`, `unstable_session_fork` and other `unstable_*`.
- FACT, client API shape. `Client.builder()` then `.on_receive_notification(...)` and `.on_receive_request(...)` handlers, then `.connect_with(agent_transport, |connection| async { ... })`. Inside: `connection.send_request(InitializeRequest::new(ProtocolVersion::V1)).block_task().await`, `NewSessionRequest::new(cwd)`, `PromptRequest::new(session_id, vec![ContentBlock::Text(..)])`. Permission requests arrive as `RequestPermissionRequest`; answer with `RequestPermissionResponse` (`Selected(option_id)` or `Cancelled`). Higher level: `cx.build_session_cwd()` gives `SessionBuilder` and `ActiveSession`. `RestoreSessionBuilder` restores a session. `mcp_server` module can host runtime-agnostic MCP servers attached per session.
  - Example: https://github.com/agentclientprotocol/rust-sdk/blob/main/src/agent-client-protocol/examples/yolo_one_shot_client.rs
  - Cookbook crate: `agent_client_protocol_cookbook` (docs.rs).
- UNVERIFIED: how to send a custom extension request (for example `_session/steering`) through the typed client API. Check the `extensibility` docs and the crate before you plan on it.

---

## 2. ACP adapters and native ACP support

### 2.1 The ACP Registry

- FACT: https://agentclientprotocol.com/get-started/registry.md . Registry JSON: `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json` (top-level `version: "1.0.0"`, `extensions: []`). The registry is "stabilized" (https://agentclientprotocol.com/announcements/acp-agent-registry-stabilized.md). It lists agents that support authentication. 43 agents were listed today.
- FACT, entry schema: `id`, `name`, `version`, `description`, `repository`, `website`, `authors`, `license`, `icon`, and `distribution` with one or more of:
  - `npx`: `{package, args?, env?}` (version pinned in the package spec)
  - `uvx`: `{package, args?, env?}`
  - `binary`: keyed by `darwin-aarch64`, `darwin-x86_64`, `linux-aarch64`, `linux-x86_64`, `windows-aarch64`, `windows-x86_64`. Each value: `{archive (URL), cmd, args?, env?, sha256?}`.
- INFERENCE: Our worker can read this JSON, pick the entry, and install or launch it. It must check `sha256` where present. Cursor entries have no `sha256`.

### 2.2 Status by harness (all FACT unless marked)

| Harness | ACP route | Version today | Notes |
|---|---|---|---|
| Claude Code | `@agentclientprotocol/claude-agent-acp` (registry id `claude-acp`) | 0.99.0 (npm, 2026-10-28), Apache-2.0 | See 2.3 |
| Codex | `@agentclientprotocol/codex-acp` (id `codex-acp`) | 2.3.0 (npm), Apache-2.0 | See 2.4 |
| OpenCode | Native: `opencode acp` | 2.2.0 | Registry has two entries, "OpenCode" and "OpenCode V2", both 2.2.0, binary distribution from `anomalyco/opencode` releases. Local help: `opencode acp [--login]`. |
| Pi | Third party: `pi-acp` (https://github.com/svkozak/pi-acp) | 0.0.34 (2026-09-24), MIT, 750 stars | Spawns `pi --mode rpc`. See 2.5 |
| Gemini CLI | Native: `gemini --acp` | registry 0.66.0 (`--experimental-acp` is deprecated; local 0.46.0 has `--acp`) | npx `@google/gemini-cli` args `["--acp"]` |
| Goose | Native: `goose acp` | 1.57.0 | binary |
| Cursor | Native: `cursor-agent acp` | 2026.10.22 | binary, no sha256 |
| Grok | Native: `@xai-official/grok agent stdio` (id `grok-build`) | 1.0.73 | npx |
| Kimi CLI | Native: `kimi acp` | 1.52.0 | binary (no macOS x86_64 build) |
| Others in registry | Amp, Auggie, Cline, Copilot, Junie, Kilo, Mistral Vibe, Qwen Code, Factory Droid, Antigravity, Devin, and more | | Source: registry JSON |

### 2.3 Claude: `claude-agent-acp`

- FACT: Repo https://github.com/agentclientprotocol/claude-agent-acp , 2.7k stars, Apache-2.0. The README calls it an "ACP adapter for the Claude Agent SDK". npm dependencies: `@anthropic-ai/claude-agent-sdk` **0.3.312**, `@agentclientprotocol/sdk` 2.1.0. Needs Node >= 22. https://www.npmjs.com/package/@agentclientprotocol/claude-agent-acp
- FACT: Agent SDK 0.3.312 maps to Claude Code 2.1.312 and ships per-platform native binaries as optional dependencies (`@anthropic-ai/claude-agent-sdk-darwin-x64`, `-linux-x64`, `-linux-arm64`, `-win32-x64`, and others). So the adapter brings its own Claude Code binary. UNVERIFIED: whether it can use the user's installed `claude` instead, and how login is shared.
- FACT, mapped features (README): permission requests (with a permission-presentation extension), config options (mode, model, effort), client MCP servers (`/mcp` to reconnect, OAuth), interactive and background terminals, slash commands, images, @-mentions, plans/TODO lists, subagent transcripts, session goals. Protocol switch `CLAUDE_AGENT_ACP_PROTOCOL=v1|v2|auto` (default `auto`, serves both). On v2 the baseline includes `session/list`, `session/resume` (with replay) and `session/close`. https://raw.githubusercontent.com/agentclientprotocol/claude-agent-acp/main/README.md
- FACT, steering: the agent advertises `_meta.steering = {supported: true}` in `initialize` and then accepts `_session/steering` for a running turn. It is a JetBrains-defined key, outside the ACP spec. The README says steering is "v1-only for now". `claudeCode.promptQueueing` exists for non-JetBrains clients. https://raw.githubusercontent.com/agentclientprotocol/claude-agent-acp/main/docs/air-extensions.md
- FACT: Other JetBrains "AIR" extensions (diff patch, async tasks, session index, recommended model/effort values) turn on only if the client sends `_meta.jetbrains.air` in `initialize`. We can ignore them.
- FACT: Zed's docs say Claude Agent uses `/login` to pick an API key or Claude Code. https://zed.dev/docs/ai/external-agents
- UNVERIFIED: README is silent on `usage_update`, on how `AskUserQuestion` maps (permission option list or elicitation), and on `plan` permission mode.
- INFERENCE: Older name was `claude-code-acp` under `zed-industries` (your note and my memory). The README does not mention it.

### 2.4 Codex: `codex-acp`

- FACT: Repo https://github.com/agentclientprotocol/codex-acp , 496 stars, Apache-2.0, TypeScript on Node. It "starts the Codex App Server, translates ACP requests into Codex operations, and maps Codex events back". npm dependency `@openai/codex ^0.164.0`; `CODEX_PATH` can point at another binary. `@agentclientprotocol/sdk ~2.0.2`.
- FACT, features: config options (model, reasoning effort, fast mode, approval, sandbox), initial mode (`read-only`, `workspace-write`, `agent`, `agent-full-access`), client MCP servers over stdio and HTTP, permission requests, token usage events, slash commands (`/status`, `/mcp`, `/review`, `/compact`, `/logout`, `/goal`, skills).
- FACT, auth: advertises ACP auth methods: ChatGPT login (hidden by `NO_BROWSER=1`), API key from `CODEX_API_KEY` or `OPENAI_API_KEY`, custom gateway.
- INFERENCE: codex-acp is itself a client of `codex app-server`. Our Rust worker can be the same client without Node.

### 2.5 Pi: `pi-acp`

- FACT: https://github.com/svkozak/pi-acp (third party, MIT). Needs Node 22+ and pi >= 0.81. Supports `session/load` (maps to Pi session files), tool call updates with diffs, `usage_update` from `get_session_stats`, slash commands, `/thinking` mapped to modes.
- FACT, limits (README): no `fs/*` or `terminal/*` delegation. **MCP servers are accepted and stored but not passed to Pi.** No separate thought stream. Permission handling is not described.
- INFERENCE: For our needs (MCP injection, approvals), pi-acp is weak. See section 5 for the native route.

---

## 3. Codex native: `codex app-server`

### 3.1 Protocol and tooling

- FACT: `codex app-server` is marked **[experimental]** in `--help`. Transports via `--listen`: `stdio://` (default, JSONL), `unix://`, `ws://IP:PORT` (docs call ws "experimental and unsupported"), `off`. Subcommands: `daemon`, `proxy`, `generate-ts`, `generate-json-schema`. There is also `codex remote-control` and `codex agents` ("shared local app-server daemon"). Local `codex app-server --help`; docs https://learn.chatgpt.com/docs/app-server (redirect from developers.openai.com/codex/app-server).
- FACT: JSON-RPC 2.0 with the `"jsonrpc":"2.0"` field omitted on the wire. Handshake: `initialize {clientInfo:{name,title,version}, capabilities?}` then an `initialized` notification. Capabilities include `experimentalApi` and `optOutNotificationMethods`. Anything marked experimental needs `experimentalApi: true`.
- FACT: Schema commands exist and work offline: `codex app-server generate-ts --out DIR` and `codex app-server generate-json-schema --out DIR`. Output matches the Codex version that ran it. I ran `generate-json-schema` locally (0.165.0). Files include `ClientRequest.json`, `ServerRequest.json`, `ServerNotification.json`, `codex_app_server_protocol.v2.schemas.json`.
- `-c key=value` config overrides work on `app-server` (TOML value). FACT from local help.

### 3.2 Rust protocol crate

- FACT: The crate is `codex-app-server-protocol` (lib `codex_app_server_protocol`) at `openai/codex/codex-rs/app-server-protocol`. Repo license Apache-2.0, 131k stars. https://github.com/openai/codex/tree/main/codex-rs
- FACT: It is **not on crates.io**. A crates.io search found no such crate. Its `Cargo.toml` depends on many workspace crates (`codex-protocol`, `codex-rollout`, `codex-secrets`, `codex-shell-command`, `codex-history`, others) and on `rmcp`. Other in-tree crates: `app-server-client`, `app-server-transport`, `app-server-daemon`, `app-server-test-client`.
- INFERENCE: A git dependency is possible but pulls a large workspace and must pin a rev that matches the installed `codex`. A lighter option: generate the JSON Schema in CI from the pinned `codex` version, write serde types for the ~15 methods we use (or run `typify`), and test them against the full schema. Community crate `codex-codes` exists in `meawoppl/rust-code-agent-sdks` (not audited).

### 3.3 Methods we need (FACT, from the locally generated 0.165.0 schema)

- Client requests: 106 methods. Relevant:
  - Thread: `thread/start`, `thread/resume`, `thread/fork`, `thread/list`, `thread/read`, `thread/archive`, `thread/name/set`, `thread/compact/start`, `thread/revert`
  - Turn: `turn/start`, `turn/steer`, `turn/interrupt`
  - Models and config: `model/list`, `config/read`, `config/mcpServer/reload`, `mcpServerStatus/list`, `mcpServer/oauth/login`
  - Account: `account/read`, `account/login/start`, `account/login/cancel`, `account/logout`, `account/rateLimits/read`
- `thread/start` params: `model`, `cwd`, `approvalPolicy`, `sandbox`, **`config`** (free-form object or null: config overrides), `baseInstructions`, `developerInstructions`, `ephemeral`, `personality`, `serviceTier`. `thread/resume` takes `threadId` plus the same overrides and `excludeTurns`.
- `turn/start` params: `threadId`, `input`, `model`, **`effort`**, `summary`, `cwd`, `approvalPolicy`, `sandboxPolicy`, `outputSchema`, `clientUserMessageId`. `turn/steer`: `threadId`, `input`, `expectedTurnId`. `turn/interrupt`: `threadId`, `turnId`. (Docs: steer appends to the active turn and does not emit a new `turn/started`.)
- Server requests (our UI must answer): `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/permissions/requestApproval`, **`item/tool/requestUserInput`**, `item/tool/call` (dynamic tools), `mcpServer/elicitation/request`, `account/chatgptAuthTokens/refresh`, `attestation/generate`, and legacy `applyPatchApproval`, `execCommandApproval`.
- Notifications we need: `turn/started`, `turn/completed`, `item/started`, `item/completed`, `item/agentMessage/delta`, `item/plan/delta`, `turn/plan/updated`, `thread/tokenUsage/updated` (`tokenUsage: {last, total, modelContextWindow}`), `account/rateLimits/updated`, `account/updated`, `account/login/completed`, `thread/status/changed`, `thread/queue/changed`, `serverRequest/resolved`, `mcpServer/startupStatus/updated`.
- Model selection: `model/list` returns `id`, `displayName`, `supportedReasoningEfforts`, `isDefault` (docs).

### 3.4 MCP injection

- INFERENCE (test it): `thread/start` `config` mirrors `config.toml` keys, so `{"mcp_servers": {"hub": {"url": "https://...", ...}}}` should work. The CLI equivalent is `codex app-server -c 'mcp_servers.hub.url="..."'`. The README only documents `mcpServerStatus/list` with `threadId` and `config/mcpServer/reload`. It does not document an `mcp_servers` param. Bearer-token field names for HTTP MCP in Codex config are UNVERIFIED (check `config.toml` docs).

### 3.5 Auth

- FACT: Codex owns login. `account/login/start`, `account/read`, `codex login` all stay inside Codex. We never hold the ChatGPT token. Do not implement `account/chatgptAuthTokens/refresh` (INFERENCE: that is for clients that supply their own tokens).

### 3.6 Stability notes

- FACT: Marked experimental. The README is changelog-style and shows removals (for example `thread/rollback` was removed in favor of `thread/revert`). Surface is large and moves every release. Many fields need `experimentalApi`.
- INFERENCE: Pin the Codex version range, regenerate and diff the schema in CI, and use `optOutNotificationMethods` to cut noise.

---

## 4. Claude native from Rust

### 4.1 What is documented (FACT)

- Headless docs (https://code.claude.com/docs/en/headless) and CLI reference (https://code.claude.com/docs/en/cli-reference.md) document these flags: `-p`, `--output-format text|json|stream-json`, `--input-format text|stream-json`, `--include-partial-messages`, `--replay-user-messages`, `--verbose`, `--resume <id|path>`, `--continue`, `--session-id <uuid>`, `--fork-session`, `--no-session-persistence`, `--mcp-config <files|json>`, `--strict-mcp-config`, `--permission-mode` (`auto`, `dontAsk`, `acceptEdits`, others), `--allowedTools`, `--permission-prompt-tool mcp__<server>__<tool>`, `--permission-prompts host|none`, `--model`, `--effort low|medium|high|xhigh|max|ultracode`, `--max-turns`, `--bare`, `--append-system-prompt`.
- Output events documented: `system/init` (model, tools, `mcp_servers`, `mcp_server_errors`, plugins, and a `capabilities` array such as `interrupt_receipt_v1`, `interrupt_cancel_queued_v1` for feature detection), `system/api_retry`, `stream_event` deltas, `assistant` and `user` messages with `parent_tool_use_id` for subagents, `result` (final text, session id, `total_cost_usd`, per-model cost, `permission_denials`).
- With `--input-format stream-json`, messages queue while a turn runs (CLI reference for `--max-turns`: "a message still queued ... stays queued and starts a new turn"). SIGINT or SDK `interrupt()` ends a turn; SIGTERM does not.
- `--permission-prompt-tool`: Claude calls an MCP tool on a server it loaded to decide each permission prompt. So **our own HTTP MCP server can answer permission prompts** without the control protocol. An `allow` result for an MCP tool that "requires user interaction" is turned into a deny (v2.1.199+).
- `--bare` does not read OAuth or keychain credentials (API key only). Do not use `--bare` with a subscription login. https://code.claude.com/docs/en/headless#start-faster-with-bare-mode
- Docs point non-Python/TS users to the CLI: "To drive the same agent loop from a language other than Python or TypeScript, run the CLI as a subprocess with the `-p` flag." https://code.claude.com/docs/en/agent-sdk/overview.md

### 4.2 What is not documented

- FACT: The control protocol (`control_request` / `control_response`, `can_use_tool`, hook callbacks, `interrupt`, `set_model`, `set_permission_mode`, MCP message passing) has no stand-alone wire spec in the headless or CLI pages. The user-input page explains `canUseTool` and `AskUserQuestion` only as SDK API (https://code.claude.com/docs/en/agent-sdk/user-input.md). The TypeScript reference contains the string `control_request` four times (context not checked).
- INFERENCE: The TypeScript SDK is the de facto spec. A Rust client must follow a protocol that Anthropic can change in any CLI release.

### 4.3 Community Rust crates (crates.io and GitHub, 2026-10-28)

| Crate | Version / last release | Signals | Notes |
|---|---|---|---|
| `claude-codes` (https://github.com/meawoppl/rust-code-agent-sdks) | **2.1.312**, 2026-10-28 | 18.6k downloads, 27 stars, Apache-2.0, 555 commits | Version equals the Claude CLI version its live test suite passed on. Nightly wire-drift checks. Sync and Tokio clients. Types: `ClaudeInput`, `ClaudeOutput`, `ControlRequest`, `ControlResponse`, `ToolPermissionRequest`, `PermissionResult`, `HookCallbackRequest`, `McpMessageRequest`, `InitializeRequest`, `ClaudeCliBuilder`. Docs 63% covered, examples untested. States "The Claude CLI protocol is unstable and evolving." Same repo has `codex-codes`, `opencode-codes`, `pi-codes`, `antigravity-codes`, `muse-codes`. |
| `claude-agent-sdk-rs` (tyrchen) | 0.6.4, 2026-02-09 | 31k downloads, 78 stars | "bidirectional streaming, hooks, custom tools". Stale for 8 months. Not audited. |
| `anthropic-agent-sdk` (bartolli) | 0.2.75, 2025-12-23 | 86k downloads, 10 stars | Hooks, permissions, MCP. Last push 2026-04. Not audited. |
| `cc-sdk` | 0.8.1, 2026-04-03 | 10k downloads | Not audited. |
| `claude-code-agent-sdk` | 0.1.39, 2026-02-13 | 11k downloads | Not audited. |
| `claude-code-sdk-rust` | 0.4.1, 2026-07-21 | 4.7k downloads | Not audited. |

- FACT: No official Anthropic Rust SDK found. `github.com/anthropics/claude-agent-sdk-rust` returns 404 even though the `claude-agent-sdk` 0.1.1 crate lists that URL. INFERENCE: treat that crate as unofficial.
- INFERENCE: `claude-codes` is the only crate that tracks the current CLI. I did not read its source. Test it against `can_use_tool`, interrupt and queue before you depend on it.

### 4.4 Terms (FACT)

- "The Claude Code binary must not be modified. Claude Code must be installed and run as published by Anthropic, and customers may not remove, disable, or restrict any authentication method built into it." https://code.claude.com/docs/en/legal-and-compliance.md
- Each end user must authenticate with their own Anthropic API key, Claude subscription credentials, or a 3P inference credential. Customers may not pay for or resell usage for end users. Same page.
- "OAuth authentication is intended exclusively for purchasers of Claude Free, Pro, Max, Team, and Enterprise subscription plans and is designed to support ordinary use of Claude Code and other native Anthropic applications." Developers building products with the Agent SDK should use API keys. Anthropic does not permit third-party developers to offer claude.ai login in their apps. Same page. Also: "Unless previously approved, Anthropic does not allow third party developers to offer claude.ai login or rate limits for their products, including agents built on the Claude Agent SDK." https://code.claude.com/docs/en/agent-sdk/overview.md
- Branding: do not name an agent "Claude Code" or "Claude Code Agent". Use "Claude Agent" or "Claude". Same overview page.
- INFERENCE: Our design (user's own Mac runs the unmodified binary, user logs in with `claude` on that machine, app never sees tokens) matches the stated rules. The hub driving it remotely is a grey area. The legal page itself says to contact sales for questions about permitted authentication. Ask before launch.

### 4.5 Trade-off: native stream-json vs TypeScript SDK through ACP

| | Native (`claude` + stream-json, Rust) | ACP adapter (`claude-agent-acp`) |
|---|---|---|
| Runtime on worker | Only the user's `claude` binary | Node >= 22 plus a bundled Claude Code binary from the SDK package |
| Binary used | The user's own installed `claude` (clean story for the "unmodified binary" rule) | The SDK-bundled binary (also published by Anthropic). UNVERIFIED if the user's `claude` can be selected |
| Maintenance | We follow an unstable, undocumented control protocol. Community crate helps | ACP org follows the SDK. We follow ACP v1 |
| Permissions | `--permission-prompt-tool` to our MCP server (documented), or `can_use_tool` control requests | `session/request_permission` (standard) |
| Steer / queue | Stream-json input queues; `interrupt_cancel_queued_v1` capability | Non-standard `_session/steering` extension (v1 only) |
| Model / effort | `--model`, `--effort`, control `set_model` | `session/set_config_option` (model, effort, mode) |
| Usage | Full `result` with usage and `total_cost_usd` | `usage_update` (context and cost) only. Per-turn tokens not stable |
| Resume | `--resume <id>`, `--fork-session`, session files on disk | `session/load` or `session/resume` |
| MCP http | `--mcp-config` with `type: "http"` (entries without `type` are skipped; check `mcp_server_errors`) | `session/new` `mcpServers` with `http` capability |
| Hooks, subagent detail, plan mode, `AskUserQuestion` | Full | Partly mapped. Check before relying on it |

---

## 5. Pi and OpenCode from Rust

### 5.1 Pi `--mode rpc`

- FACT: Docs ship inside the install: `/opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent/docs/{rpc.md, rpc-commands.md, rpc-extension-ui.md, json.md, mcp.md, security.md}` (package `@earendil-works/pi-coding-agent` 1.2.0, MIT, repo `earendil-works/pi`). RPC is documented as the route for "language-independent integrations".
- FACT, framing: strict JSONL, one JSON object per LF-terminated line. Do not split on U+2028 or U+2029. Stdout is protocol only; logs go to stderr. Close stdin to shut down. Every command may carry an `id`; the `response` repeats it.
- FACT, commands: `prompt`, `steer`, `follow_up`, `abort`, `clear_queue`, `new_session`, `get_state`, `get_messages`, `set_model`, `cycle_model`, `get_available_models`, `set_thinking_level`, `get_available_thinking_levels`, `set_steering_mode`, `set_follow_up_mode`, `compact`, `bash`, `get_session_stats`, `switch_session`, `fork`, `clone`, `get_entries`, `get_tree`, `set_session_name`, `get_commands`.
- FACT, steer and queue: while streaming, `prompt` needs `streamingBehavior` `"steer"` (delivered after the current tool calls, before the next LLM call) or `"followUp"` (after the agent stops). `data.disposition` is `started`, `queued` or `handled`. Wait for `agent_settled`, not `agent_end`, to know the run is over.
- FACT, usage: `message_update` events carry cumulative `usage` (`input`, `output`, `cacheRead`, `cacheWrite`, `totalTokens`, `cost{...}`).
- FACT, resume: CLI flags `--session <path|id>`, `--session-id <id>` (create if missing), `--continue`, `--resume`, `--fork`, `--session-dir`, `--no-session`; RPC `switch_session`.
- FACT, model and effort: `--provider`, `--model provider/id[:thinking]`, `--thinking off|minimal|low|medium|high|xhigh|max`. Providers such as Fireworks are Pi's concern.
- FACT, MCP: Pi has built-in MCP (stdio and streamable HTTP) with `pi mcp add`, `--bearer-token-env-var`, config in `~/.pi/mcp.json` and `.pi/mcp.json` (project file needs trust). Local help lists `--mcp-config <value>  Path to MCP config file` and `--no-mcp`. INFERENCE: Write a temp `mcp.json` per session and pass `--mcp-config`.
- FACT, permissions: Pi "does not ask for approval before every tool call". Approval UI exists only through extensions that call `ctx.ui` (`select`, `confirm`, `input`, `editor`), which appear as `extension_ui_request` on stdout and are answered with `extension_ui_response` on stdin. INFERENCE: To get approvals through our UI we must ship a small Pi extension (a permission gate) and load it with `-e`. The Pi docs mention `/plan` and `--plan` from a plan-mode extension.
- Rust: `pi-codes` exists in `meawoppl/rust-code-agent-sdks` (not audited). The wire format is simple enough to write by hand.

### 5.2 OpenCode

- FACT (local): `opencode` v2.2.0. `opencode serve` = "Start the v2 API and web server" with `--hostname`, `--port`, `--cors`, `--service`, `--stdio`. `opencode service` manages a background server; `--server <url>` connects a client to a server; `opencode api <operation | method path>` accepts an **OpenAPI operation ID**; `opencode run`, `opencode session`, `opencode models`, `opencode mcp`, `opencode pair`, `opencode acp`. Global flag `--auto` auto-approves permissions that are not explicitly denied.
- FACT (v1 docs, https://opencode.ai/docs/server/): OpenAPI 3.1 spec at `/doc`; basic auth with `OPENCODE_SERVER_PASSWORD` (user `opencode`); `POST /session`, `POST /session/:id/message` (waits), `POST /session/:id/prompt_async` (204), `POST /session/:id/abort`, `POST /session/:id/permissions/:permissionID {response, remember}`, `GET /event` (SSE, first event `server.connected`), `GET /config/providers`, `GET /mcp`, `POST /mcp {name, config}` (add MCP dynamically), `GET/PATCH /config`. Default `127.0.0.1:4096`.
- UNVERIFIED: whether these v1 paths still hold in v2. The v2 docs page (https://opencode.ai/v2/docs) did not describe the server API, and `/v2/docs/server` returned 404. The local help calls the server "v2 API" and refers to OpenAPI operation IDs, so a spec exists. Next step: run `opencode serve` on a free port and read its spec (not done here, because it starts a server).
- INFERENCE: For day one, use `opencode acp` (first-party, in the registry). Later, add the HTTP/SSE API if we need richer events, and generate a Rust client from the pinned version's OpenAPI spec.

---

## 6. MCP in Rust: `rmcp`

- FACT: `rmcp` **3.8.0** (2026-10-26), 41.7M downloads, repo https://github.com/modelcontextprotocol/rust-sdk (official). Release cadence: 3.4.1 (09-23), 3.5.0 (09-28), 3.5.1 (10-05), 3.6.0 (10-20), 3.7.0 (10-22), 3.8.0 (10-26). There is a 3.x migration guide. https://crates.io/crates/rmcp
- FACT: Implements the stable MCP spec **2026-07-28**; stays compatible with 2025-11-25 and earlier. It does **not** provide the legacy 2024-11-05 HTTP+SSE transport. https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/main/README.md
- FACT, features: `server`, `transport-io`, `transport-child-process`, `transport-streamable-http-server` (a Tower service), `transport-streamable-http-client-reqwest`, `transport-worker`, `client-side-sse`, `request-state`, `base64`. Macros: `#[tool]`, `#[tool_router]`, `#[tool_handler]`, `#[prompt]` and friends.
- FACT, hosting: `StreamableHttpService::new(factory, LocalSessionManager::default().into(), StreamableHttpServerConfig::default().with_legacy_session_mode(false).with_json_response(true))`, mounted with `axum::Router::new().nest_service("/mcp", service)`. In stateless mode the factory runs per request. Example: `examples/servers/src/counter_streamhttp.rs`.
- FACT, auth: README links `docs/OAUTH_SUPPORT.md` only. It shows no bearer-token example.
- INFERENCE: Bearer auth = a Tower/axum middleware layer in front of `nest_service` that checks `Authorization: Bearer <per-session token>` and maps the token to a session or task id. Because the service runs per request in stateless mode, identity must come from the token, not from MCP session state.
- INFERENCE, maturity: official, heavily used, but a major-version-3 API with weekly releases. Pin exactly. Also the official ACP crate has `agent-client-protocol-rmcp`, which shows the two projects work together.
- Client compatibility to test: Claude `--mcp-config` needs `"type": "http"`; Codex needs its HTTP MCP config keys; ACP needs the agent's `mcpCapabilities.http`; Pi needs `--mcp-config`.

---

## 7. Zed as a reference

- FACT, license: Zed repo `zed-industries/zed`, 92k stars. GitHub reports the root license as `NOASSERTION` (mixed per crate). Per-crate files: `crates/agent_servers`, `crates/acp_thread`, `crates/acp_tools`, `crates/agent_ui`, `crates/agent` each have **`LICENSE-GPL`**; `agent_servers/Cargo.toml` says `license = "GPL-3.0-or-later"`. `crates/gpui` has **`LICENSE-APACHE`**. https://github.com/zed-industries/zed
- INFERENCE: Zed's agent crates are GPL-3.0-or-later. Read them for design ideas. Do not copy code unless our app is GPL-3.0-or-later too. `gpui` (Apache-2.0) is safe to depend on. Re-implement from the spec and the Apache-2.0 ACP SDK.
- FACT, structure: `crates/agent_servers/src/` contains `agent_servers.rs`, `acp.rs`, `acp/`, `custom.rs`, `e2e_tests.rs`. `acp_thread` holds the thread model that UI renders. `acp_tools` is the ACP log viewer (docs: `dev: open acp logs`). INFERENCE from names: `agent_servers` = process launch and ACP connection per agent; `acp_thread` = conversation state; `agent_ui` = panel.
- FACT, behavior (https://zed.dev/docs/ai/external-agents): Zed hosts the thread; each agent runs as its own process and owns its runtime, auth, model choice and tools. Install from the ACP Registry (`zed: acp registry`, or Agent Settings > External Agents). Custom agents go under `agent_servers` in settings with `type`, `command`, `args`, `env`. Zed MCP servers "may be forwarded over ACP". Auth is per agent (a Zed Anthropic key does not configure Claude Agent). Agent threads can be imported into history. Docs do not say how adapters are downloaded; the registry's `npx`/`uvx`/`binary` entries suggest the installer resolves those (INFERENCE).
- Takeaway: the same model fits us. One generic ACP connection type, per-agent launch config from the registry, per-agent auth owned by the agent, a thread model separate from the wire protocol.

---

## Recommendation

**Use ACP as the common spine, and write native adapters only where the feature loss hurts.** Put one internal `HarnessSession` trait in front of everything. Model it on ACP's shape plus two extras ACP lacks today: `steer/queue` and per-turn usage.

| Harness | Day-one path | Why | Features lost on this path |
|---|---|---|---|
| **Claude Code** | **ACP** via `claude-agent-acp` (pin version). Keep a native `claude -p` stream-json adapter (crate `claude-codes`, or our own thin types) as the planned second backend. | Fastest route to a working loop. ACP org keeps the SDK mapping current. We avoid an undocumented control protocol on day one. | Needs Node >= 22 and a bundled Claude binary on every worker. Steering depends on the non-standard `_session/steering` (v1 only). No stable per-turn token count (only `usage_update` for context and cost). `AskUserQuestion`, plan mode and hooks mapping not verified. Cannot choose the user's own `claude` binary (UNVERIFIED). Move to native if any of these block the coordinator bot. |
| **Codex** | **Native `codex app-server`** over stdio, with types generated from `generate-json-schema` for the pinned `codex` version. | First-party, typed, schema generator ships in the binary. The ACP adapter is itself just a Node wrapper over it. We use about 15 methods. Gives `turn/steer`, `thread/tokenUsage/updated`, per-turn `effort`, `item/tool/requestUserInput`, rate limits, account state, and the user's own `codex` binary. | We own protocol drift (marked experimental, large surface, regenerate and diff in CI). We write the mapping to our event model ourselves. `mcp_servers` via `thread/start.config` must be tested. |
| **OpenCode** | **ACP**: `opencode acp`. Add the HTTP/SSE API later if needed. | First-party ACP, in the registry. v2 HTTP API not yet verified. | No steer or queue in ACP. Per-turn usage not stable. Richer event stream and dynamic MCP add (`POST /mcp`) only through HTTP. |
| **Pi** | **Native `pi --mode rpc`**. | Documented JSONL protocol with `steer`, `follow_up`, `abort`, cumulative `usage`, `--session`, `--mcp-config`. `pi-acp` drops MCP servers, has no fs or terminal, and no thought stream. | Pi has no built-in approval prompts. We must ship a permission-gate extension that talks through `extension_ui_request`. `AskUserQuestion`-style questions only through extensions. |
| Gemini, Goose, Cursor, Grok, Kimi, others | **ACP** via registry entries. | Native ACP, no extra work beyond a launch recipe. | Whatever the agent leaves out of ACP. Test MCP `http` capability per agent before you promise tool injection. |

Why not ACP only: ACP v1 has no steer or queue and no stable per-turn usage. Both are core needs of a coordinator bot. The adapters add Node as a runtime dependency on every worker.

Why not native only: Claude's control protocol has no public spec, and each harness has a different wire format. ACP gives breadth for the cost of one client.

Build order (INFERENCE):

1. Worker process manager plus ACP client (`agent-client-protocol` 3.6.x, features `process`, pinned). Registry reader with sha256 checks. Test with Claude (`claude-agent-acp`) and `opencode acp`.
2. Our MCP server with `rmcp` 3.8.x, streamable HTTP, bearer-token layer, one token per session. Inject it per harness (see section 6 checklist).
3. Native Codex adapter on `app-server`. Schema diff in CI.
4. Native Pi adapter and permission-gate extension.
5. Optional native Claude adapter if the ACP path loses a feature you need.

Open questions to test first (all UNVERIFIED above):

- Does `claude-agent-acp` accept a custom `claude` binary, and does it share the user's Claude Code login?
- How does `claude-agent-acp` expose `AskUserQuestion` (permission options or `elicitation/create`)?
- Can the Rust ACP client send `_session/steering`?
- Does `thread/start` `config.mcp_servers` work for HTTP servers with a bearer token?
- What does the OpenCode v2 OpenAPI spec contain (permission reply, SSE events, MCP add)?
- Does Anthropic accept a hub that drives a user's logged-in `claude` on the user's machine? Ask Anthropic sales, as the legal page says.

## Source list

- ACP: https://agentclientprotocol.com/llms.txt , https://agentclientprotocol.com/protocol/v1/overview.md , https://agentclientprotocol.com/protocol/v1/session-setup.md , https://agentclientprotocol.com/protocol/v1/session-config-options.md , https://agentclientprotocol.com/protocol/v1/transports.md , https://agentclientprotocol.com/rfds/streamable-http-websocket-transport.md , https://agentclientprotocol.com/rfds/end-turn-token-usage.md , https://agentclientprotocol.com/announcements/acp-v2-draft.md , https://agentclientprotocol.com/get-started/registry.md , https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json
- Rust SDK: https://crates.io/crates/agent-client-protocol , https://docs.rs/agent-client-protocol/latest/agent_client_protocol/ , https://github.com/agentclientprotocol/rust-sdk
- Adapters: https://github.com/agentclientprotocol/claude-agent-acp , https://github.com/agentclientprotocol/codex-acp , https://github.com/svkozak/pi-acp , npm: `@agentclientprotocol/claude-agent-acp`, `@agentclientprotocol/codex-acp`, `pi-acp`
- Codex: https://learn.chatgpt.com/docs/app-server , https://github.com/openai/codex/tree/main/codex-rs/app-server-protocol , local `codex app-server --help` and `generate-json-schema` (0.165.0)
- Claude: https://code.claude.com/docs/en/headless , https://code.claude.com/docs/en/cli-reference.md , https://code.claude.com/docs/en/agent-sdk/overview.md , https://code.claude.com/docs/en/agent-sdk/user-input.md , https://code.claude.com/docs/en/legal-and-compliance.md , https://docs.rs/claude-codes/latest/claude_codes/ , https://github.com/meawoppl/rust-code-agent-sdks
- Pi: local docs in `@earendil-works/pi-coding-agent` 1.2.0 (`docs/rpc.md`, `rpc-commands.md`, `rpc-extension-ui.md`, `json.md`, `mcp.md`, `security.md`)
- OpenCode: https://opencode.ai/docs/server/ , https://opencode.ai/v2/docs , local `opencode --help`, `serve --help`, `api --help`, `acp --help` (2.2.0)
- MCP: https://crates.io/crates/rmcp , https://github.com/modelcontextprotocol/rust-sdk
- Zed: https://zed.dev/docs/ai/external-agents , https://github.com/zed-industries/zed (crate license files via GitHub API)
