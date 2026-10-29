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
	/// Takes the input at once, in order with the other calls. The future
	/// gives the answer and does not borrow the session. It fails with
	/// `HarnessError::Unsupported` when `capabilities().steer` is false.
	fn steer(&mut self, input: TurnInput) -> SteerFuture;
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
- If a harness asks for permission, the adapter emits `request.opened` and waits for `respond`. It never answers on its own, even in Full access. In Full access, the adapter configures the harness so that it does not ask. Two exceptions keep a session inside its limits: after an interrupt, the adapter cancels each new request at once, and it removes the options that would raise the session's mode (see [Claude Code and the repo's settings](#claude-code-and-the-repos-settings)).

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

In the UI a harness is a **provider**, and a harness account is an **account**. Accounts belong to a worker. The user makes them in Settings > Workers, with `account.create` (see [protocol.md](protocol.md#requests)). One provider can have several accounts on one worker. A new worker has no accounts, and the worker does not pick up a login that already exists on the machine.

### Profile folders

Each account that Consort makes has its own profile folder, `<consort home>/profiles/<name>/`. The name comes from the account name when the worker makes the account: lowercase, each run of characters other than `a` to `z` and `0` to `9` as one `-`, no `-` at either end, at most 48 characters, `account` when nothing is left, and `-2`, `-3` and so on when the name is taken. The worker writes the account to `worker.toml` as `{ id, harness, label, profile = true, profile_dir = "<name>" }`, plus `acp` for an ACP agent, so a rename does not move the folder. The Consort home is `~/.consort` (`%USERPROFILE%\.consort` on Windows), or `CONSORT_HOME`. `consort dev` uses `<data>/home` unless `CONSORT_HOME` is set. The profile folder holds the harness's own config and login, and `env.json` with the account's secret variables. The worker derives each harness's home settings from the profile folder:

| Harness | Settings |
|---|---|
| Claude Code | `CLAUDE_CONFIG_DIR=<profile>/claude` |
| Codex | `CODEX_HOME=<profile>/codex`. Its `config.toml` gets `cli_auth_credentials_store = "file"` and `check_for_update_on_startup = false` |
| Gemini CLI | `GEMINI_CLI_HOME=<profile>/gemini`. Gemini CLI reads `$GEMINI_CLI_HOME/.gemini/settings.json`, which gets `general.enableAutoUpdate: false` |
| OpenCode | `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME` under `<profile>/xdg/` |
| ACP agents | `HOME` stays as it is. Only the env is set |

The worker merges `env.json` into the session env after the profile variables. It writes `env.json` with mode 0600 on Unix. The inventory reports the variable names (`secret_names`), never the values. The process of a profile account does not get the login variables of the worker's own environment (such as `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `CLAUDE_CONFIG_DIR` and `CODEX_HOME`), so it never uses a login that the machine already has.

In the inventory, an ACP registry agent has the harness `acp:<registry id>`, so a client can show the registry icon. A custom ACP command has `acp:custom`. An `acp` account from `worker.toml` stays `acp`. A new worker has no accounts. The `auto_discover` key of older `worker.toml` files still reads, and the worker ignores it.

### Accounts in worker.toml

A user can still write an account in `worker.toml` by hand (see [data-model.md](data-model.md#worker-local-state)). Such an account has a `home` and an optional `shadow_home`. A shadow home is a folder that links to the shared home for everything except the login files. So two accounts of one harness share settings, skills and history, and each keeps its own login. T3 Code does this for Codex with symlinks. Each harness keeps its login in a different place, so each adapter knows which files to keep private:

| Harness | Home setting | Private files |
|---|---|---|
| Codex | `CODEX_HOME` | `auth.json` |
| Claude Code | `CLAUDE_CONFIG_DIR`. `claude-agent-acp` reads the login of the folder in this variable | `.credentials.json`, or the keychain entry on macOS (to verify) |
| Pi | Pi's agent folder | `auth.json` |
| OpenCode | `XDG_DATA_HOME` (OpenCode keeps its data in `$XDG_DATA_HOME/opencode`) | `opencode/auth.json`. The shadow home has a real `opencode` folder, and each entry of the shared `opencode` folder except `auth.json` is linked into it |
| ACP agents | Set per registry entry | Set per agent |

The worker never reads the private files. It only reports `auth_state`, which each adapter gets from the harness itself, for example `account/read` in Codex.

A session can find out that the login is gone before the next probe. When a turn fails for that reason, the adapter sends a `signed_out` error event. When the open of a session fails for that reason (`auth_required` in ACP), the adapter returns `HarnessError::SignedOut`, and the worker sends the same event. In both cases the event has the step to sign in, and the worker marks the account signed out in its inventory at once, so placement stops choosing it.

### Sign-in

The user signs in from a client, through the harness's own sign-in flow on the worker (`sign_in.start`, see [protocol.md](protocol.md#sign-in)). Anthropic's terms require the sign-in to go through Anthropic's own flow, so the app never asks for a Claude password or token. ADR 0010 has the credential rules.

| Method | How the worker runs it |
|---|---|
| `claude_plan`, `claude_console` | The managed `claude auth login --claudeai` (or `--console`) in a pseudo-terminal (`portable-pty`, ConPTY on Windows) with the profile env and `DISABLE_AUTOUPDATER=1`. `BROWSER` is a program that does nothing (`true` on Unix, `where.exe` on Windows): Claude Code runs `$BROWSER <url>`, so no browser opens on the worker. The worker strips the terminal codes from the output and sends `needs_code` with the first `https://` URL. It types the code from `sign_in.code` and Enter. Success is exit code 0 and `loggedIn` from `claude auth status --json`. A wrong code makes Claude Code exit with "Login failed: ...", and the sign-in fails with that line. Tested with Claude Code 2.1.312 on macOS |
| `chatgpt_device`, `chatgpt_browser` | `codex app-server` with the profile `CODEX_HOME`, then `account/login/start` with `chatgptDeviceCode` (`device_code` with `verificationUrl` and `userCode`) or `chatgpt` (`open_url` with `authUrl`), and a wait for the `account/login/completed` of that `loginId`. Cancel calls `account/login/cancel`. One browser sign-in per worker at a time, because the callback port is fixed |
| `api_key` | No flow of the harness. The key arrives in `env` of `account.create` or `env_set` of `account.update`, before or after `sign_in.start`. The sign-in waits for the key, then probes the account. Codex does not read `OPENAI_API_KEY` from its environment for its account (codex-cli 0.165.0), so for Codex the worker also stores the key with `account/login/start` `apiKey` |
| `acp_agent` | `authenticate` with the method ID, on a short-lived agent process with the profile env |
| `terminal` | The client shows the command. The user runs it on the worker |

Sign out: Claude runs `claude auth logout`. Codex calls `account/logout`. An API key account deletes the key from `env.json`. An ACP agent calls `logout` if the agent has it (`agentCapabilities.auth.logout`). `account.remove` stops the sessions of the account (they end as `lost` with `account_removed`), signs out where it can, and deletes the profile folder.

For Claude, the probe uses `claude auth status --json` (`loggedIn`, `authMethod`, `email`, `subscriptionType`) before the ACP probe, and the inventory reports `email` and `plan`. When `loggedIn` is false, the account is signed out and the ACP probe does not run. Claude Code 2.1.312 reports no expiry. For Codex, `account/read` gives the email and the plan. For an ACP agent, the probe turns the `authMethods` of `initialize` into `sign_in_methods`: a method with no `type` or `type: "agent"` is `acp_agent`, `terminal` is `terminal` with the agent's command and the method's `args`, and `env_var` is `api_key`. The worker offers sign-in methods only for profile accounts.

Two `codex app-server` processes that start at the same time on a new, empty `CODEX_HOME` can fail with "failed to initialize sqlite state runtime" (codex-cli 0.165.0). The adapter tries again, at most three times.

## How each path gives a session the hub tools

Every session gets the worker's local MCP endpoint (see [Hub tools](protocol.md#hub-tools)). Each path passes it in a different way:

| Path | How the endpoint reaches the harness |
|---|---|
| ACP | In `mcpServers` of `session/new`, as an `http` server with an `Authorization` header. The agent must report the `http` MCP capability. If it does not, the worker gives it a `stdio` server that runs `consort mcp-bridge --url <endpoint>` with the token in `CONSORT_MCP_TOKEN`, which forwards stdio to the local HTTP endpoint. Other processes of the same OS user can read that variable. The harness starts the bridge, so no safer channel exists for it. Claude Code and OpenCode report `http`, so they do not use the bridge |
| Codex app-server | In the `config` object of `thread/start`, as `mcp_servers.hub` with `url` and `http_headers` (`Authorization: Bearer <token>`). The token goes through the stdio pipe, not the process environment. Tested with codex-cli 0.165.0: Codex sends the header to the endpoint and does not write the token to its home folder |
| Pi RPC | The `--mcp-config` flag, with a generated config file in the session's runtime folder |

The worker also adds the local MCP servers that the channel may use, with the same mechanism.

## Approval modes

The effective approval mode of a session is the lower of the requested mode and the worker policy's `max_approval_mode`. Each adapter maps the four modes onto its harness:

| Mode | Claude (ACP) | Codex | OpenCode | Gemini CLI | Pi |
|---|---|---|---|---|---|
| Ask | `default` permission mode | Approval policy `untrusted`, sandbox `read-only` | `edit`, `bash` and `webfetch` are `ask` | `default` | Not supported in v1 |
| Auto edits | `acceptEdits` | `on-request`, sandbox `workspace-write` | `edit` is `allow`, `bash` and `webfetch` are `ask` | `auto_edit` | Not supported in v1 |
| Auto | `acceptEdits` (channel allowed tools are not built yet) | `never`, sandbox `workspace-write` | The same as Auto edits | `auto_edit` | Not supported in v1 |
| Full access | `bypassPermissions` | `never`, sandbox `danger-full-access` | `edit`, `bash` and `webfetch` are `allow` | `yolo` | Default behavior |

The adapter tests check these values, and `tests/harness_modes.rs` in `consort-harness` checks the next rule for every adapter. OpenCode gets its mode in `OPENCODE_CONFIG_CONTENT` as a `permission` setting. Gemini CLI gets `--approval-mode`. Other ACP agents report Ask only.

Full access is the only mode in which a harness runs commands with no prompt and no sandbox. Codex Auto runs commands with no prompt, but in its `workspace-write` sandbox. OpenCode, Gemini CLI and Pi have no sandbox, so a mode with no prompt is Full access for them. So in Auto, OpenCode and Gemini CLI ask before each command and each fetch, like Claude, and Pi runs only in Full access. A new worker caps sessions at Auto, so a Full access session needs `max_approval_mode = "full_access"` in `worker.toml`.

A repo's own configuration can raise these limits, so below Full access the adapter keeps it out where the harness allows that:

| Harness | Repo configuration | Below Full access |
|---|---|---|
| Claude (ACP) | `.claude/settings.json`, `.claude/settings.local.json`, `.mcp.json` | Not loaded (see the next section) |
| OpenCode | `opencode.json`, and the `.opencode`, `.claude` and `.agents` folders of the workspace and the folders above it. They can start local MCP servers with any command, load plugins and allow tools per agent | Not loaded. The agent gets `OPENCODE_CONFIG_PROJECT_DISABLE=1` and `OPENCODE_DISABLE_PROJECT_CONFIG=1`. Tested with OpenCode 2.2.0: a repo's local MCP server ran without them and did not run with them |
| Gemini CLI | `.gemini/settings.json` | Still loaded when the user trusts the folder or turns folder trust off. Gemini CLI 0.46.0 has no switch to skip it in a trusted folder, and an untrusted folder forces Gemini's `default` mode. So a Gemini session is weaker here |

The worker's limits also apply to an approval that it would have to cut to send. If the details of an approval do not fit in an event (192 KiB), the worker denies it and does not send it, because the user would allow a command or a change that they cannot see in full. The session gets an `approval_too_long` error event that says what to do. When the harness can steer, the model gets the same text as a steer, because the harness tells the model only that the request was denied.

If `allow_shell` is false in the worker policy, the worker caps the mode at Auto edits and denies each command approval itself. That is worker policy, so the adapter rule above still holds. A command that does not ask for approval still runs, so the adapters also remove the shell tools (`allow_shell` in `SessionSpec`):

| Harness | How the shell tools go away |
|---|---|
| Claude (ACP) | `_meta.claudeCode.options.disallowedTools` is `Bash`, `PowerShell` and `Monitor` (the tools that run commands in Claude Agent SDK 0.3.312) |
| Codex | `features.shell_tool` and `features.unified_exec` are false in the `thread/start` config. Tested with codex-cli 0.165.0: the model gets no `exec_command` and no `write_stdin` tool |
| OpenCode | `bash` is `deny` in `OPENCODE_CONFIG_CONTENT` |
| Gemini CLI | Not done yet. Only the worker's denial of each command approval applies |

### What an approval shows

Every command approval names the folder that the command runs in (`cwd` of the `command` details). When the harness names no folder, the adapter gives the session's workspace, where the harness starts its commands. Claude Code's `Bash` names no folder. OpenCode 2.2.0 names it `workdir` and Gemini CLI 0.46.0 names it `dir_path`, and a relative folder is in the workspace.

Codex asks for network access with a command approval that has `networkApprovalContext` (a host and a protocol). The adapter keeps the `command` details, with the command and its folder, and the summary names the host, for example "Run `git fetch` with network access to github.com". So the user sees which command uses the host. The `network` details have only a `target`, so they would hide the command. A web fetch of an ACP agent (tool kind `fetch`) still has `network` details with the URL.

### Claude Code and the repo's settings

`claude-agent-acp` loads the user, project and local settings of Claude Code. A repo's `.claude/settings.json` can allow tools, set the permission mode, add hooks and start MCP servers, all without an approval. So below Full access, the adapter sends `settingSources: ["user"]` and `allowDangerouslySkipPermissions: false` in `_meta.claudeCode.options`. Only the user's own settings in the account's home then load, and nothing can turn on `bypassPermissions`.

Claude Code loads CLAUDE.md files only with the project settings. So the adapter reads `CLAUDE.md` and `.claude/CLAUDE.md` in the workspace folder itself, and adds them to the system prompt, at most 64 KiB in all. It follows `@path` imports as Claude Code does, up to 5 deep, outside code blocks and code spans. Each file must be a plain file (not a link) whose real path is in the workspace, so an import cannot read a file from somewhere else, and `@~/` imports are not followed. A file loads once, also in a cycle. A workspace with more than one repo is a folder that is not a git repo, with one repo in each subfolder. Then the adapter also reads the CLAUDE.md files of each repo folder. Other repo files that Claude Code would load do not load: skills, agents, commands, hooks, `.mcp.json` and nested CLAUDE.md files. In Full access, Claude Code loads all of its settings as usual.

After `session/new` or `session/load`, the adapter sets the Claude permission mode and reads it back from the `configOptions` of the answer. If the agent has no mode, does not offer the mode, refuses it or reports another mode, the open fails and the adapter stops the agent.

`claude-agent-acp` turns `bypassPermissions` off for root without `IS_SANDBOX`, and when the Claude settings set `disableBypassPermissionsMode`. A probe sends no `allowDangerouslySkipPermissions`, so its mode list shows whether the agent offers `bypassPermissions`. When it does not, the account's capabilities have no Full access, and the worker lowers a Full access request to Auto. When the setting changes after the probe, the open of a Full access session fails with that reason. The adapter then stops that agent, starts a new one in Auto with the limits of Auto, and the session's capabilities have no Full access. The worker then sends an `approval_mode_lowered` error event and removes Full access from the account in the inventory.

The session must stay at or below the Claude mode of its approval mode. The modes rank `plan`, `dontAsk` and `default` lowest, then `acceptEdits`, `auto` and `bypassPermissions`. An unknown mode ranks highest. Two rules hold the session there:

- The adapter removes each permission option that would set a higher mode. These are the options of `claude-agent-acp` 0.99.0 to leave plan mode, such as "Yes, and use auto mode" and "Yes, auto-accept edits" (both `allow_always`, so a client's "Allow for task" would pick them). For a file change, `allow-with-updates` can set `acceptEdits` for the session, so it counts as `acceptEdits`. A request always keeps an allow-once and a deny option.
- When the agent reports a higher mode in a `current_mode_update` or a `config_option_update`, the adapter sets the mode back at once and sends a `mode_reset` error event. If the agent refuses, does not answer in 30 s, or still reports the higher mode, the adapter stops the agent and reports the session as `lost` with a `mode_reset_failed` error event.

Pi has no built-in approval prompts and no sandbox, so a Pi session can only run in Full access. A later Pi extension can add a gate that asks the worker before each tool call, as T3 Code does. Then Pi can support the lower modes.

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
| ACP | `session/load`, which replays history, or `session/resume` on agents that support ACP v2. Claude Code gets the same `_meta` as in `session/new`, so it keeps the system prompt and the limits |
| Codex | `thread/resume` with the thread ID |
| Pi | `--session` with the session file |

If resume fails, the worker reports the session as `lost`. For a coordinator, the hub then starts a new session with a context pack (see [architecture.md](architecture.md#how-a-coordinator-keeps-its-context)). For a task, the hub moves the task to `failed` with the reason, and the user or the coordinator can retry it.

## Processes and time limits

The adapter starts each harness process in the session's runtime folder, not in the workspace. The workspace goes to the harness as `cwd` in `session/new` or `thread/start`. So `npx` does not read the repo's `.npmrc`, and a Windows shell does not find programs in the repo.

| Wait | Limit | What happens then |
|---|---|---|
| The workspace of `session.start`, for all repos together | 40 min | The start fails and the worker removes what it made. A clone has its own 15 min limit inside this. The hub waits 60 min for `session.start`, and the harness open and the first turn take at most about 8 min more |
| ACP agent start and `session/new` | 180 s | The open fails, and the adapter stops the agent |
| ACP `initialize` | 120 s | The open fails, and the adapter stops the agent |
| ACP probe | 150 s | The probe reports that it timed out, and the adapter stops the agent |
| Codex `initialize` | 30 s | The adapter kills the process |
| Any other Codex request | 90 s | The request fails |
| ACP and Codex steer | 10 s | The steer fails. The worker waits for the answer in its own task, so interrupts, policy denials and hub tool answers do not wait for it |
| A Claude mode reset | 30 s | The adapter stops the agent and reports the session as `lost` (`mode_reset_failed`) |
| A turn after an interrupt | 30 s | The adapter kills the harness, ends the turn as failed and reports the session as `lost` (`harness_unresponsive`) |

An ACP turn that stops with `max_tokens`, `max_turn_requests`, another reason or no reason ends as `failed`, with the reason in the error. Only `end_turn` is `completed`, and `cancelled` is `interrupted`.

On Unix each harness process leads its own process group. The worker records each group in `worker.db` with the start time of its leader. A killed worker cannot stop its groups, so at the next start the worker kills each recorded group whose leader still has the recorded start time. When the leader is gone, the processes that it started keep the group, so the worker kills each process of the group that started at or after the recorded time (it reads `/proc` on Linux and `ps` on macOS). When another process has the leader's ID, the worker leaves the group alone. On Windows the job object stops the tree when the worker exits.

Only one worker can use a worker folder. A running worker holds an exclusive lock on `worker.lock` in the folder (`flock` on Unix, `LockFileEx` on Windows), and a second worker refuses to start before it touches the recorded groups.

When the worker stops, it turns on a stopping flag first, then closes each harness and keeps the sessions for the next run. A harness that exits while the flag is on does not make its session lost: the worker drops its `harness_exited`, `harness_unresponsive` and `lost` events. The first of those events waits up to 1 s for the flag, because the same stop signal can reach the harness a moment before the worker. A session start that the stop cut short (still `starting`, with no harness reference) is not resumed. The hub has no result for that `session.start`, so it sends it again, and the worker runs it then.

The flag also covers a `session.start` whose harness fails in the open or in the first turn. The worker waits up to 1 s for the flag here too. When the flag is on, the worker does not mark the session lost and stores no result. If the harness opened, the worker forgets its harness reference, so the session stays `starting` and the next run does not resume it. While the flag is on, the worker also sends no answer to a command that did not run: the hub would take the error as the result. The link closes, and the hub sends the command again after the restart.

So that a stop signal reaches only the worker:

- On Windows each harness starts with `CREATE_NEW_PROCESS_GROUP` and `CREATE_NO_WINDOW`. It does not share the worker's console, so Ctrl-C and a console close do not reach it.
- On Unix each harness leads its own process group, so Ctrl-C in a terminal does not reach it. In a systemd unit, set `KillMode=mixed`. Then SIGTERM goes to the worker only, and the worker stops the harnesses in order. With the default `KillMode=control-group`, SIGTERM reaches every harness at the same time, and a harness can stop in the middle of its work.

## Install harnesses

Each worker downloads and updates its own copy of each harness CLI, as a managed tool in `<consort home>/tools/<tool>/<version>/`. It does not use a global install. The `consort-tools` crate owns the downloads, the checks, the install layout and the launch specs. The worker launches every harness through it, and installs a missing tool first. While a tool installs, the account status says so, for example "Installing Claude Code".

| Tool | Source | Check | Launch |
|---|---|---|---|
| `node` | Node 24 LTS from `nodejs.org/dist` (`unofficial-builds.nodejs.org` on musl Linux) | `SHASUMS256.txt` | |
| `claude-code` | `downloads.claude.ai/claude-code-releases/<version>/<platform>/claude`, version from `stable` | SHA-256 from `manifest.json`, and the signature of `manifest.json` | `claude` |
| `claude-agent-acp` | npm, with the managed Node | npm | `node .../dist/index.js`, with `CLAUDE_CODE_EXECUTABLE` set to the managed `claude` |
| `codex` | GitHub release `rust-v<version>`, `codex-package-<triple>.tar.gz` | `codex-package_SHA256SUMS` | `codex app-server` |
| `opencode` | The `opencode-v2` entry of the ACP registry | SHA-256 from the registry | `opencode acp` |
| `gemini-cli` | npm, with the managed Node | npm | `node .../bundle/gemini.js --acp` |
| `acp:<id>` | An ACP registry agent: `binary` with a SHA-256, or `npx` | SHA-256. A `binary` without one is refused. `uvx` is not supported | From the registry |

An install goes to a temporary folder next to the target, is checked, and is then renamed, so a failed install leaves nothing. The previous version stays until the new one is ready. The worker checks for updates once a day and on `tools.check`, and reports them in the inventory. It installs an update only on `tools.install`. It never updates a tool while a session, a probe or a sign-in uses it, or uses a tool that needs it (Node for the npm tools, Claude Code for its ACP adapter): the install waits until the tool is idle, and the tool's `detail` says so. A missing tool installs when a session, a sign-in or a probe needs it. A session waits for the install, and a probe reports "Installing <tool>." and probes again when the install ends. An account with a `binary` in `worker.toml` runs that program and no managed tool. Self-updates are off: `DISABLE_AUTOUPDATER=1` for Claude Code, `check_for_update_on_startup = false` for Codex, `OPENCODE_DISABLE_AUTOUPDATE=true` for OpenCode, and `general.enableAutoUpdate: false` for Gemini CLI. OpenCode version 2 is the supported line.

## Tests

The `testkit` crate has a fake harness that plays a script of events. Every adapter passes the same contract tests against its real harness: start a session, run a turn, interrupt, round-trip an approval, resume after the process ends, and use a hub tool. The contract tests run on macOS, Windows and Linux, because the worker runs on all three.

Pin exact versions of `agent-client-protocol`, the Codex schema and each adapter package. The ACP crate had six releases in three weeks in October 2026.
