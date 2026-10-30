# Data model

This page lists the entities that the hub, the workers and the clients store, with their fields and state machines. The terms come from [CONTEXT.md](../CONTEXT.md). Wire formats are in [protocol.md](protocol.md).

## Where data lives

| Role | Store | Contents |
|---|---|---|
| Hub | `hub.db` (SQLite, WAL mode) | Every entity on this page except the worker-local state |
| Hub | `blobs/` | Attachments, channel files, artifacts, skill files and skill uploads, named by SHA-256 |
| Hub | `composio.key` | The Composio project key, encrypted with a key that the hub makes (mode 0600). See ADR 0011 |
| Hub | FTS5 tables in `hub.db` | Search index over messages, memory notes and task titles |
| Worker | `worker.toml` | Hub URL, tags, worker policy, harness accounts, local MCP servers, checkouts |
| Worker | `worker.db` (SQLite) | Event spool, processed command IDs, local session registry |
| Worker | `workspaces/` | Worktrees and clones that the worker made for tasks |
| Worker | `<consort home>/profiles/<name>/` | The profile folder of each account, named after the account name when the worker made it (`profile_dir` in `worker.toml`): the harness's own config and login, and `env.json` with the secret variables (mode 0600 on Unix) |
| Worker | `<consort home>/tools/<tool>/<version>/` | The managed tools that the worker installed |
| Worker | Each harness's own files | Raw session transcripts and harness logins. The worker never copies these to the hub |
| Client | `cache.db` (SQLite) | A partial copy of hub entities and the last feed sequence number |

The Consort home is `~/.consort` (`%USERPROFILE%\.consort` on Windows). `CONSORT_HOME` overrides it. `consort dev` sets it to `<data>/home` unless `CONSORT_HOME` is set.

The hub is the source of truth for everything except harness logins, secret variables and raw transcripts. It is also the source of connectors and skills: workers get a session's skills from the hub, and only the hub talks to Composio. A session's native state on a worker is a cache that the hub can rebuild from the timeline and memory.

On Unix the hub data folder and `blobs/` have mode 0700, and `hub.db` with its `-wal` and `-shm` files has mode 0600, also when they exist already. The hub writes each blob with mode 0600.

## Identifiers

Every ID is a UUIDv7 string with a type prefix, for example `tsk_0192f0c4-...`. UUIDv7 sorts by creation time. The UUID part is lowercase and hyphenated. Readers check the prefix. They accept any UUID version, so test fixtures can use fixed IDs.

| Prefix | Entity |
|---|---|
| `grp_` | Group |
| `chn_` | Channel |
| `msg_` | Message |
| `tsk_` | Task |
| `ses_` | Session |
| `itm_` | Session item |
| `apr_` | Approval |
| `dec_` | Decision |
| `rep_` | Repo |
| `cko_` | Checkout |
| `rtn_` | Routine |
| `wrk_` | Worker |
| `acc_` | Harness account (unique per worker) |
| `cli_` | Client |
| `sgn_` | Sign-in |
| `con_` | Connection |
| `skl_` | Skill |
| `ccl_` | Connector call (audit record) |

All timestamps are UTC, stored as Unix milliseconds. Hub tool arguments also accept RFC 3339 strings, because models write those more reliably.

Turn IDs, harness request IDs and native references are opaque strings that the harness adapter chooses.

## Conversation entities

### Group

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `name` | text | |
| `position` | integer | Sidebar order |
| `collapsed` | bool | Shared by all clients |

