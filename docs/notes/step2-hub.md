# Step 2, hub part: notes

These notes are for the merge into `docs/DECISIONS.md` and for the agents of the worker and the desktop. The plan is [CONNECTORS-SKILLS-PLAN.md](../CONNECTORS-SKILLS-PLAN.md). The contract (`consort-proto`) did not change.

## What was built

All of it is in `crates/consort-hub`.

- **Composio settings** (`src/connectors/mod.rs`, `src/connectors/key.rs`). `composio.set_key`, `composio.remove_key` and `composio.status`. The key is encrypted at rest. The hub has one stable `user_id`. `set_key` checks the key with one cheap call before it stores the key.
- **Composio client** (`src/connectors/composio.rs`). A hand-written `reqwest` client for the v3.1 API, with only the calls in the list below. The base URL is a setting. `HubHandle::set_composio_url` and the `CONSORT_COMPOSIO_URL` variable set it.
- **Catalog**. `connector_catalog.search`, `.categories` and `.service`, from a cache in memory that the hub fetches again after `CATALOG_REFRESH_MS`.
- **Connections** (`src/connectors/connections.rs`, `src/store/connectors.rs`, migration `schema_v5.sql`). The `connections` table, feed upserts and deletes, and the `connections` field of the `all` snapshot. `connection.connect`, `.update`, `.disconnect`, `.reconnect` and `.calls`. A task follows each browser sign-in until the account is active, fails, or `CONNECT_ATTEMPT_TIMEOUT_MS` ends. The hub starts these tasks again when it starts.
- **Channel exceptions and the effective set**. `channel.set_connection_enabled` and `channel.set_skill_enabled`. `store::connectors::effective_connections` and `store::skills::effective_skills` give the effective set.
- **Session extensions** (`src/connectors/extensions.rs`). Coordinator and task sessions get `SessionStart.extensions` from the effective set. `tool.list` shows the connector tools only when the channel has a connection, and `list_skills` and `read_skill` only for a harness that does not load skills itself.
- **Connector tools** (`src/connectors/run.rs`). `list_connector_tools`, `get_connector_tool` and `run_connector_tool`, with the approval rule for each tool kind, connector cards, the audit log (`connector_calls`) and the result limit.
- **Hub cards** (`src/connectors/cards.rs`). The hub posts each card as an approval with the `request_id` `hub:<approval id>`. It carries out the answer itself and never sends `session.respond` for a card. The session that asked gets `card_answered`.
- **Management tools** (`src/tools/management.rs`). Every tool of the plan for both audiences, except `search_skills` and `add_skill` (step 3). Each tool follows the risk rule.
- **Skills, storage side** (`src/skills.rs`, `src/store/skills.rs`). The `skills`, `skill_versions` and `skill_bundle_grants` tables. The files are blobs. Feed, snapshot, `skill.update`, `skill.remove`, `skill.files`, `skill.read_file`, `skill_bundle.get`, `list_skills`, `read_skill`, `list_installed_skills`, `set_channel_skill`, `update_skill` and `remove_skill`. `Hub::store_skill` stores a checked set of files. `HubHandle::add_skill_from_files` makes it available to tests and to tools on the hub machine.
- **Tests**. A fake Composio server (`src/connectors/fake.rs`) that the unit tests and `tests/connectors.rs` both use. No test talks to the real Composio.

## Decisions to merge into DECISIONS.md

