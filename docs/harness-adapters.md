# Harness adapters

The worker runs every session through one Rust interface, so the hub never knows which harness is behind a session. This page explains that interface, how each harness plugs into it, and the order in which we build the adapters. The design copies ideas from T3 Code's provider layer but no code. Research notes are in [rust-harness-integration.md](../research/rust-harness-integration.md) and [t3-provider-layer.md](../research/t3-provider-layer.md).

## The interface

The `harness` crate has two traits. A `Harness` knows one kind of harness and makes sessions for a harness account. A `HarnessSession` is one live conversation.

```rust
#[async_trait]
pub trait Harness: Send + Sync {
	fn kind(&self) -> HarnessKind;

	/// Installed version, sign-in state, models with option descriptors, capabilities.
	async fn probe(&self, account: &AccountConfig) -> AccountStatus;

	/// Starts a new session, or resumes one when `spec.resume` is set.
	async fn open(
		&self,
		account: &AccountConfig,
		spec: SessionSpec,
	) -> Result<(Box<dyn HarnessSession>, mpsc::Receiver<SessionEvent>), HarnessError>;
}

#[async_trait]
pub trait HarnessSession: Send {
	fn capabilities(&self) -> &Capabilities;
	fn native_ref(&self) -> Option<NativeRef>;

	async fn turn(&mut self, input: TurnInput) -> Result<TurnId, HarnessError>;
	/// Returns `HarnessError::Unsupported` when `capabilities().steer` is false.
	async fn steer(&mut self, input: TurnInput) -> Result<(), HarnessError>;
	async fn interrupt(&mut self) -> Result<(), HarnessError>;
	async fn respond(&mut self, request: RequestId, answer: Answer) -> Result<(), HarnessError>;
	async fn close(self: Box<Self>) -> Result<(), HarnessError>;
}
```

`SessionSpec` holds the workspace path, the model and its options, the approval mode, the text to append to the system prompt, the MCP servers to give the session, extra environment variables, and an optional `NativeRef` to resume.