### Channel

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `kind` | `tutti` or `standard` | Exactly one `tutti` channel exists. It has no group and cannot be archived or deleted |
| `name` | text | |
| `icon`, `color` | text | |
| `group_id` | ID, optional | |
| `position` | integer | Order inside the group |
| `purpose` | text, one line | |
| `instructions` | markdown | |
| `coordinator_selection` | model selection | Falls back to the Tutti channel's value |
| `task_selection` | model selection | Default for new tasks in this channel |
| `preferred_workers` | list of worker IDs | Ordered. Placement tries these first |
| `repo_ids` | list of repo IDs | The `channel_repos` join, in order. Part of the client view |
| `default_isolation` | `in_place`, `worktree` or `clone` | A task that the assistant starts runs in place only when this is `in_place` |
| `default_approval_mode` | `ask`, `auto_edits`, `auto` or `full_access` | Also the most that a task that the assistant starts gets |
| `allowed_local_mcp_servers` | list of text | The local MCP servers, by name, that a task that the assistant starts may use. Empty by default. A task that the user starts is not limited by it |
| `memory_enabled` | bool | |
| `tags` | list of text | Requirements. A coordinator turn, a task or a routine run of the channel runs only on a worker that has every tag. Empty by default |
| `disabled_connections` | list of connection IDs | Channel exceptions: connections that the channel turned off. Empty by default |
| `disabled_skills` | list of skill IDs | Channel exceptions: skills that the channel turned off. Empty by default |
| `archived_at` | timestamp, optional | |
| `created_at`, `updated_at` | timestamp | |

Stored data from before hub migration 6 says `everything` for this kind. The migration rewrites it to `tutti`, and the code still reads `everything` as an alias. See [ADR 0012](adr/0012-the-main-channel-is-called-tutti.md).

### Repo and channel repo

A repo is global, so two channels can attach the same repo.

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `url` | text | Git remote URL, normalized |
| `name` | text | |
| `default_branch` | text | |

`channel_repos` joins a channel to its repos: `channel_id`, `repo_id`, `position`.

### Message

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `channel_id` | ID | |
| `thread_root_id` | ID, optional | Empty for timeline messages |
| `author` | `user`, `assistant` or `system` | |
| `author_session_id` | ID, optional | The coordinator or task session that posted it |
| `author_task_id` | ID, optional | Set when a task posted it |
| `author_routine_id` | ID, optional | Set when a routine caused it |
| `kind` | `text`, `task_card`, `decision`, `approval` or `notice` | |
| `body` | markdown | For `notice`, a JSON object with a `kind` field |
| `attachments` | list of blob references | |
| `in_timeline` | bool | True for a thread reply that also shows in the timeline |
| `client_message_id` | text, optional | Idempotency key from the client. Unique per client |
| `reply_count`, `last_reply_at` | integer, timestamp | Set on thread roots only |
| `reactions` | list | The client view of `reactions`: `author`, `emoji`, `created_at` |
| `edited_at`, `deleted_at` | timestamp, optional | |
| `created_at` | timestamp | |

`reactions` holds `message_id`, `author`, `emoji` and `created_at`.

### Read marker

The hub has one user, so a message that is read on one client is read on all of them.

| Field | Type | Notes |
|---|---|---|
| `channel_id` | ID | |
| `thread_root_id` | ID, optional | Empty for the timeline |
| `last_read_message_id` | ID | |

## Work entities