| Area | Decision | Why |
|---|---|---|
| Hub | The hub makes a random 256-bit key once and keeps it in `hub-secret.key` (mode 0600). `composio.key` (mode 0600) is `CSK1`, a 12-byte nonce and the AES-256-GCM ciphertext. The associated data is `consort composio.key v1`. | The plan asks for a hub-made key in a 0600 file. A copy of `hub.db` or of `composio.key` alone does not give the key |
| Hub | `composio.set_key` checks the key with `GET /toolkits?limit=1`. A refused key fails with `invalid` and is not stored. A network failure fails with `composio_error` and is not stored. | v3.1 has no call that checks a project key alone. Only a key that works is kept |
| Hub | `composio.status` has no `plan` and no `usage` yet. | The plan is only in `GET /auth/session/info`, which takes a user key, not a project key. The units of the usage endpoint are not documented |
| Hub | The status (without the key) is JSON in `meta` under `composio_status`. The `user_id` is `consort-hub-<uuid>` in `meta` under `composio_user_id`, made the first time the hub needs it. | No migration for two small values, as with preferences (row 332) |
| Hub | The default base URL is `https://backend.composio.dev/api/v3.1`. `CONSORT_COMPOSIO_URL` and `HubHandle::set_composio_url` change it. `HubConfig` did not change. | Tests need a local fake. A new `HubConfig` field would break `consort-cli` |
| Hub | The catalog cache is in memory. The hub fetches it when a request needs it and the copy is older than one day. A failed fetch answers with the old copy, and the hub waits 30 seconds before it tries again. Search is local (slug, name and description). The cursor of `connector_catalog.search` is an offset. The tools of each service have their own cache with the same age. Deprecated tools are left out. | One fetch serves every client and session. The toolkit list is small enough to filter on the hub |
| Hub | `logo_url` is the URL that Composio gives. The hub does not cache logos yet. | The task allowed it. data-model.md says "From the hub's logo cache", so it must change or the cache must come |
| Hub | Without a requested scheme, the hub picks a Composio-managed browser scheme, then any managed scheme, then a browser scheme, then the first scheme. For the auth config it reuses an enabled Composio-managed config of that scheme, then makes a managed one, then reuses one that the user made in Composio. A scheme with no browser step gets a new custom config. A browser scheme that Composio does not manage and that has no config is refused. | "Composio-managed first" from the plan |
| Hub | The Composio alias of a connected account is the connection handle. | Handles are unique per hub, and Composio wants a unique alias per user and toolkit |
| Hub | `connection.reconnect` deletes the old connected account first, then links again with the same alias. | The alias must be unique, and the refresh endpoint of v3.1 is deprecated |
| Hub | `identity` comes only from the keys `email`, `user_email`, `account_email`, `email_address`, `login`, `username`, `user_name` and `account_name` in the connected account's `state.val`, `data` or `params`. | v3.1 has no identity field, and `state.val` also holds tokens. The hub reads no other key |
| Hub | Composio statuses: `ACTIVE` is active. `INITIALIZING` and `INITIATED` wait. `EXPIRED`, `INACTIVE` and `REVOKED` make an active connection `needs_sign_in` and a connect attempt `failed`. Every other status is `failed`. | The statuses in the OpenAPI file |
| Hub | A connect attempt asks Composio every 3 seconds. | Fast enough for the dialog, and at most 200 calls in 10 minutes |
| Hub | The hub checks the active connections every hour. It also checks the account when a tool call fails. An expired account makes the connection `needs_sign_in`, and the call fails with `needs_user`. | "A connection that Composio reports expired becomes needs a new sign-in" |
| Hub | The effective set has the connections that are `active` or `needs_sign_in`. A connection that is `connecting` or `failed` is left out. | A session can do nothing with it. A connection that needs a new sign-in stays, so the assistant can tell the user |
| Hub | A new name gives the connection a new handle. | The handle is the slug of the name |
| Hub | `connection.disconnect` deletes the Composio account first (no `revoke_on_delete`), then the connection, its channel exceptions and its "ask once" answers. The audit log stays. With no key, only the connection goes. A Composio error keeps the connection. | The user can try again, and the audit stays for later |
| Hub | `run_connector_tool` waits 4 minutes 45 seconds, not 5 minutes, before it returns `waiting_for_user`. | The worker gives up on a hub request after 5 minutes (`REQUEST_TIMEOUT` in `consort-worker/src/link.rs`). The answer must reach it first. protocol.md says "up to 5 minutes" |
| Hub | A connector card has no time limit. A later Allow runs the call if the connection is still active and still in the channel's effective set. | The user may answer much later. The scope may have changed in the meantime |
| Hub | "Ask once per task" answers live in the `connector_grants` table, keyed by the task ID (or the coordinator session ID), the connection and the tool kind. | A task can last days and outlive a hub restart |
| Hub | A result over 256 KiB becomes a string with the start of its JSON text, with `truncated`. `result_bytes` in the audit is the full size. | The data stays valid JSON, and the assistant sees where it stops |
| Hub | Card fields: To, Cc, Bcc, Subject and Body first, from the usual argument names (`to`, `recipient_email`, `subject`, `body`, `message_body` and others). Then every other argument, with its name as a label in sentence case. A value has at most 2,000 characters, and a card has at most 20 fields. | "For email: To, Subject, Body", and readable fields for other tools |
| Hub | A hub card is not canceled when its session ends or reopens. `release_task` still cancels the cards of a task that leaves. | A hub card needs no harness request. Its answer goes to the channel or the task |
| Hub | A card message has the author `assistant` and the kind `approval`. In a task it goes in the task thread, and the task becomes Needs input. | The same as an approval from a harness |
| Hub | When the user allows a management card, the hub parses the stored tool call again (`ManagementCard.tool` and `arguments`) and runs it with the state of that moment. | One code path for "at once" and "after the card". The result fits the state when it runs |
| Hub | The activity line is a notice with the kind `assistant_change` in the session's channel, for a change that ran at once. A change after a card has no activity line, because the card shows it. | The plan asks for "a short activity line". Notices are the hub's way to post one |
| Hub | Risk details: `create_channel` with tags asks first. `update_channel` asks only when the tags change. `set_channel_connector` and `set_channel_skill` run at once in both directions, inside the item's scope. `update_skill` asks for `accept_new_version`, for turning on a skill with scripts, and for a wider scope. `set_default_model` sets the coordinator model of Everything. `set_account_extras` asks only to turn extras on. | New tags give a channel access, "only approval moves the pin", and the plan marks the channel tools as scope-limited, not "asks first" |
| Hub | `request_access` on a tag scope makes the scope a channel list: the channels that the tags give it now, plus this channel. On a channel list it adds the channel. | A scope cannot say "these tags or this channel". The card states the new scope |
| Hub | `connect_service` from a channel coordinator accepts a `use_in` that the assistant gives. The connect card shows it to the user. | The user completes the card, so the user sees the scope first |
| Hub | `start_sign_in` reads `method` as a method kind, such as `claude_plan`, and keeps it only when the account offers it. A method with fields, such as an API key, needs the app. Allow on a sign-in card with a method starts the sign-in and puts `sign_in_id` on the card. | The assistant never handles a secret |
| Hub | `add_account` creates the account on the worker at once (it holds no secret), then posts a sign-in card. | The card needs an account to sign in to |
| Hub | `group` and `repos` in the channel tools: a group is found by its name (any case) or made. A repo is found by its URL or made with `repo.create`. | DECISIONS row 346 names them by name and URL |
| Hub | The coordinator prompt fingerprint holds the channel's effective set. When the set changes, the next run of the channel ends an idle session and starts one with the new set. | DECISIONS row 349 |
| Hub | The harnesses `claude`, `codex`, `gemini` and `opencode` load skills themselves. Every other harness (`acp`, `pi` and others) gets `list_skills` and `read_skill`. | The delivery list of the plan |
| Hub | `skill_versions` keeps the file list of every version. `skill_bundle_grants` records each bundle that a `session.start` named to a worker. | data-model.md: "The hub keeps the file list of each version that a running session may still use", and protocol.md: the hub answers only for a named hash |
| Hub | `skill.update` with `accept_new_version` turns the skill off when its scripts changed, unless the same request turns it on. Turning on a blocked skill fails with `skill_blocked`. | protocol.md, and the risk rule for scripts |
| Hub | `Hub::store_skill` reads `name`, `description` and `license` from the `SKILL.md` front matter (plain, quoted and folded values). A skill with scripts starts off. A failed audit blocks the skill. It does not remove hooks yet. | The storage part of the install pipeline. The rest is step 3 |
| Hub | `search_connector_catalog` returns at most 25 services. | The result goes into the model's context |
| Hub | Turning off an item that the channel's scope does not give it is allowed and stores the exception. | It does no harm, and it keeps the item off if the scope later grows |

