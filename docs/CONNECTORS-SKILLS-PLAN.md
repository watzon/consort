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