### Task

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `channel_id` | ID | |
| `title` | text | |
| `brief` | markdown | The first instruction to the task session |
| `status` | task status | See [Task status](#task-status) |
| `status_reason` | text, optional | For example "Waiting for bigboi to come online" |
| `created_by` | `user`, `assistant` or `routine` | |
| `card_message_id` | ID | The task card in the timeline. The task thread hangs from this message |
| `model_selection` | model selection | |
| `placement` | placement request | See below |
| `approval_mode` | approval mode | |
| `start_at` | timestamp, optional | Set for Scheduled tasks |
| `current_session_id` | ID, optional | |
| `worker_id` | ID, optional | The worker of the current session, so the task card can show when it is offline |
| `review_summary` | markdown, optional | The text the task gave when it went to Ready for review |
| `resolved_at`, `archived_at` | timestamp, optional | Archive hides a task in any status |
| `created_at`, `updated_at` | timestamp | |

A placement request has these fields:

| Field | Type | Notes |
|---|---|---|
| `worker_id` | ID, optional | A fixed worker. Placement fails clearly if it cannot use this worker |
| `require` | object, optional | Constraints: `os`, `tags`, `local_mcp_servers`. The channel's tags apply too |
| `repos` | list of repo IDs | Defaults to the channel repos |
| `isolation` | isolation | Defaults to the channel value |

### Task status

| Status | UI tab | Meaning |
|---|---|---|
| `scheduled` | Scheduled | `start_at` is in the future |
| `up_next` | Up next | Waiting for placement or for a worker to come online |
| `working` | Active | A session is running a turn |
| `needs_input` | Active | One or more approvals or decisions are pending |
| `review` | Active, under Ready for review | The session finished its turn and waits for the user |
| `failed` | Active | The session cannot continue. `status_reason` says why |
| `resolved` | Resolved | The user marked the task done |

Allowed transitions:

| From | To | Cause |
|---|---|---|
| (new) | `scheduled` | Created with a future `start_at` |
| (new) | `up_next` | Created without `start_at` |
| `scheduled` | `up_next` | `start_at` passed |
| `up_next` | `working` | Placement found a worker and the session started |
| `working` | `needs_input` | An approval or a decision opened |
| `needs_input` | `working` | The last pending approval or decision closed |
| `working` | `review` | The turn ended with nothing pending, or the task called `submit_for_review`. Also when the worker refused the turn of a message that moved the task from `review` |
| `review` | `working` | The user or the coordinator sent the task a message |
| `review` | `resolved` | The user resolved the task. No session can do this |
| `resolved` | `review` | The user chose **Not done**. The hub refuses it when archive deleted the workspace of the task |
| `up_next`, `working`, `needs_input`, `review` | `failed` | The session was lost and cannot resume, the harness failed, or the fixed worker was removed |
| `failed` | `up_next` | The user or the coordinator chose retry |
| `review`, `resolved` | `up_next` | The user or the coordinator chose retry after the session of the task ended. The hub refuses it while the session runs |

A task whose worker goes offline stays in its current status. The task card shows the worker as offline. The worker keeps the session running and sends the buffered events when it reconnects.

A turn that ends while the task is in `needs_input` and nothing waits moves the task on as any turn end does: to `working`, and then to `review` when nothing else comes. The hub keeps hub-only records next to a task: when it asked the workers to delete the workspace, and whether the open turn came from `review`.

### Session

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `role` | `coordinator` or `task` | |
| `channel_id` | ID | |
| `task_id` | ID, optional | Set when `role` is `task` |
| `worker_id` | ID | A session never moves to another worker |
| `account_id` | ID | Harness account on that worker |
| `harness` | text | `claude`, `codex`, `pi`, `opencode`, `gemini`, `acp:<registry id>` for an ACP registry agent, `acp:custom` for a custom ACP command, or `acp` for an ACP account from `worker.toml` |
| `model`, `options` | text, object | |
| `workspace_path` | text | Path on the worker |
| `native_ref` | text, optional | The harness's own session ID, used to resume |
| `state` | `starting`, `active`, `idle`, `ended` or `lost` | |
| `last_seq` | integer | Highest event sequence number that the hub stored |
| `usage` | object | Input, output and cached tokens, and cost when the harness reports it |
| `started_at`, `ended_at` | timestamp | |
| `end_reason` | text, optional | |

### Session item

The hub stores a normalized copy of what a session did. An update with the same `id` replaces the earlier copy.

| Field | Type | Notes |
|---|---|---|
| `id` | ID | Unique per session |
| `session_id` | ID | |
| `turn_id` | text | |
| `kind` | text | `assistant_message`, `reasoning`, `command`, `file_change`, `tool_call`, `plan`, `subagent`, `web_search` or `error` |
| `status` | `in_progress`, `completed` or `failed` | |
| `payload` | JSON | Command output keeps only the last 8 KiB. File changes keep paths and line counts. The full data stays on the worker |
| `created_at`, `updated_at` | timestamp | |

The payload for each kind:

| Kind | Payload |
|---|---|
| `assistant_message`, `reasoning` | `text` |
| `command` | `command`, `cwd`, `output` (the last 8 KiB), `output_truncated`, `exit_code`, `duration_ms` |
| `file_change` | `changes`, each with `path`, `change` (`add`, `modify`, `delete` or `rename`), `from_path`, `lines_added`, `lines_removed` |
| `tool_call` | `server`, `tool`, `arguments`, `output`, `is_error` |
| `plan` | `steps`, each with `text` and `status` (`pending`, `in_progress` or `completed`) |
| `subagent` | `agent`, `description`, `result` |
| `web_search` | `query`, `urls` |
| `error` | `message`, `code` |

### Approval

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `channel_id` | ID | |
| `session_id` | ID | |
| `task_id` | ID, optional | Empty when a coordinator session asked |
| `message_id` | ID | The approval card in the task thread |
| `request_id` | text | The harness request. `session.respond` sends it back |
| `summary` | text | One line for the card and the push notification |
| `details` | JSON | Has a `kind`: `command`, `file_change`, `tool`, `network` or `other` from a harness, or a hub card kind: `connector`, `management`, `connect_service`, `sign_in` or `skill_install`. The other fields depend on the kind: command and folder, paths, tool name and arguments, or the facts of the card (see [Hub cards](#hub-cards)) |
| `options` | list | From the harness, for example allow once, allow for this session, deny. Each has `id`, `label`, `kind` and `description` |
| `status` | `pending`, `allowed`, `denied`, `expired` or `canceled` | |
| `answer_option_id` | text, optional | The option that the user chose |
| `answered_at`, `answered_by_client_id` | timestamp, ID | |
| `created_at` | timestamp | |

### Hub cards

A hub card is an approval that the hub makes itself, not a harness. Its `request_id` is `hub:<approval id>`, its options are `allow` and `deny`, and the hub carries out the answer. It never sends `session.respond` for it. The hub refuses a hub card kind in a `request.opened` from a worker, and reads it as `other`.

| `kind` | Card | Fields | Allow | Deny |
|---|---|---|---|---|
| `connector` | Connector approval | `connection_id`, `connection_name`, `service_name`, `tool`, `tool_kind`, `fields` (each `label` and `value`, such as To, Subject and Body), `arguments` | Allow: the call runs | Don't |
| `management` | Management approval card | `tool`, `change`, `effect` (for example "GitHub Work becomes available in 4 more channels"), `arguments` | Allow: the hub makes the change | Don't |
| `connect_service` | Connect card | `service`, `service_name`, `logo_url`, `name`, `use_in`, `approvals`, and `connection_id` after the user continued | Continue in browser | Not now |
| `sign_in` | Sign-in card | `worker_id`, `account_id`, `account_label`, `harness`, `method`, and `sign_in_id` after the user started | Sign in | Not now |
| `skill_install` | Install card | `name`, `description`, `source`, `commit`, `license`, `scripts`, `audits`, `use_in`, `blocked_reason`, and `skill_id` after the user added it | Add | Not now |

A hub card in a task makes the task Needs input, as any approval does. When a card that a coordinator posted closes, the hub wakes the coordinator with a `card_answered` event (see [protocol.md](protocol.md#coordinator-turn-input)).

### Decision

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `channel_id` | ID | |
| `task_id` | ID, optional | |
| `session_id` | ID | The session that asked |
| `message_id` | ID | The decision card |
| `request_id` | text, optional | Set when the decision comes from a harness question. The hub then sends the answer with `session.respond` |
| `question` | text, at most 1,200 characters | |
| `context` | markdown, optional | |
| `options` | 2 to 4 objects | Each has `label` (at most 40 characters) and `consequence` (at most 800 characters). An option from a harness question also has the harness's `id` |
| `recommended_index` | integer, optional | |
| `allow_free_text` | bool | |
| `status` | `pending`, `answered`, `expired` or `canceled` | |
| `answer` | object, optional | `option_index`, `text` or both |
| `answered_at` | timestamp, optional | |
| `created_at` | timestamp | |

### Routine

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `channel_id` | ID | |
| `name` | text | |
| `schedule` | object | Has a `kind`. One of: `cron` with `expression` (five fields) and `time_zone`, `interval` with `seconds` (at least 60), or `once` with `at` |
| `prompt` | markdown | |
| `action` | object | Has a `kind`: `coordinator_turn`, or `start_task` with `title`, `model_selection` and `placement` |
| `enabled` | bool | |
| `next_run_at`, `last_run_at` | timestamp | |
| `last_run_status` | `ok` or `error` | |

`routine_runs` keeps one row per run: `routine_id`, `started_at`, `message_id` or `task_id`, and `error`.

## Knowledge entities

### Memory note

| Field | Type | Notes |
|---|---|---|
| `scope` | `global` or a channel ID | The Tutti channel's memory is the global memory |
| `path` | text | For example `people/kevin.md` |
| `content` | markdown | |
| `sha256` | text | Hash of `content` |
| `version` | integer | |
| `updated_by` | session ID or `user` | |
| `updated_at` | timestamp | |

`memory_note_versions` keeps every earlier version.

Every write states the hash that it expects, or `not_exists` for a new note. If the stored hash differs, the hub rejects the write with `stale_write`.

| Writer | Reads | Writes |
|---|---|---|
| Tutti coordinator | All scopes | `global` |
| Channel coordinator | `global` and its own channel | Its own channel. It can suggest a global note to the Tutti coordinator |
| Task session | `global` and its channel | Nothing. It can suggest a note to its channel coordinator |
| User, from a client | All scopes | All scopes |

A suggestion goes one level up: from a task to its channel coordinator, and from a channel coordinator to the Tutti coordinator. The coordinator that gets a suggestion decides whether to write the note. The hub never writes a suggestion to memory by itself.

### Channel file and artifact

| Field | Type | Notes |
|---|---|---|
| `channel_id` | ID | |
| `path` | text | |
| `blob_sha256` | text | |
| `size`, `mime` | integer, text | |
| `task_id` | ID, optional | Set on artifacts |
| `title` | text, optional | Set on artifacts |
| `updated_by`, `updated_at` | text, timestamp | |
| `deleted_at` | timestamp, optional | |

An artifact is a channel file with a `task_id`. Tasks publish artifacts under `artifacts/<task-id>/`.

## Machine entities

### Worker

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `name` | text | Set by the user |
| `hostname` | text | |
| `os` | `macos`, `windows` or `linux` | |
| `arch` | text | |
| `public_key` | Ed25519 public key | |
| `version`, `protocol_version` | text, integer | |
| `tags` | list of text | User tags, set in a client, for example `work` or `gpu`. Placement matches a tag from this list or from the inventory, which has the detected tags and the tags from `worker.toml`. Stored JSON from before the rename has `labels`, which reads as `tags` |
| `paused` | bool | Set by the user. Placement skips a paused worker. Running sessions continue |
| `state` | `online` or `offline` | |
| `last_seen_at` | timestamp | |
| `inventory` | JSON | The newest inventory snapshot. See [protocol.md](protocol.md#inventory) |
| `paired_at`, `revoked_at` | timestamp | |

### Harness account

The worker owns the account configuration and the login. The hub stores only what the worker reports in its inventory. The user makes, renames and removes accounts from a client (`account.create`, `account.update`, `account.remove`). An account that Consort made has its own profile folder, so two accounts of one provider on one worker never share a login. A new worker has no accounts. Accounts that a user wrote in `worker.toml` by hand still load.

| Field | Type | Notes |
|---|---|---|
| `worker_id`, `account_id` | ID | |
| `harness` | text | |
| `label` | text | For example "Claude Personal". Accounts with the same label on different workers count as one account for a model selection |
| `harness_version` | text | |
| `auth_state` | `signed_in`, `signed_out`, `error` or `unknown` | |
| `auth_detail` | text, optional | A detail such as the plan name, or the command to sign in |
| `models` | list | Each model has an ID, a name and option descriptors (select or boolean) |
| `capabilities` | object | `steer`, `resume`, `fork`, `images`, `plan_mode`, `usage`, `mcp_http` and the approval modes it supports |
| `enabled` | bool | |
| `profile` | bool | True for an account that Consort made, with a profile folder |
| `email`, `plan` | text, optional | Who is signed in and on which plan, when the harness tells |
| `secret_names` | list of text | The names of the variables in `env.json`. Never the values |
| `sign_in_methods` | list | The methods that a client may offer. See [protocol.md](protocol.md#sign-in) |
| `extras_enabled` | bool | Whether the harness starts with its provider extras. True by default, and for an inventory from an older worker |
| `extras` | object, optional | What the account syncs from the provider: `mcp_servers`, `plugins` and `skills`, each with a `count` and at most 20 short `names`, and `cannot_turn_off`, the extras that the worker cannot turn off, in words. Never a secret |

### Sign-in

One run of a provider's sign-in flow for one account. A feed entity with no channel. The hub stores it in the `sign_ins` table (`id`, `worker_id`, `updated_at`, `data`). `updated_at` copies `SignIn.updated_at` in Unix milliseconds, so the hub can delete old rows without reading every document.

| Field | Type | Notes |
|---|---|---|
| `id` | ID | `sgn_` |
| `worker_id`, `account_id` | ID | |
| `method` | object | `kind` and its fields. See [protocol.md](protocol.md#sign-in) |
| `state` | object | `kind` and its fields: `starting`, `open_url`, `device_code`, `needs_code`, `verifying`, `succeeded`, `failed`, `cancelled` or `expired` |
| `created_at`, `updated_at` | timestamp | |
| `expires_at` | timestamp | 15 minutes after the last update. Then the state is `expired` |

The hub deletes a sign-in one day after its last update. A sign-in never holds a code or a secret.

### Managed tool

A value in the inventory, not a row: `tool`, `version`, `latest`, `state` and `detail`. See [protocol.md](protocol.md#inventory).

### Model selection

A model selection is a value, not a row: `account_label`, `model` and `options`. Placement turns it into a worker and an account ID on that worker.

### Checkout

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `repo_id` | ID | |
| `worker_id` | ID | |
| `path` | text | |
| `managed` | bool | True when the worker made the clone itself |
| `last_seen_at` | timestamp | |

### Client

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `name` | text | |
| `kind` | `desktop`, `ios` or `android` | |
| `public_key` | Ed25519 public key | |
| `push` | object, optional | Provider (`apns`, `fcm` or `expo`) and token. The client view has only `push_provider`. The token stays on the hub |
| `paired_at`, `last_seen_at`, `revoked_at` | timestamp | |
| `paired_by`, `revoked_by` | text, optional | Hub only, not in the client view. A client ID, or `hub` for the hub machine. `paired_by` is who made the pairing code that the client used. `consort hub client list` shows both |

A worker has the same `paired_by` and `revoked_by`. Each pairing code records who made it in `created_by`. When a client is removed, the codes that it made and that are not used yet are deleted.

## Connector and skill entities

See ADR 0011 for why the hub owns these.

### Connection

One connected account of one Composio service. A feed entity with no channel (`connection`).

| Field | Type | Notes |
|---|---|---|
| `id` | ID | `con_` |
| `name` | text | For example "Gmail Work". At most 80 characters |
| `handle` | text | The assistant's name for it: a unique slug of the name, such as `gmail_work`. At most 40 lowercase ASCII letters, digits and `_`. A repeat gets `_2`, `_3` |
| `service`, `service_name` | text | The Composio toolkit slug (`gmail`) and its name (`Gmail`) |
| `logo_url` | text, optional | From the hub's logo cache |
| `identity` | text, optional | The account's email address or login, when the service tells |
| `use_in` | object | `kind`: `every_channel`, `tags` with `tags` (a channel needs all of them), or `channels` with `channel_ids` |
| `approvals` | object | `read`, `change` and `delete`, each `run_without_asking`, `ask_once_per_task` or `ask_every_time`. The defaults are run without asking, ask every time and ask every time |
| `status` | object | `kind`: `connecting` with `redirect_url` and `expires_at`, `active`, `needs_sign_in` with `message`, or `failed` with `message` |
| `last_used_at` | timestamp, optional | |
| `created_at`, `updated_at` | timestamp | |

A connect attempt is not an entity of its own. `connection.connect` makes the connection in `connecting`, and each change of the status is a feed upsert. The hub waits 10 minutes, then sets `failed`. The Composio connected account ID stays on the hub and is not in the client view.

The tool kind comes from Composio's hints: `destructiveHint` gives `delete`, a tool with only `readOnlyHint` gives `read`, and every other tool gives `change`.

### Connector call

One row of the audit log, kept on the hub. Not a feed entity. A client reads it with `connection.calls`.

| Field | Type | Notes |
|---|---|---|
| `id` | ID | `ccl_` |
| `connection_id` | ID | |
| `channel_id`, `session_id`, `task_id` | ID, optional | |
| `tool`, `tool_kind` | text | |
| `arguments_sha256` | text | The SHA-256 of the canonical JSON of the arguments (keys sorted, no spaces). The hub never stores the arguments or the result |
| `decision` | text | `ran_without_asking`, `allowed_by_user`, `allowed_earlier_in_task`, `denied_by_user` or `expired` |
| `outcome` | text | `succeeded`, `failed` or `not_run` |
| `error`, `composio_log_id` | text, optional | |
| `result_bytes`, `duration_ms` | integer | |
| `at` | timestamp | |

### Skill

An installed skill. A feed entity with no channel (`skill`).

| Field | Type | Notes |
|---|---|---|
| `id` | ID | `skl_` |
| `name`, `description` | text | From `SKILL.md`. The name is 1 to 64 lowercase letters, digits and `-`, and is unique on the hub. The description has at most 1,024 characters |
| `source` | object | `kind`: `git` with `url`, `path` and `ref`, `directory` with `directory`, `entry_id`, `url` and `path`, or `upload` with `file_name` |
| `commit` | text, optional | The pinned commit. Empty for an upload |
| `content_hash` | text | The SHA-256 of one line per file, sorted by path: `<path>\0<sha256>\0<x or ->\n`, where `x` marks an executable file |
| `license` | text, optional | |
| `scripts` | list of text | Files that can run: files with the executable bit and files under `scripts/` |
| `audits` | list | Each has `source`, `status` (`passed`, `warning`, `failed` or `unknown`), `summary` and `url` |
| `enabled` | bool | A skill with scripts stays off until the user turns it on |
| `use_in` | object | As for a connection |
| `update` | object | `kind`: `up_to_date`, `available` with `commit`, `content_hash`, `changes`, `scripts_changed` and `found_at`, or `check_failed` with `message` |
| `blocked_reason` | text, optional | Set when the hub blocks it, for example after a failed audit |
| `hooks_removed` | bool | True when the hub removed Claude-specific `hooks` |
| `file_count`, `size_bytes` | integer | At most 500 files, 5 MiB for one file and 20 MiB together |
| `installed_at`, `updated_at` | timestamp | |

The files of each version are blobs. The hub keeps the file list of each version that a running session may still use.

### Effective set

A session gets the connections and skills whose `use_in` matches its channel, minus the channel's exceptions. A skill must also be on and not blocked. The Tutti channel matches only `every_channel`, unless it has tags of its own.

## Preferences

The user's preferences are one record that every client shares. The hub stores it as JSON in the `meta` table, under the key `preferences`, so it needs no table of its own. A hub that has no record yet has empty preferences.

| Field | Type | Notes |
|---|---|---|
| `favorite_models` | list of `{account_label, model}` | The favorite models of the model picker, in the order that the user starred them. At most 200. Each field has at most 200 characters |

A favorite whose account is not signed in on any worker, or whose account no longer offers the model, stays in the list. The clients do not show it until the account or the model comes back. The feed `entity_id` of the record is `preferences`, and its entries have no channel.

## Feed

Every write on the hub adds one feed entry in the same database transaction. Clients use the feed to stay in sync.

| Field | Type | Notes |
|---|---|---|
| `seq` | integer | Increases by one for each entry, across the whole hub |
| `at` | timestamp | |
| `channel_id` | ID, optional | Lets a client filter by channel |
| `entity` | text | `channel`, `group`, `repo`, `message`, `read_marker`, `task`, `session_item`, `approval`, `decision`, `memory_note`, `channel_file`, `routine`, `worker`, `client`, `sign_in`, `preferences`, `connection` or `skill` |
| `entity_id` | text | The entity's ID. A memory note uses `<scope>:<path>`, a channel file `<channel-id>:<path>`, a read marker `<channel-id>` or `<channel-id>:<thread-root-id>`, a session item `<session-id>:<item-id>`, and the preferences `preferences` |
| `op` | `upsert` or `delete` | |
| `payload` | JSON | The full client view of the entity after the write. `null` for `delete` |

The hub merges updates to the same session item that arrive within 250 ms into one feed entry. The hub deletes feed entries after 30 days. A client whose last `seq` is older than the oldest kept entry gets a `resync_required` error and reloads its snapshots.

## Worker-local state

`worker.toml` holds the configuration that the user owns on that machine:

```toml
hub_url = "https://hub.example.com"
name = "bigboi"
tags = ["windows-test", "gpu"]

[policy]
allowed_roots = ["C:/Users/chris/Code"]
max_approval_mode = "auto_edits"
allow_shell = true

[[accounts]]
id = "acc_0192f0c4-..."
harness = "codex"
label = "Codex Personal"
binary = "codex"
home = "~/.codex"
shadow_home = "~/.local/share/app/homes/codex-personal"
args = []
env = {}
enabled = true

# An account that Consort made. Its home folders are in the profile folder
# <consort home>/profiles/claude-personal/.
[[accounts]]
id = "acc_0192f0c5-..."
harness = "claude"
label = "Claude Personal"
profile = true
profile_dir = "claude-personal"

[[local_mcp_servers]]
name = "home-assistant"
transport = "http"
url = "http://192.168.1.20:8123/mcp"

[[checkouts]]
repo_url = "git@github.com:watzon/sayso.git"
path = "C:/Users/chris/Code/sayso"
```

`worker.db` holds:

- `spool`: session ID, sequence number, event and an acknowledged flag. The worker keeps events until the hub acknowledges them.
- `processed_commands`: command ID, result and time, kept for 8 days (the 7 days that the hub keeps a queued command, and one more day) so that a repeated command runs once.
- `sessions`: the local registry of sessions with their native references and workspace paths.
- `harness_processes`: the process group of each harness that the worker started on Unix, with the start time of its leader. At the next start, the worker stops only the recorded groups whose start time still matches.
- `account_settings`: one row per account with `extras_enabled`. A missing row means on. The worker deletes the row when it removes the account.

On Unix the worker folder, `workspaces/` and `runtime/` have mode 0700, and `worker.db` has mode 0600.