## Composio endpoints

The base is `https://backend.composio.dev/api/v3.1`. Every call sends the project key in `x-api-key`. The shapes come from the OpenAPI file at <https://docs.composio.dev/openapi.json> (version 3.1.0).

| Use | Call | Docs |
|---|---|---|
| Check a key | `GET /toolkits?limit=1` | <https://docs.composio.dev/reference/authenticating-to-composio> |
| Catalog | `GET /toolkits?sort_by=usage&limit=1000&cursor=...` (`items`, `next_cursor`; `meta.logo`, `meta.categories`, `meta.tools_count`, `auth_schemes`, `composio_managed_auth_schemes`) | OpenAPI `GET /toolkits` |
| One service | `GET /toolkits/{slug}` (`auth_config_details[].fields.connected_account_initiation`) | OpenAPI `GET /toolkits/{slug}` |
| Categories | `GET /toolkits/categories` | OpenAPI `GET /toolkits/categories` |
| Tools of a service | `GET /tools?toolkit_slug=...&include_deprecated=false&limit=1000` (`input_parameters`, `tags`) | OpenAPI `GET /tools`, <https://docs.composio.dev/docs/tool-tags> |
| One tool | `GET /tools/{tool_slug}` | OpenAPI `GET /tools/{tool_slug}` |
| Auth configs | `GET /auth_configs?toolkit_slug=...`, `POST /auth_configs` with `{"toolkit": {"slug"}, "auth_config": {"type": "use_composio_managed_auth"}}` or `{"type": "use_custom_auth", "authScheme": "API_KEY"}` | <https://docs.composio.dev/reference/api-reference/auth-configs> |
| Browser sign-in | `POST /connected_accounts/link` with `auth_config_id`, `user_id`, `alias` (answer: `redirect_url`, `connected_account_id`) | OpenAPI `POST /connected_accounts/link` |
| Sign-in with a key | `POST /connected_accounts` with `{"auth_config": {"id"}, "connection": {"user_id", "alias", "state": {"authScheme", "val": {"status": "ACTIVE", ...credentials}}}}` | <https://docs.composio.dev/docs/authentication/importing-existing-connections> |
| Account status | `GET /connected_accounts/{nanoid}` (`status`, `status_reason`) | OpenAPI `GET /connected_accounts/{nanoid}` |
| Disconnect | `DELETE /connected_accounts/{nanoid}` | OpenAPI `DELETE /connected_accounts/{nanoid}` |
| Run a tool | `POST /tools/execute/{tool_slug}` with `connected_account_id`, `user_id`, `arguments` (answer: `data`, `error`, `successful`, `log_id`). No `version`: v3.1 then runs the latest toolkit version | <https://docs.composio.dev/docs/migration-guide/toolkit-versioning> |
| Errors | `{"error": {"message", "code", "slug", "status", "suggested_fix"}}` | <https://docs.composio.dev/reference/errors> |

