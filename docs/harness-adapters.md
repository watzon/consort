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
	/// The mode of the next turns, at or below the session's mode. See
	/// "A mode for one turn". The default is `Unsupported`.
	async fn set_approval_mode(&mut self, mode: ApprovalMode) -> Result<(), HarnessError>;
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
| `images` | Turn input can hold images. The adapter sends image attachments as image input (see [Attachments and images](#attachments-and-images)) |
| `plan_mode` | The harness has a plan-only mode |
| `usage` | The harness reports token counts |
| `mcp_http` | The harness accepts an HTTP MCP server with a bearer token |
| `approval_modes` | The approval modes that the adapter can enforce |

## Attachments and images

The worker saves the attachments of a turn in the workspace before the turn starts (see [protocol.md](protocol.md#attachments-and-channel-files-on-the-worker)). `consort_harness::attachments` has the path rule, so the worker and the adapters agree on it. Every adapter puts a short block after the turn text:

```text
Attached files. They are saved in your workspace, so you can open them:
- photo.png (image/png, 340.2 KB, also in this message as an image): /path/to/workspace/.consort/attachments/3fa2c1d09e4b-photo.png
- report.pdf (application/pdf, 1.2 MB): /path/to/workspace/.consort/attachments/9b01aa77c2d4-report.pdf
- big.zip (application/zip, 300.0 MB): not available. The worker could not download it. Ask the user to send it again.
```

An adapter whose harness takes image input also sends each image attachment as an image. The adapter reads the format from the first bytes of the file, not from the media type, and sends only PNG, JPEG, GIF and WebP:

| Harness | Image input | Limit |
|---|---|---|
| Claude Code (`claude-agent-acp`) | An ACP `image` content block with base64 `data` and `mimeType`, in `session/prompt` and in `_session/steering` | 5 MiB per image |
| Gemini CLI, OpenCode | An ACP `image` content block, as for Claude Code | 5 MiB per image |
| Codex | A `localImage` item with the absolute path, in `turn/start` and `turn/steer` | 20 MiB per image. Codex scales a large image down itself |
| Other ACP agents | An ACP `image` content block, when the agent reports `agentCapabilities.promptCapabilities.image` | 5 MiB per image |

An ACP session sends image blocks only when the agent reports `promptCapabilities.image` in its `initialize` result, and its `images` capability comes from that report. Before a probe, `images` is on for Claude Code, Gemini CLI, OpenCode and Codex, whose image input was checked in their source (see `docs/notes/phase1-backend.md`). A larger image, or another file, goes by its path only.

## Models and effort

The probe of an account reports its models, each with its options. The `effort` option is a select with the levels that the provider offers for that model, lowest first, and the provider's default level when it names one.

- **Codex** reports the levels per model in `model/list` (`supportedReasoningEfforts` and `defaultReasoningEffort`). The adapter sends the effort with each turn.
- **ACP agents** (Claude Code through `claude-agent-acp` 0.99.0, OpenCode 2.2.1, Gemini CLI) report a `thought_level` config option. The levels change with the model: for example, Claude Code offers no effort for Haiku 4.5, and no Extra high for Opus 4.6. So the probe chooses each model in turn with `session/set_config_option` and reads the options that come back. It stops after 30 seconds or 1,000 models. A model that it does not reach gets the levels of the first model. Both agents list a value `default` that means "the model decides". It is not a level, so the adapter leaves it out, and the provider default is then unknown. At session start the adapter sets the model, then the effort, with `session/set_config_option`. A value that the agent refuses becomes an `option_failed` error, and the session uses the default.

The worker writes the model and the options of each session that it opens to its log (`opening a harness session`).

## Failed turns

The adapter sorts a failed turn into a kind, so the hub can decide what to do (see "The fallback path" in [architecture.md](architecture.md)). `rate_limit` is a usage limit, with the reset time when the harness gives it. `provider_outage` is a provider that is down or overloaded. `expired_sign_in`, `worker_restarted` and `unknown` start no move.

- **Claude Code and the other ACP agents** give the error as text. The adapter reads the words of the Anthropic and OpenAI API errors for an outage: `overloaded_error`, status 500, 502, 503, 504 and 529, "service unavailable", "bad gateway", "gateway timeout" and "internal server error", and the `errorKind` values `overloaded` and `server_error`. A 429 is always a limit, even when the text also says "overloaded". A bare "Internal error" is `unknown`, because the adapters send it for many causes.
- **Codex** gives `codexErrorInfo`. `serverOverloaded`, `internalServerError`, `httpConnectionFailed`, `responseStreamConnectionFailed` and `responseStreamDisconnected` are outages.

These words come from the known API errors. They were not tested against a real outage of a provider.

## Harness accounts

In the UI a harness is a **provider**, and a harness account is an **account**. Accounts belong to the session host on the hub machine. The user makes them in Settings > Providers, with `account.create` (see [protocol.md](protocol.md#requests)). One provider can have several accounts. The session host starts with no accounts, and it does not pick up a login that already exists on the machine.

In the sections below, "the worker" as the owner of an account, a profile folder or a managed tool means the session host. The code moved at the cutover ([ADR 0015](adr/0015-the-hub-machine-owns-accounts-and-agents.md)).

### Harnesses in the boundary

A task session runs its CLI in the boundary, with tools from Consort. `Harness::boundary_support` says whether a harness may run there: proven, not needed or off with a reason. A harness turns on only after its tests in the boundary pass.

| Harness | State |
|---|---|
| Claude Code | On. Real Haiku tasks ran in `sandbox-exec` on a Mac (see [E2E.md](E2E.md)). On Linux, only the boundary and the proxy ran, with no real model. A token refresh in the boundary ran on a Mac (see [E2E.md](E2E.md)), and did not run on Linux |
| Codex | On. Real Codex 0.166.0 ran in `sandbox-exec` on a Mac with a local model, and called a Consort tool (see [E2E.md](E2E.md)). No real ChatGPT turn in the boundary has run yet, and none on Linux |
| Gemini CLI | Off. No test proves it in the boundary |
| OpenCode | Off. No test proves it in the boundary |
| ACP registry agents and custom commands | Off. No test proves them in the boundary |

The config of Claude Code is one function, for a new session and for a load: mode `default`, `settingSources: []`, `allowedTools: ["mcp__consort"]`, no native tools, no plugins and never `bypassPermissions`. `CLAUDE_CODE_EXECUTABLE` pins the version. The allow list of the proxy is `api.anthropic.com` and `platform.claude.com` on port 443. Because `settingSources` is empty, the CLI does not read `AGENTS.md` or `CLAUDE.md` by itself. The system prompt tells the model to read them in the workspace root.

The config of Codex is also one set of keys for a new session and a resume, in all approval modes. The policy is `never` and the sandbox is `read-only`. The `features` of the shell, agents, goals, images, hooks and the browser are false, `web_search` is `disabled`, and each Consort server has `default_tools_approval_mode = "approve"`. Consort asks the user when a Consort tool runs, as it does for Claude Code. `apply_patch` comes from the model catalog and has no switch, so some models still see it. The fixed policy rejects each call, with no prompt and no file. Codex shows MCP tools through `tool_search`, so the developer instructions tell the model to look there. The login is a file in the profile, so a Codex boundary has no keychain rules. The allow list of the proxy is `chatgpt.com`, `api.openai.com` and `auth.openai.com`. A sign-in runs outside the boundary, because it needs a browser.

### Profile folders

Each account that Consort makes has its own profile folder, `<consort home>/profiles/<name>/`. The name comes from the account name when the worker makes the account: lowercase, each run of characters other than `a` to `z` and `0` to `9` as one `-`, no `-` at either end, at most 48 characters, `account` when nothing is left, and `-2`, `-3` and so on when the name is taken. The worker writes the account to `session-host.toml` as `{ id, harness, label, profile = true, profile_dir = "<name>" }`, plus `acp` for an ACP agent, so a rename does not move the folder. The Consort home is `~/.consort` (`%USERPROFILE%\.consort` on Windows), or `CONSORT_HOME`. `consort dev` uses `<data>/home` unless `CONSORT_HOME` is set. The profile folder holds the harness's own config and login, and `env.json` with the account's secret variables. The worker derives each harness's home settings from the profile folder:

| Harness | Settings |
|---|---|
| Claude Code | `CLAUDE_CONFIG_DIR=<profile>/claude` |
| Codex | `CODEX_HOME=<profile>/codex`. Its `config.toml` gets `cli_auth_credentials_store = "file"` and `check_for_update_on_startup = false` |
| Gemini CLI | `GEMINI_CLI_HOME=<profile>/gemini`. Gemini CLI reads `$GEMINI_CLI_HOME/.gemini/settings.json`, which gets `general.enableAutoUpdate: false` |
| OpenCode | `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME` under `<profile>/xdg/`, and `HOME` (`USERPROFILE` too on Windows) set to `<profile>/home/`. OpenCode reads `~/.claude/skills`, `~/.agents/skills` and `~/.claude/CLAUDE.md` from the home folder, so an own `HOME` keeps the skills of the real home out. The worker pins the tools of the user to their real places, for each that exists and that the worker's own environment does not set already: `GIT_CONFIG_GLOBAL` (`~/.gitconfig`), `GNUPGHOME`, `CARGO_HOME`, `RUSTUP_HOME`, `BUN_INSTALL` and `npm_config_cache`. `env.json` can override them. **OpenSSH needs no pin.** `ssh` finds `~/.ssh` (keys, `config`, `known_hosts`) through the password database, not through `HOME`, so the own `HOME` hides `~/.claude` and `~/.agents` but not the ssh files of the user. Tested on macOS with OpenSSH 10.3 and by `openssh_ignores_the_own_home_of_an_opencode_account` in `consort-worker` (`ssh -G` with a `HOME` that has its own `.ssh/config`). `SSH_AUTH_SOCK` stays in the environment, so the ssh agent works. Win32-OpenSSH reads the profile folder of the user token, not `USERPROFILE`; this is not tested. An OpenCode account from `session-host.toml` (not made by Consort) keeps the real `HOME`, so it also loads `~/.claude/skills`; make the account in the app to get the own `HOME` |
| ACP agents | `HOME` stays as it is. Only the env is set |

The worker merges `env.json` into the session env after the profile variables. It writes `env.json` with mode 0600 on Unix. The inventory reports the variable names (`secret_names`), never the values. The process of a profile account does not get the login variables of the worker's own environment (such as `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `CLAUDE_CONFIG_DIR` and `CODEX_HOME`), so it never uses a login that the machine already has.

In the inventory, an ACP registry agent has the harness `acp:<registry id>`, so a client can show the registry icon. A custom ACP command has `acp:custom`. An `acp` account from `session-host.toml` stays `acp`. A new worker has no accounts. The `auto_discover` key of older `session-host.toml` files still reads, and the worker ignores it.

### Accounts that Consort did not make

An older `session-host.toml` can hold an account that Consort did not make. No user needs one, and the app has no way to sign in to it. Such an account has a `home` and an optional `shadow_home`. A shadow home is a folder that links to the shared home for everything except the login files. So two accounts of one harness share settings, skills and history, and each keeps its own login. T3 Code does this for Codex with symlinks. Each harness keeps its login in a different place, so each adapter knows which files to keep private:

| Harness | Home setting | Private files |
|---|---|---|
| Codex | `CODEX_HOME` | `auth.json` |
| Claude Code | `CLAUDE_CONFIG_DIR`. `claude-agent-acp` reads the login of the folder in this variable | `.credentials.json`, or the keychain entry on macOS (to verify) |
| Pi | Pi's agent folder | `auth.json` |
| OpenCode | `XDG_DATA_HOME` (OpenCode keeps its data in `$XDG_DATA_HOME/opencode`) | `opencode/auth.json`. The shadow home has a real `opencode` folder, and each entry of the shared `opencode` folder except `auth.json` is linked into it |
| ACP agents | Set per registry entry | Set per agent |

The worker never reads the private files. It only reports `auth_state`, which each adapter gets from the harness itself, for example `account/read` in Codex.

**A login that expired.** `claude auth status` reports no expiry, so an expired Claude Code login passes the probe (`signed in`), and the first turn finds out. `claude-agent-acp` then ends the turn with `auth_required`, or with an internal error that carries the API error: `Failed to authenticate. API Error: 401`, `authentication_error` or "OAuth token has expired". When a failed turn of a Claude Code account has `auth_required` or one of these words (`login_expired_text` in `consort-harness`), the adapter sends the error event `login_expired` with the message "The login expired. Sign in again." and ends the turn with the same text. The worker marks the account `error` with that detail in its inventory (the app reads "expired" as Expired and shows Sign in again), and it keeps the mark across probes, because the probe cannot see the expiry. A completed turn on the account, or a sign-in that succeeds, clears the mark. `login_expired` is not `signed_out`: the login is not gone, it needs a new sign-in. Other harnesses do not use it, because their probes see an expiry or the text is not known.

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

## Provider extras and skills

Two things could reach a session from outside the account's own folders: what the provider syncs to the account, and the **skills** that the hub gives to the session's channel. Consort turns off the first one, always. The user has no setting for it. The reasons, the tests and the results of the live checks are in [notes/step2-worker.md](notes/step2-worker.md).

### What the provider syncs

The harness starts without the connectors, plugins, skills and hooks that the provider adds to the account, where the harness allows that. The session host sets this at each `session/new`, `session/load` or `thread/start` or `thread/resume`. The app and the hub tools have no switch for it. The session host reports nothing about it in the inventory.

| Harness | Synced by the provider | How it is off |
|---|---|---|
| Claude Code | claude.ai connectors, synced plugins, synced skills | `_meta.claudeCode.options` of `session/new` and `session/load` gets `settings` `{"disableClaudeAiConnectors": true, "syncClaudeAiSkills": false, "syncClaudeAiPlugins": false}` and `env` `{"ENABLE_CLAUDEAI_MCP_SERVERS": "false"}`. Inline settings apply to the run only and move no file. Consort never writes the `syncClaudeAi*` keys to a settings file, because there Claude Code moves the synced folders to `.trash`. `settingSources` is empty, so no hook of a profile or a repo runs |
| Codex | With ChatGPT sign-in: ChatGPT apps, plugins of the account, cloud skills | The `config` of `thread/start` and `thread/resume` gets `features.apps` and `features.plugins` false and `cloud.skills.enabled` false. `features.plugins` is one switch for all plugins, so it also stops plugins that the user installed by hand |
| Gemini CLI | None for a personal Google account. The admin controls of an organization | No switch. Organization settings apply |
| OpenCode | Servers and providers of a connected OpenCode Console | No switch that spares the hub tools |
| ACP registry agents | Differs by agent, mostly enterprise settings | No switch |

### Skills

`session.start` names the skills of a session in `extensions.skills`, each with a content hash. For a harness with native skills, the worker:

1. reads the bundle from its cache, `<consort home>/cache/skills/<content hash>/`, and checks every file again;
2. on a miss, asks the hub for the file list (`skill_bundle.get`), checks `SkillBundle::is_consistent`, downloads each file with the signed `GET /v1/blobs/{sha256}`, checks the size and SHA-256 of each file, and renames a finished folder into the cache;
3. copies the files into `<config dir>/runtime/<session id>/skills/` in the layout of the harness;
4. removes that folder when the session ends (also for a lost session, a stop and an account removal), and at the next start for the sessions that ended while the worker was down.

**The cache prune.** Each use of a cached bundle touches its `used` file. When the worker starts, and then once a day, it removes the bundles that no open session names (the hashes in the `extensions.skills` of the sessions in `worker.db`) and that nobody used for 30 days. A bundle that a session needs again comes from the hub. A temporary folder of a download in progress stays. A bundle from before the `used` file counts from the time of its file list.

A resume writes the folder again from the cache, so the session keeps the version that it started with, also when the hub is down. A skill that fails does not fail the session. The session starts without it, the context text leaves it out, and the worker records a `skills_not_delivered` error event.

| Harness | Layout in the session folder | How the harness finds it |
|---|---|---|
| Claude Code | `consort-skills/.claude-plugin/plugin.json` and `consort-skills/skills/<name>/`. The plugin has no hooks, commands, agents or MCP servers | `plugins` `[{"type": "local", "path": ...}]` in `_meta.claudeCode.options`. Claude Code names the skills `consort-skills:<name>` |
| Codex | `skills/<name>/` | `skills/extraRoots/set` on the session's own `app-server`, before `thread/start` |
| Gemini CLI (Unix) | `gemini-home/`: a copy of the account's Gemini home in which every file and folder is a link, except `.gemini/skills/`, which is a real folder with links to the account's own skills and copies of the Consort skills | `GEMINI_CLI_HOME` of the agent process. At the end of the session the worker moves back what Gemini CLI made in the copy |
| OpenCode | `skills/<name>/` | `skills` (a list of folders) in `OPENCODE_CONFIG_CONTENT` |
| Other ACP agents, Pi, and Gemini CLI on Windows | Nothing is written | The hub tools `list_skills` and `read_skill`, and a line in the context text |

The worker adds `SessionExtensions::prompt_blocks(native)` to the end of `system_prompt_append`, after the text from the hub. That text goes to the harness as the hub text always did: `systemPrompt.append` for Claude Code, `developerInstructions` for Codex, and in front of the first turn for the other ACP agents.

## Approval modes

The effective approval mode of a session is the lower of the requested mode and the worker policy's `max_approval_mode`. Each adapter maps the four modes onto its harness:

| Mode | Claude (ACP) | Codex | OpenCode | Gemini CLI | Pi |
|---|---|---|---|---|---|
| Ask | `default` permission mode | Approval policy `untrusted`, sandbox `read-only` | `edit`, `bash` and `webfetch` are `ask` | `default` | Not supported in v1 |
| Auto edits | `acceptEdits` | `on-request`, sandbox `workspace-write` | `edit` is `allow`, `bash` and `webfetch` are `ask` | `auto_edit` | Not supported in v1 |
| Auto | `auto` (asks only for destructive actions and for actions outside the workspace). If the adapter or the model does not offer it, `acceptEdits` | `never`, sandbox `workspace-write` | The same as Auto edits | `auto_edit` | Not supported in v1 |
| Full access | `bypassPermissions` | `never`, sandbox `danger-full-access` | `edit`, `bash` and `webfetch` are `allow` | `yolo` | Default behavior |

The adapter tests check these values, and `tests/harness_modes.rs` in `consort-harness` checks the next rule for every adapter. OpenCode gets its mode in `OPENCODE_CONFIG_CONTENT` as a `permission` setting. Gemini CLI gets `--approval-mode`. Other ACP agents report Ask only.

Full access is the only mode in which a harness runs commands with no prompt and no sandbox. Codex Auto runs commands with no prompt, but in its `workspace-write` sandbox. OpenCode, Gemini CLI and Pi have no sandbox, so a mode with no prompt is Full access for them. So in Auto, OpenCode and Gemini CLI ask before each command and each fetch, and Pi runs only in Full access. Claude Code has a middle mode of its own (`auto`), so in Auto it asks only for destructive actions and for actions outside the workspace. A new worker caps sessions at Auto, so a Full access session needs `max_approval_mode = "full_access"` in `session-host.toml`.

A repo's own configuration can raise these limits, so below Full access the adapter keeps it out where the harness allows that:

| Harness | Repo configuration | Below Full access |
|---|---|---|
| Claude (ACP) | `.claude/settings.json`, `.claude/settings.local.json`, `.mcp.json` | Not loaded (see the next section) |
| OpenCode | `opencode.json`, and the `.opencode`, `.claude` and `.agents` folders of the workspace and the folders above it. They can start local MCP servers with any command, load plugins and allow tools per agent | Not loaded. The agent gets `OPENCODE_CONFIG_PROJECT_DISABLE=1` and `OPENCODE_DISABLE_PROJECT_CONFIG=1`. Tested with OpenCode 2.2.0: a repo's local MCP server ran without them and did not run with them |
| Gemini CLI | `.gemini/settings.json` | Still loaded when the user trusts the folder or turns folder trust off. Gemini CLI 0.46.0 has no switch to skip it in a trusted folder, and an untrusted folder forces Gemini's `default` mode. So a Gemini session is weaker here |

The worker's limits also apply to an approval that it would have to cut to send. If the details of an approval do not fit in an event (192 KiB), the worker denies it and does not send it, because the user would allow a command or a change that they cannot see in full. The session gets an `approval_too_long` error event that says what to do. When the harness can steer, the model gets the same text as a steer, because the harness tells the model only that the request was denied.

If `allow_shell` is false in the worker policy, the worker caps the mode at Auto edits and denies each command approval itself. That is worker policy, so the adapter rule above still holds. A command that does not ask for approval still runs, so the adapters also remove the shell tools (`allow_shell` in `SessionSpec`). A coordinator session always has `allow_shell` false and `SessionSpec.coordinator` true, whatever the policy says (DECISIONS 740). For a coordinator the adapters also remove the web tools:

| Harness | Shell tools | Web tools (coordinator only) |
|---|---|---|
| Claude (ACP) | `_meta.claudeCode.options.disallowedTools` is `Bash`, `PowerShell` and `Monitor` (the tools that run commands in Claude Agent SDK 0.3.312) | `WebFetch` and `WebSearch` are added to `disallowedTools` |
| Codex | `features.shell_tool` and `features.unified_exec` are false in the `thread/start` config. Tested with codex-cli 0.165.0: the model gets no `exec_command` and no `write_stdin` tool | `web_search` is `disabled` in the `thread/start` config |
| OpenCode | `bash` is `deny` in `OPENCODE_CONFIG_CONTENT` | `webfetch` and `websearch` are `deny`, and `hub_*` is `allow`. The `hub_*` match was not tested with a real model |
| Gemini CLI | A policy file in the runtime folder, passed with `--admin-policy`, denies `run_shell_command`. Tested with Gemini CLI 0.66.0 | The same file denies `google_web_search`, `web_fetch` and `codebase_investigator`, and allows the MCP server `hub` |
| Other ACP agents | No removal at the source. The worker denies each approval request | Same |

The worker allows a hub tool of a coordinator without a card only when the adapter names the tool in a field that the harness sets (for Claude, `_meta.claudeCode.toolName`). A title that the agent or the model chose does not count.

### A mode for one turn

A `session.turn` (and the first turn of a `session.start`) can name an approval mode for that turn only. The hub does this for a coordinator's proactive turns and the research pass: they run in Ask. A coordinator session runs in Ask anyway (DECISIONS 700), so today the turn mode and the session mode are the same, and the rule stays as a guard. The worker runs the turn at the lower of that mode and the session's own mode, and the next turn that names no mode goes back to the session's mode. It calls `HarnessSession::set_approval_mode` only when the mode changes, and never while a turn runs.

| Adapter | How it changes the mode |
|---|---|
| Claude (ACP) | The same call that sets the mode at the start (`session/set_config_option` with the `mode` option, or `session/set_mode`), and the adapter reads the mode back. The new limit holds at once: the options that would raise the mode are removed, and a higher mode that the agent reports is set back |
| Codex | `turn/start` takes `approvalPolicy` and `sandboxPolicy` (an object such as `{"type": "readOnly"}`). Codex keeps them for the next turns, so the adapter sends them only with the turn whose mode changed. A session that goes back to its mode gets the plain sandbox policy of that mode, without the extra settings of the user's Codex config |
| OpenCode, Gemini CLI, other ACP agents, Pi | Not supported: they take their mode when they start. The worker refuses a stricter turn with `policy_denied`. A coordinator on such a harness runs proactive turns only when the channel's default mode is Ask already |

### A model change in a running session

A `session.turn` can carry a `model_change` (a model and its options). The worker calls `HarnessSession::set_model` before the turn, and only when `capabilities().switch_model` is true. It saves the new model in the stored start of the session, so a restart resumes with it. A harness that cannot switch, or a switch that fails, makes the worker refuse the turn with `unsupported`, and the hub starts a new session instead.

| Adapter | How it changes the model |
|---|---|
| Claude (ACP), OpenCode | `session/set_config_option` with the `model` option, then the `effort` option. The adapter sets the model and its options before the mode, and sets the mode of a Claude session again after a later model change, because the new model decides which modes are on offer |
| Codex | `turn/start` takes the model and the effort. Codex keeps them for the next turns |
| Gemini CLI, other ACP agents, Pi | Not supported: `switch_model` is false |

### What an approval shows

Every command approval names the folder that the command runs in (`cwd` of the `command` details). When the harness names no folder, the adapter gives the session's workspace, where the harness starts its commands. Claude Code's `Bash` names no folder. OpenCode 2.2.0 names it `workdir` and Gemini CLI 0.46.0 names it `dir_path`, and a relative folder is in the workspace.

Codex asks for network access with a command approval that has `networkApprovalContext` (a host and a protocol). The adapter keeps the `command` details, with the command and its folder, and the summary names the host, for example "Run `git fetch` with network access to github.com". So the user sees which command uses the host. The `network` details have only a `target`, so they would hide the command. A web fetch of an ACP agent (tool kind `fetch`) still has `network` details with the URL.

### Claude Code and the repo's settings

`claude-agent-acp` loads the user, project and local settings of Claude Code. A repo's `.claude/settings.json` can allow tools, set the permission mode, add hooks and start MCP servers, all without an approval. So below Full access, the adapter sends `settingSources: ["user"]` and `allowDangerouslySkipPermissions: false` in `_meta.claudeCode.options`. Only the user's own settings in the account's home then load, and nothing can turn on `bypassPermissions`.

Claude Code loads CLAUDE.md files only with the project settings. So the adapter reads `CLAUDE.md` and `.claude/CLAUDE.md` in the workspace folder itself, and adds them to the system prompt, at most 64 KiB in all. It follows `@path` imports as Claude Code does, up to 5 deep, outside code blocks and code spans. Each file must be a plain file (not a link) whose real path is in the workspace, so an import cannot read a file from somewhere else, and `@~/` imports are not followed. A file loads once, also in a cycle. A workspace with more than one repo is a folder that is not a git repo, with one repo in each subfolder. Then the adapter also reads the CLAUDE.md files of each repo folder. Other repo files that Claude Code would load do not load: skills, agents, commands, hooks, `.mcp.json` and nested CLAUDE.md files. In Full access, Claude Code loads all of its settings as usual.

After `session/new` or `session/load`, the adapter sets the Claude permission mode and reads it back from the `configOptions` of the answer. If the agent has no mode, does not offer the mode, refuses it or reports another mode, the open fails and the adapter stops the agent.

One case does not fail: Auto. `claude-agent-acp` 0.99.0 always lists `auto`, but it keeps `acceptEdits` when the model does not support `auto` mode (`supportsAutoMode`), and Claude Code can refuse `auto` for an account. When the adapter does not offer `auto`, refuses it, or reports `acceptEdits` after the request, the session goes on in `acceptEdits` and the worker logs a warning (`claude_mode_fallback`). The same holds for a mode change in a running session. The session never falls back to a mode above the one the user chose. See DECISIONS 701.

`claude-agent-acp` turns `bypassPermissions` off for root without `IS_SANDBOX`, and when the Claude settings set `disableBypassPermissionsMode`. A probe sends no `allowDangerouslySkipPermissions`, so its mode list shows whether the agent offers `bypassPermissions`. When it does not, the account's capabilities have no Full access, and the worker lowers a Full access request to Auto. When the setting changes after the probe, the open of a Full access session fails with that reason. The adapter then stops that agent, starts a new one in Auto with the limits of Auto, and the session's capabilities have no Full access. The worker then sends an `approval_mode_lowered` error event and removes Full access from the account in the inventory.

The session must stay at or below the Claude mode of its approval mode. The modes rank `plan`, `dontAsk` and `default` lowest, then `acceptEdits`, `auto` and `bypassPermissions`. So `auto` is allowed in Auto and Full access, and never in Ask and Auto edits. An unknown mode ranks highest. Two rules hold the session there:

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

The session host downloads and updates its own copy of each harness CLI, as a managed tool in `<consort home>/tools/<tool>/<version>/`. It does not use a global install. The `consort-tools` crate owns the downloads, the checks, the install layout and the launch specs. The worker launches every harness through it, and installs a missing tool first. While a tool installs, the account status says so, for example "Installing Claude Code".

| Tool | Source | Check | Launch |
|---|---|---|---|
| `node` | Node 24 LTS from `nodejs.org/dist` (`unofficial-builds.nodejs.org` on musl Linux) | `SHASUMS256.txt` | |
| `claude-code` | `downloads.claude.ai/claude-code-releases/<version>/<platform>/claude`, version from `stable` | SHA-256 from `manifest.json`, and the signature of `manifest.json` | `claude` |
| `claude-agent-acp` | npm, with the managed Node | npm | `node .../dist/index.js`, with `CLAUDE_CODE_EXECUTABLE` set to the managed `claude` |
| `codex` | GitHub release `rust-v<version>`, `codex-package-<triple>.tar.gz` | `codex-package_SHA256SUMS` | `codex app-server` |
| `opencode` | The `opencode-v2` entry of the ACP registry | SHA-256 from the registry | `opencode acp` |
| `gemini-cli` | npm, with the managed Node | npm | `node .../bundle/gemini.js --acp` |
| `acp:<id>` | An ACP registry agent: `binary` with a SHA-256, or `npx` | SHA-256. A `binary` without one is refused. `uvx` is not supported | From the registry |

An install goes to a temporary folder next to the target, is checked, and is then renamed, so a failed install leaves nothing. The previous version stays until the new one is ready. The worker checks for updates once a day and on `tools.check`, and reports them in the inventory. It installs an update only on `tools.install`. It never updates a tool while a session, a probe or a sign-in uses it, or uses a tool that needs it (Node for the npm tools, Claude Code for its ACP adapter): the install waits until the tool is idle, and the tool's `detail` says so. A missing tool installs when a session, a sign-in or a probe needs it. A session waits for the install, and a probe reports "Installing <tool>." and probes again when the install ends. An account with a `binary` in `session-host.toml` runs that program and no managed tool. Self-updates are off: `DISABLE_AUTOUPDATER=1` for Claude Code, `check_for_update_on_startup = false` for Codex, `OPENCODE_DISABLE_AUTOUPDATE=true` for OpenCode, and `general.enableAutoUpdate: false` for Gemini CLI. OpenCode version 2 is the supported line.

**Helper tools.** Node.js and the Claude Code ACP adapter (`node` and `claude-agent-acp`) only support a provider. They install and update as before, but the app lists no row or section for them in Settings > Providers. The client hides them from the view, and the inventory still reports them. A helper that installs, fails to install or has an update shows on the row of the provider that needs it: Claude Code for both, and Gemini CLI for Node.js. That row then says "Installing Node.js.", "Node.js did not install." with the cause, or "An update for Node.js is available.", and its Try again or Update button runs the install of the provider and of the helpers that failed or have an update. Check for updates already checks every tool, helpers included.

## Tests

The `testkit` crate has a fake harness that plays a script of events. Every adapter passes the same contract tests against its real harness: start a session, run a turn, interrupt, round-trip an approval, resume after the process ends, and use a hub tool. The contract tests run on macOS, Windows and Linux, because the worker runs on all three.

Pin exact versions of `agent-client-protocol`, the Codex schema and each adapter package. The ACP crate had six releases in three weeks in October 2026.