`SessionEvent` is the `proto` type from the [session events table](protocol.md#session-events). Each adapter translates its harness's events into that vocabulary. The worker numbers the events, writes them to the spool and sends them to the hub.

Four rules keep the adapters honest:

- An adapter reports a feature in `Capabilities` only if it has a test for that feature. The hub and the clients read `Capabilities` and hide what a session cannot do.
- Two sessions of the same account share no state inside the adapter. Two accounts of one harness never share a login.
- `turn` returns when the harness accepts the input, not when the turn ends. The end of a turn comes as a `turn.ended` event.
- If a harness asks for permission, the adapter emits `request.opened` and waits for `respond`. It never answers on its own, even in Full access. In Full access, the adapter configures the harness so that it does not ask.

## Capabilities

| Field | Meaning |
|---|---|
| `steer` | The session accepts new input while a turn runs |
| `resume` | The session can continue after the harness process restarts |
| `fork` | The session can branch from an earlier turn |
| `images` | Turn input can hold images |
| `plan_mode` | The harness has a plan-only mode |
| `usage` | The harness reports token counts |
| `mcp_http` | The harness accepts an HTTP MCP server with a bearer token |
| `approval_modes` | The approval modes that the adapter can enforce |

## Harness accounts

The user configures harness accounts in `worker.toml` (see [data-model.md](data-model.md#worker-local-state)). Each account has a `home` and an optional `shadow_home`. A shadow home is a folder that links to the shared home for everything except the login files. So two accounts of one harness share settings, skills and history, and each keeps its own login. T3 Code does this for Codex with symlinks. Each harness keeps its login in a different place, so each adapter knows which files to keep private:

| Harness | Home setting | Private files |
|---|---|---|
| Codex | `CODEX_HOME` | `auth.json` |
| Claude Code | `CLAUDE_CONFIG_DIR`. `claude-agent-acp` reads the login of the folder in this variable | `.credentials.json`, or the keychain entry on macOS (to verify) |
| Pi | Pi's agent folder | `auth.json` |
| OpenCode | `XDG_DATA_HOME` (OpenCode keeps its data in `$XDG_DATA_HOME/opencode`) | `auth.json` |
| ACP agents | Set per registry entry | Set per agent |

The worker never reads the private files. It only reports `auth_state`, which each adapter gets from the harness itself, for example `account/read` in Codex.

The user signs in on the worker machine, with the harness's own command. The client shows the sign-in state and the command to run. Anthropic's terms require the sign-in to go through Anthropic's own flow, so the app never asks for a Claude credential.

## How each path gives a session the hub tools

Every session gets the worker's local MCP endpoint (see [Hub tools](protocol.md#hub-tools)). Each path passes it in a different way:

| Path | How the endpoint reaches the harness |
|---|---|
| ACP | In `mcpServers` of `session/new`, as an `http` server with an `Authorization` header. The agent must report the `http` MCP capability. If it does not, the worker gives it a `stdio` server that runs `consort mcp-bridge --url <endpoint>` with the token in `CONSORT_MCP_TOKEN`, which forwards stdio to the local HTTP endpoint. Claude Code and OpenCode report `http` |
| Codex app-server | In the `config` object of `thread/start`, as `mcp_servers.hub` with `url` and `bearer_token_env_var`. The worker puts the token in that variable of the `codex app-server` process. Tested with codex-cli 0.165.0 |
| Pi RPC | The `--mcp-config` flag, with a generated config file in the session's runtime folder |

The worker also adds the local MCP servers that the channel may use, with the same mechanism.

## Approval modes

The effective approval mode of a session is the lower of the requested mode and the worker policy's `max_approval_mode`. Each adapter maps the four modes onto its harness:

| Mode | Claude (ACP) | Codex | Pi |
|---|---|---|---|
| Ask | `default` permission mode | Approval policy `untrusted`, sandbox `read-only` | Not supported in v1 |
| Auto edits | `acceptEdits` | `on-request`, sandbox `workspace-write` | Not supported in v1 |
| Auto | `acceptEdits` (channel allowed tools are not built yet) | `never`, sandbox `workspace-write` | Default behavior |
| Full access | `bypassPermissions` | `never`, sandbox `danger-full-access` | Default behavior |

The adapter tests check these values. OpenCode gets its mode in `OPENCODE_CONFIG_CONTENT` as a `permission` setting for `edit`, `bash` and `webfetch` (`ask` or `allow`). Gemini CLI gets `--approval-mode` (`default`, `auto_edit` or `yolo`) and supports Ask, Auto edits and Auto. Other ACP agents report Ask only.

If `allow_shell` is false in the worker policy, the worker caps the mode at Auto edits and denies each command approval itself. That is worker policy, so the adapter rule above still holds.

Pi has no built-in approval prompts, so a Pi session can only run in Auto or Full access. A later Pi extension can add a gate that asks the worker before each tool call, as T3 Code does.

## Adapter order

ACP is the common path, because one adapter covers Claude Code (through `claude-agent-acp`, which runs the official Claude Agent SDK), OpenCode and the ACP Registry agents. Native adapters come next, where ACP loses a feature that matters.

| Order | Adapter | Harnesses | Features this path loses |
|---|---|---|---|
| 1 | ACP (`agent-client-protocol` crate) | Claude Code via `@agentclientprotocol/claude-agent-acp`, OpenCode via `opencode acp`, Gemini CLI, Goose and other registry agents | Steering is a non-standard `_session/steering` extension in `claude-agent-acp`. Per-turn token usage is a draft. `claude-agent-acp` needs Node.js 22 or later on the worker. Each agent's `http` MCP support needs a test |
| 2 | Codex native (`codex app-server`) | Codex | The protocol is marked experimental, so we own the changes. We generate Rust types from `codex app-server generate-json-schema` |
| 3 | Pi native (`pi --mode rpc`) | Pi, for any model provider that Pi supports | No approval prompts (see above) |
| Later | Claude native (`claude -p` with `stream-json`) | Claude Code without Node.js | The control protocol has no public specification. Build this only if the ACP path blocks a need |

Fireworks needs no adapter of its own. OpenCode and Pi both support Fireworks with an API key, so it works after step 1.

The ACP path also covers Codex through `codex-acp` before step 2 is done, so all three day-one needs work after step 1. Those needs are Claude through the Agent SDK, Codex with ChatGPT sign-in, and Fireworks.

## Resume after a restart

| Path | Resume method |
|---|---|
| ACP | `session/load`, which replays history, or `session/resume` on agents that support ACP v2 |
| Codex | `thread/resume` with the thread ID |
| Pi | `--session` with the session file |

If resume fails, the worker reports the session as `lost`. For a coordinator, the hub then starts a new session with a context pack (see [architecture.md](architecture.md#how-a-coordinator-keeps-its-context)). For a task, the hub moves the task to `failed` with the reason, and the user or the coordinator can retry it.

## Install harnesses

The worker finds harnesses on `PATH` or at the `binary` path of each account. For ACP Registry agents, the worker can also install the agent from its registry entry into the worker's tools folder, after the user asks for it in a client. The registry gives `npx`, `uvx` or binary launch recipes, and binary archives come with a SHA-256 hash that the worker checks.

## Tests

The `testkit` crate has a fake harness that plays a script of events. Every adapter passes the same contract tests against its real harness: start a session, run a turn, interrupt, round-trip an approval, resume after the process ends, and use a hub tool. The contract tests run on macOS, Windows and Linux, because the worker runs on all three.

Pin exact versions of `agent-client-protocol`, the Codex schema and each adapter package. The ACP crate had six releases in three weeks in October 2026.