Facts that the docs do not confirm, for the live run (step 4):

- The exact strings of the hints in a tool's `tags`. The hub looks for `readOnlyHint`, `createHint`, `updateHint` and `destructiveHint` in any case.
- Whether Composio lets one `user_id` have two active accounts of one auth config (for example two Gmail accounts). The docs say the SDK needs `allow_multiple` for this, and the REST name of that flag is not documented. If the live run shows a refusal, each connection may need its own auth config or its own `user_id`.
- Whether the connected account answer ever holds an email. If it does not, `identity` stays empty.

## Proto changes

None.

## Dependencies

`aes-gcm` 0.10 (default features off, `aes` and `alloc`) in the workspace and the hub. It was in `Cargo.lock` already through `pgp`, so the lock file gains one line. The hub builds its query strings itself, because the workspace `reqwest` has no `query` feature.

## For the other agents

- **Worker.** `run_connector_tool` can take up to 4 minutes 45 seconds while the user answers a card. The harness's MCP tool timeout must allow that, or the assistant sees a timeout instead of `waiting_for_user`. The hub sends `extensions` now: connectors for every session with a connection, and skills for every session with a skill. `skill_bundle.get` answers only for a hash that a `session.start` to the same worker named.
- **Desktop and client.** A change that ran at once posts a notice with the kind `assistant_change` and a `text`. Hub cards come as approvals with the kinds `connector`, `management`, `connect_service`, `sign_in` and `skill_install`. A connect card that the user allows with `approval.answer` makes the hub connect with the card's settings. This works only for a browser scheme. For a scheme with a form, call `connection.connect` with the credentials and the `approval_id`.

## Left undone

- Step 3: `skill_directory.*`, `skill.preview`, `skill.install`, the update checker, `search_skills` and `add_skill`. They answer `unsupported`. `store_skill` does not remove Claude-specific hooks yet.
- The logo cache. `logo_url` is Composio's URL.
- `plan` and `usage` in `composio.status`.
- A new Composio key for another project does not move the old connections. Their accounts belong to the old project, so their calls fail until the user connects them again.
- `composio.status.error` stays empty. The hub does not record a key that Composio refuses later.
- Doc updates for the merge: protocol.md ("waits up to 5 minutes" is 4 minutes 45 seconds now, and the `assistant_change` notice), data-model.md (the logo cache, and the files `hub-secret.key` and `composio.key`), and the "Placeholders for step 2" list in the plan (only the step 3 entries are left).
