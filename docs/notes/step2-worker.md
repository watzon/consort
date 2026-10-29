# Step 2, worker part: notes

Date: 2026-10-29. Scope: `consort-worker` and `consort-harness`. The plan is in [CONNECTORS-SKILLS-PLAN.md](../CONNECTORS-SKILLS-PLAN.md). The adapter rules are in [harness-adapters.md](../harness-adapters.md#provider-extras-and-skills).

## What was built

- **Provider extras toggle.**
  - `account.update` with `extras_enabled` works. The worker keeps the flag in `worker.db` (table `account_settings`). A missing row means on. Removing the account removes the row.
  - The inventory reports `extras_enabled` and `extras`.
  - Each new session and each resume reads the flag. A running session keeps what it started with.
  - Claude Code, off: inline `settings` and `env` in `_meta.claudeCode.options`, on `session/new` and on `session/load`.
  - Codex, off: `features.apps`, `features.plugins` and `cloud.skills.enabled` set to false in the `config` of `thread/start` and `thread/resume`.
- **Skill delivery.**
  - `SessionStart.extensions.skills` is read. Bundles are fetched with `skill_bundle.get` and the signed blob download, checked, and cached by content hash in `<consort home>/cache/skills/`.
  - A per-session folder is written in `<config dir>/runtime/<session id>/skills/`, in the layout of the harness. The folder is removed when the session ends.
  - `SessionExtensions::prompt_blocks(native)` is added to `system_prompt_append`.
- **OpenCode HOME.** An OpenCode profile account runs with `HOME=<profile>/home`. The tools of the user are pinned to their real places.
- **Code.**
  - `consort-harness`: `extras.rs`, `skills.rs`, new `SessionSpec` fields (`extras_enabled`, `skills`), the Claude `_meta`, the OpenCode config, the Codex config and root, and the Gemini environment.
  - `consort-worker`: `skills.rs` (cache, fetch, check, deliver), `http::download_blob`, the store table, `accounts.rs`, `profile.rs` and `sessions.rs`.
  - `consort-testkit`: one line in a test, for the new `SessionSpec` fields. No other change. No change in `consort-proto`.

## Delivery method per harness, and why

| Harness | Method | Why this one |
|---|---|---|
| Claude Code | A generated skills-only plugin, `plugins` in `_meta.claudeCode.options` | The ACP adapter passes `plugins` to the SDK as it is. The SDK starts Claude Code with `--plugin-dir`. `CLAUDE_CODE_PLUGIN_DIRS` also works (live check below), but it is a setting of the whole process. `plugins` is a setting of one session, and it is in the adapter's rebuild table, so a changed value rebuilds the session on `session/load` |
| Codex | `skills/extraRoots/set` on the session's own `app-server`, before `thread/start` | Codex has no setting, flag or variable for more skill folders. `.agents/skills` in the workspace would write into the user's repo. `$CODEX_HOME/skills` is the account's folder, and a copy of it would need `auth.json`, which Codex rewrites at a token refresh. Each session has its own `app-server`, so a process-wide root is a session root. A core test of `openai/codex` shows that extra roots reach the developer message of a turn |
| Gemini CLI | A per-session Gemini home of links, with a real `.gemini/skills` folder, in `GEMINI_CLI_HOME` | Gemini CLI reads user skills from `$GEMINI_CLI_HOME/.gemini/skills` and `.agents/skills` only, plus the exact workspace folder. It has no setting for more folders. `gemini skills link` writes into the account folder. The workspace route writes into the repo and needs folder trust. The overlay keeps the login, settings and history in the account, through links |
| OpenCode | `skills`, a list of folders, in `OPENCODE_CONFIG_CONTENT` | It is the documented key of OpenCode 2. The older `skills.paths` also works in 2.2.0 |
| Other ACP agents, Pi | The hub tools `list_skills` and `read_skill`, and the context text | No native skills |

Claude Code and OpenCode copy the files. Codex reads the copy. A copy is safe on Windows, and the harness cannot change the cache through it.

### Gemini CLI overlay details

- Every entry of the account's `.gemini` is a link, except `skills`. The account's own skills are links in the real `skills` folder. A Consort skill with the same name replaces the link in the session only.
- Gemini CLI makes files in its home during a session. A file or folder that is new goes to the account when the session ends. A file that Gemini CLI wrote with a rename (this replaced our link, as `projects.json` does) overwrites the account's file. A folder that both have is left as the account has it.
- Two sessions that run at the same time and both make the same new folder keep the one that ends first. This is a rare loss of history for a first run.
- Windows: links to folders need Developer Mode. `native_delivery(GeminiCli)` is false on non-Unix, so Gemini CLI on Windows gets the hub tools. Not tested on Windows.

## Verification of the Claude research

Date: 2026-10-29. Claude Code 2.1.312, claude-agent-acp 0.99.0, SDK 0.3.312. The profile was the user's own, with 19 claude.ai connectors, 2 synced plugins and 2 synced skill folders. Six real runs of `claude -p "Reply with ok" --output-format stream-json --verbose --max-turns 1 --model haiku` in an empty folder. No credential file was read. The synced folders had the same entries after each run, and `plugins/.trash` did not change.

| `system`/`init` data | A: no change | B: inline settings and env off | C: only the two `syncClaudeAi*` keys |
|---|---|---|---|
| `mcp_servers`, total | 33 | 10 | 28 |
| of these, source `claudeai` | 19 | 0 | 19 |
| `skills` | 184 | 157 | 157 |
| `plugins` | 16 | 14 | 14 |
| `plugin_errors` | none | none | none |

- B is `--settings '{"disableClaudeAiConnectors":true,"syncClaudeAiSkills":false,"syncClaudeAiPlugins":false}'` with `ENABLE_CLAUDEAI_MCP_SERVERS=false`.
- **Result: inline settings hide the already synced skills and plugins at run time.** B removed the 15 skills of the synced skill folders (`anthropic-skills:*`), both synced plugins (`engineering@synced`, `cowork-plugin-management@synced`), their 12 skills and their 5 MCP servers. C shows that the two `syncClaudeAi*` keys alone do this. The connectors need `disableClaudeAiConnectors`.
- The SDK says the same: under `--settings`, downloads stop and synced items are hidden "for sessions in that workspace or invocation only (nothing is moved)". Only `false` is honored.
- Not explained: a bare `deep-research` skill stays in B. It is not in the synced folders. `plugin:linear:linear` shows up in B.
- **Skills-only plugin.** A plugin folder with `.claude-plugin/plugin.json` and `skills/<name>/SKILL.md` loads with `--plugin-dir` and with `CLAUDE_CODE_PLUGIN_DIRS` (absolute paths, `:` on Unix and `;` on Windows). It still loads with B. The init list shows `consort-skills:consort-probe-skill`, and the plugin as `consort-skills@inline`. The model called the `Skill` tool with that name, in a run with `bypassPermissions`.
- **Source of the adapter** (`dist/acp-agent.js`): `...userProvidedOptions` is spread into the SDK options (line 8980). `plugins` has no special handling. `settings` as an object is used as it is, with the model config and the provider `apiKeyHelper` added. `env` is `{...process.env, ...options.env, ...providerEnv}`. `settings`, `env` and `plugins` are in `OPTION_REBUILDS_SESSION`.
- **End to end through the real adapter.** `npx @agentclientprotocol/claude-agent-acp@0.99.0`, `session/new` with the exact `_meta` that the worker builds, and the `available_commands_update` of the session (no model call):

  | `_meta.claudeCode.options` | commands | `consort-skills:*` | `anthropic-skills:*` | `engineering:*` and `cowork-plugin-management:*` |
  |---|---|---|---|---|
  | `plugins` only | 242 | 1 | 4 | 12 |
  | `plugins`, `settings`, `env` (extras off) | 215 | 1 | 0 | 0 |

## Verification of the other harnesses

Done with the local binaries (codex-cli 0.165.0, Gemini CLI 0.46.0, OpenCode 2.2.0) in throwaway homes, with no sign-in and no model call, and from the sources of `openai/codex`, `google-gemini/gemini-cli`, `anomalyco/opencode` and the ACP registry.

- **Codex, skills.** Real `app-server`: after `skills/extraRoots/set` with the folder `<root>/skills`, `skills/list` shows the skill with scope `user`. `$CODEX_HOME/skills`, `$HOME/.agents/skills` and the repo's `.agents/skills` and `.codex/skills` are the other places. `~/.claude/skills` is not read. A symlinked skill folder loads, a symlinked `SKILL.md` does not, so the worker copies.
- **Codex, extras.** `thread/start` accepts `features.apps`, `features.plugins` and `cloud.skills.enabled` in `config`. The real sync needs ChatGPT sign-in, so it was not run. The source shows what each key stops (`codex-mcp/src/mcp/mod.rs`, `core-plugins/src/manager.rs`). With an API key none of the three applies.
- **Gemini CLI, skills.** `gemini skills list --all` with `GEMINI_CLI_HOME` set to a link overlay shows the account's own skill (through a link) and the Consort skill. Gemini CLI made `history/` and `projects.json` as real entries in the overlay at the first run, which is why the worker moves them back.
- **Gemini CLI, extras.** A personal Google account has none. An organization can push admin controls (`fetchAdminControls`: required MCP servers, extensions on or off, skills on or off). The docs say users cannot override them. Source: `packages/core/src/code_assist/server.ts`, `admin/admin_controls.ts`, `docs/admin/enterprise-controls.md`.
- **OpenCode, skills and HOME.** A real `opencode serve` with the exact config content that the adapter builds, `/api/skill`:
  - With the real `HOME`: `leak` (from `~/.claude/skills`), `leak2` (from `~/.agents/skills`) and `pdf-forms` (ours).
  - With `HOME` set to a profile folder: `pdf-forms` only.
  - OpenCode takes data, cache, config and state from `XDG_*_HOME` when they are set. `HOME` only decides `~/.claude`, `~/.agents` and `~` in paths. So the sign-in does not move. On Windows `os.homedir()` reads `USERPROFILE`.
  - An isolated `HOME` hides these for the agent's shell tools: the git global config, the npm cache and `.npmrc`, `CARGO_HOME`, `RUSTUP_HOME`. The worker pins them (see below). OpenSSH reads `~/.ssh` from the password database, not from `HOME` (documented, not tested).
  - `OPENCODE_DISABLE_EXTERNAL_SKILLS` does not exist in OpenCode 2. `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1` removes `~/.claude` and the project's `.claude`. The adapter does not need it: `HOME` hides the global folders, and below Full access the project folders are off already.
  - A plain `opencode` command joins a machine-wide background service. Consort starts `opencode acp`, which has a private server.
- **OpenCode, extras.** A connected OpenCode Console adds remote MCP servers, providers, policies and web search. Skills and plugins are not on its list. A policy in `OPENCODE_CONFIG_CONTENT` can deny `mcp:*`, but that also blocks the hub tools, so the worker has no switch. Not run live.
- **ACP registry agents.** Most read MCP servers, skills and rules from local files. Mistral Vibe connectors, GitHub Copilot and Cursor (enterprise settings), Cline, Kilo, Factory Droid and Amp have some account or organization sync. None has a documented per-session switch, except Mistral Vibe `enable_connectors = false` in a file. The notes on the agents other than Mistral, Copilot and Cursor are leads from a fast read, not facts.

## Decisions that the plan did not settle

1. The flag is in `worker.db`, not in `worker.toml`, because a command sets it and a hand edit does not.
2. A change of the flag reaches the next session start or resume. A running session is not restarted.
3. For Claude Code, `extras.mcp_servers` is always empty. The worker cannot list claude.ai connectors without the network, and a session does not report them through ACP. Empty means "not known", not "none". The desktop dialog must not show a count of zero for it.
4. Claude Code uses the SDK `plugins` option, not `CLAUDE_CODE_PLUGIN_DIRS`.
5. The Codex switch is built, though the plan named Claude only. It is in the source and the app-server accepts it. Its side effect is stated in the adapter doc.
6. `extras` is `None` for Codex and Pi. Gemini CLI, OpenCode and ACP accounts report one `cannot_turn_off` sentence each.
7. The Claude `cannot_turn_off` sentence names only plugins and skills that the user installed by hand. A plugin that the organization requires also ignores `enabledPlugins`, but that is about one plugin, not the switch.
8. A skill that fails to fetch does not fail the session. The context text leaves it out, and an error event `skills_not_delivered` tells the user.
9. The context text goes after the hub text in `system_prompt_append`, separated by a blank line.
10. A skill folder is a copy, not a link.
11. The cache folder is `<consort home>/cache/skills/<hash>/` with `bundle.json` and `files/`. Each use checks every file again.
12. A Consort skill replaces an account skill of the same name inside a Gemini session.
13. The tool pins for OpenCode: `GIT_CONFIG_GLOBAL`, `GNUPGHOME`, `CARGO_HOME`, `RUSTUP_HOME`, `BUN_INSTALL` and `npm_config_cache`. Each is set only when the path exists and the worker's own environment does not set the variable. `gh` and other tools that use `XDG_CONFIG_HOME` were already outside the real home before this change.

## What is left

- **Hub side.** The hub must answer `skill_bundle.get` and the blob GET for a worker. It must show `list_skills` and `read_skill` to a session whose harness has no native delivery. The source of truth is `consort_harness::skills::native_delivery`: Claude Code, Codex and OpenCode always, Gemini CLI on Unix only, and no other harness. A Gemini CLI worker on Windows needs the hub tools, and the hub knows the worker OS from its inventory.
- **Model-level checks.** These need a signed-in account and a model call. Does the model see the Consort skills in Codex and Gemini CLI? Does Claude Code ask for a permission when the model uses the `Skill` tool in Ask mode? The checks above stop at the harness's own skill list.
- **Cache clean-up.** Old content hashes stay in `cache/skills/`. A prune by age, with the hashes of open sessions kept, is not built.
- **Windows.** The OpenCode `USERPROFILE` setting and the Gemini CLI fallback are not tested on Windows.
- **Extras.** Counts for Codex and OpenCode, the claude.ai connector names for Claude Code (a session cannot report them yet), and a live check of the Codex and OpenCode switches with a signed-in account.
- **Registry agents.** Check each one that the user adds, with its own docs.
