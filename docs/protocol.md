# Protocol

This page is the reference for the three links in the system. Entity fields are in [data-model.md](data-model.md). Terms are in [CONTEXT.md](../CONTEXT.md).

| Link | Between | Transport | Who connects |
|---|---|---|---|
| Client API | Client and hub | WebSocket over TLS, plus HTTPS for blobs and pairing | The client |
| Worker link | Worker and hub | WebSocket over TLS, plus HTTPS for blobs and pairing | The worker |
| Hub tools | Session and hub, through the session's worker | MCP over local HTTP on the worker, then `tool.call` on the worker link | The harness |

The hub never opens a connection to a worker or a client. Workers and clients behind NAT need only outbound HTTPS.

## Common rules

### Frames

Each WebSocket text frame holds one JSON object. Binary frames are not used.

A request:

```json
{ "v": 1, "id": "01JC5...", "type": "task.create", "body": { } }
```

A response:

```json
{ "v": 1, "re": "01JC5...", "ok": true, "body": { } }
{ "v": 1, "re": "01JC5...", "ok": false, "error": { "code": "worker_offline", "message": "bigboi is offline." } }
```

A message that needs no response has a `type` and no `id`.

`v` is the frame format version. The protocol version is agreed in the `hello` exchange. A frame larger than 1 MiB is a protocol error. Large data goes through the blob endpoints.

### Versions

Each side sends the protocol versions that it supports in `hello`. The hub picks the highest common version. If no common version exists, the hub closes the connection with `protocol_mismatch`.

### Error codes

| Code | Meaning |
|---|---|
| `unauthorized` | The key, signature or token is not valid, or the worker or client is revoked |
| `protocol_mismatch` | No common protocol version |
| `invalid` | The body does not match the schema. `message` names the field |
| `not_found` | The entity does not exist |
| `stale_write` | The expected memory note hash does not match the stored hash |
| `worker_offline` | The worker that this action needs is offline |
| `worker_unknown` | No worker has this ID |
| `no_matching_account` | No online worker has a harness account with this label and model |
| `policy_denied` | The worker policy does not allow this action. `message` names the rule |
| `resync_required` | The client's feed position is older than the oldest kept feed entry |
| `conflict` | The action does not apply to the entity's current state, for example resolving a task that is `up_next` |
| `internal` | An unexpected error on the hub. The hub logs the details |

## Pairing and authentication

Workers and clients pair the same way, and both prove their identity with an Ed25519 key.

### Pair a worker

1. In a client, the user chooses **Add worker**. The hub makes a pairing code: 8 characters, single use, valid for 10 minutes.
2. On the new machine, the user runs `app worker pair https://hub.example.com --code ABCD-EFGH`.
3. The worker makes an Ed25519 key pair. The private key stays in the OS keychain, or in a file with mode 0600 when no keychain is available.
4. The worker sends `POST /v1/pair/worker` with the code, the public key, the hostname, the OS and the architecture.
5. The hub checks the code, stores the worker, and returns the `worker_id`.
6. The worker writes `hub_url` and its ID to `worker.toml`.

### Pair a client

The first client uses the setup code that the hub prints on its first start. To pair a later client, a paired client shows a QR code that holds the hub URL and a new pairing code. The new client then follows the worker steps with `POST /v1/pair/client`.

### Connect

1. The worker opens `wss://<hub>/v1/worker`. A client opens `wss://<hub>/v1/client`.
2. The hub sends `auth.challenge` with a 32-byte random `nonce`.
3. The peer sends `auth.response` with its ID and an Ed25519 signature over the nonce and the hub origin.
4. The hub checks the signature against the stored public key and continues with `hello`.

A client that needs the blob endpoints sends `auth.http_token` and gets a bearer token that is valid for 10 minutes.

## Worker link

### Worker to hub

| Type | Body | Response |
|---|---|---|
| `hello` | `worker_id`, `version`, `protocol_versions`, `sessions` (each with `session_id`, `last_seq` and `state`) | `protocol_version` and the hub's stored `last_seq` for each session |
| `inventory` | See [Inventory](#inventory) | None |
| `session.events` | `session_id` and `events` (each with `seq`, `at` and `event`) | `ack` with the highest contiguous `seq` that the hub stored |
| `tool.list` | `session_id` | The hub tools for that session's role: each tool's `name`, `description` and `input_schema` |
| `tool.call` | `session_id`, `call_id`, `name`, `arguments` | `content` and `is_error` |

### Hub to worker

The envelope `id` of a hub command is its command ID. The worker stores the result of each command ID for 24 hours. If the same command arrives again, the worker returns the stored result and does not run the command again.

| Type | Body | Result |
|---|---|---|
| `session.start` | `session_id`, `role`, `account_id`, `model`, `options`, `approval_mode`, `workspace` (see below), `system_prompt_append`, `allowed_local_mcp_servers`, `channel_files` (a list of paths and hashes), optional `resume` with a `native_ref`, optional first `turn` | `workspace_path` and optional `native_ref` |
| `session.turn` | `session_id`, `turn` (`text` and `attachments`) | `turn_id` |
| `session.steer` | `session_id`, `turn` | None. The worker sends `policy_denied` if the harness cannot steer |
| `session.interrupt` | `session_id` | None |
| `session.respond` | `session_id`, `request_id`, `answer` | None |
| `session.stop` | `session_id`, `reason` | None |
| `workspace.release` | `task_id`, `delete` (bool) | None |
| `item.detail` | `session_id`, `item_id` | A blob hash with the full command output or diff |
| `inventory.refresh` | Empty | None |

The `workspace` object of `session.start` has these fields:

| Field | Notes |
|---|---|
| `repos` | A list. Each repo has `repo_id`, `url`, optional `branch` and optional `checkout_path` |
| `isolation` | `in_place` runs in the checkout. `worktree` makes a git worktree from the checkout's `HEAD`. `clone` makes a fresh clone in `workspaces/` |
| `carry_uncommitted` | Copy uncommitted changes from the checkout into the worktree. Allowed only with `worktree` |

### Session events

The `event` field of a session event is one of these objects:

| `type` | Fields | Notes |
|---|---|---|
| `state` | `state` | `starting`, `active`, `idle`, `ended` or `lost` |
| `native_ref` | `ref` | The harness's own session ID |
| `turn.started` | `turn_id` | |
| `item` | A full session item | Replaces any earlier item with the same `id` |
| `output.delta` | `item_id`, `text` | Command output. The worker merges deltas into at most one event every 250 ms |
| `request.opened` | `request_id`, `kind` (`approval` or `question`), `summary`, `details`, `options` | The harness waits for `session.respond` |
| `request.closed` | `request_id`, `outcome` | |
| `usage` | `input_tokens`, `output_tokens`, `cached_tokens`, optional `cost_usd` | Totals for the session |
| `turn.ended` | `turn_id`, `reason` (`completed`, `interrupted` or `failed`), optional `error` | |
| `error` | `code`, `message` | |

### Delivery rules

- The worker numbers each session's events from 1, with no gaps.
- The worker writes every event to its spool before it sends the event. The worker deletes an event from the spool after the hub acknowledges it.
- The worker sends batches of at most 256 events or 256 KiB.
- On reconnect, `hello` lists the open sessions. The hub answers with its stored `last_seq` for each session, and the worker sends every event after that number.
- The hub keeps commands for an offline worker in a per-session queue and sends them in order on reconnect. A queued command expires after 7 days.
- The worker sends a WebSocket ping every 20 seconds. The hub marks the worker offline after 60 seconds with no frames.

### Inventory

The worker sends a full inventory after `hello` and again when any part changes.

| Field | Notes |
|---|---|
| `hash` | Hash of the rest of the inventory |
| `hostname`, `os`, `arch`, `labels` | |
| `accounts` | Each harness account: `account_id`, `harness`, `label`, `harness_version`, `auth_state`, `models` and `capabilities`. Never a credential |
| `local_mcp_servers` | Each server's `name`, `description` and tool names. Never a URL with a secret, and never a credential |
| `checkouts` | Each checkout's `repo_url`, `path` and `managed` flag |
| `policy` | `max_approval_mode`, `allowed_roots` and `allow_shell` |
| `load` | `active_sessions` |

### Blobs

`PUT /v1/blobs/{sha256}` uploads a blob. `GET /v1/blobs/{sha256}` downloads it. Workers authenticate with a signature header. Clients authenticate with the token from `auth.http_token`. The hub rejects an upload whose content does not match the hash.

At `session.start`, the worker downloads the channel files that it does not have and puts a read-only copy in `<workspace>/.channel/files/`.

## Hub tools

The worker runs one MCP endpoint per session at `http://127.0.0.1:<port>/mcp/<session_id>`, with streamable HTTP transport. The worker makes a random 32-byte bearer token for each session and gives it only to that session's harness.

The worker is the MCP server. It gets the tool list from the hub with `tool.list` when the session starts, and it sends each tool call to the hub as `tool.call`. The hub runs the tool logic but does not speak MCP. The hub knows the session from the authenticated worker link, so the token never leaves the worker.

The worker answers part of `publish_artifact` itself: it uploads the file from the workspace, then sends the call to the hub with the blob hash added.

The hub gives each session the tools for its role.

### Tools for every session

| Tool | Arguments | Result |
|---|---|---|
| `memory_list` | optional `scope` | Paths, hashes and update times |
| `memory_read` | optional `scope`, `path` | `content` and `sha256` |
| `files_list` | optional `prefix` | Channel file paths, sizes and hashes |
| `list_workers` | None | Each worker's ID, name, OS, state, labels, account labels with their models, local MCP server names and repo checkouts |
| `ask_decision` | `question`, `options` (2 to 4, each with `label` and `consequence`), optional `recommended_index`, `context` and `allow_free_text` | `decision_id`. The answer arrives as input to a later turn |
| `search` | `query`, optional `channel_id` | Matching messages, memory notes and tasks |

### Tools for coordinators

| Tool | Arguments | Result |
|---|---|---|
| `post_message` | `body`, optional `attachments` | `message_id` |
| `reply` | `message_id`, `body`, optional `in_timeline` | `message_id` |
| `update_message` | `message_id`, `body` | Only for the session's own messages |
| `react` | `message_id`, `emoji` | |
| `no_reply_needed` | None | Ends the turn with no visible reply |
| `fetch_timeline` | optional `before`, `limit` | Messages |
| `fetch_thread` | `message_id` | The root and its replies |
| `memory_write` | `path`, `content`, `expected_sha256` or `"not_exists"` | New `sha256`, or `stale_write` |
| `memory_delete` | `path`, `expected_sha256` | |
| `files_write` | `path`, `content` | Text files only |
| `start_task` | `title`, `brief`, optional `model_selection`, `worker_id`, `require`, `repos`, `isolation`, `approval_mode` and `start_at` | `task_id`, `status` and optional `status_reason` |
| `message_task` | `task_id`, `body` | Starts a turn, or steers the running turn when the harness can steer |
| `list_tasks` | optional `status` | Tasks in this channel |
| `read_task` | `task_id`, optional `since_item_id` | The task, its recent session items and its review summary |
| `interrupt_task`, `retry_task` | `task_id` | |
| `routine_list`, `routine_create`, `routine_update`, `routine_delete` | Routine fields | |
| `suggest_memory` | `note`, `reason` | Channel coordinators only. Sends the note to the Everything coordinator as a global memory suggestion |

The Everything coordinator also has these tools:

| Tool | Arguments | Result |
|---|---|---|
| `list_channels` | None | Each channel's ID, name, purpose and counts of active tasks |
| `read_channel` | `channel_id`, optional `since` | The timeline, the tasks and the memory index of that channel |
| `message_channel` | `channel_id`, `body` | Posts in that channel as a message from Everything and wakes its coordinator |

Memory tools in the Everything coordinator accept any `scope` for reads. Writes go to `global` only.

### Tools for task sessions

| Tool | Arguments | Result |
|---|---|---|
| `post_update` | `body` | Posts in the task thread without ending the turn |
| `submit_for_review` | `summary` | The task goes to Ready for review when the turn ends |
| `publish_artifact` | `path` (in the workspace), `title` | The artifact's channel file path |
| `suggest_memory` | `note`, `reason` | Sends the note to the channel coordinator as input to its next turn. The coordinator decides whether to write it |

## Coordinator turn input

The hub wakes a coordinator when one of these events happens in its channel: a user message, a task status change to `review`, `needs_input` or `failed`, a decision answer, a routine run, a message from Everything, or a memory suggestion from a task. The hub runs one turn at a time for each channel. Events that arrive during a turn go into the next turn.

The hub writes the events into the turn text in this form:

```xml
<events>
  <user_message id="msg_..." thread="msg_..." at="2026-10-28T21:49:00Z">Explain what makes a channel special.</user_message>
  <task_update task="tsk_..." title="Go through open Sayso issues" status="review">PR #62 is up. Windows jobs still need a manual run.</task_update>
  <decision_answered id="dec_..." option="Not now"/>
  <routine id="rtn_..." name="Morning brief">Summarize my calendar and open tasks.</routine>
  <memory_suggestion from="tsk_..." reason="The user said this twice in the thread">Never push to main in sayso. Open a PR.</memory_suggestion>
</events>
```

A coordinator speaks only through `post_message`, `reply` and `no_reply_needed`. If a turn ends with assistant text and none of those three calls, the hub posts the last assistant message to the timeline.

## Client API

The client API uses the same frames, errors and authentication as the worker link.

### Sync

| Type | Body | Result |
|---|---|---|
| `sync.snapshot` | `scope`: `all` or a channel ID | The entities in that scope and the current feed `seq` |
| `sync.subscribe` | `after_seq`, optional `channel_ids` | `ok`. The hub then sends `feed` messages |
| `feed` (from the hub) | `seq`, `entity`, `entity_id`, `op` and `payload` | None |

### Requests

| Area | Types |
|---|---|
| Channels and groups | `channel.create`, `channel.update`, `channel.archive`, `channel.delete`, `channel.move`, `group.create`, `group.update`, `group.delete`, `group.reorder` |
| Messages | `message.send` (with `client_message_id`), `message.edit`, `message.delete`, `message.react`, `message.search`, `read.mark` |
| Tasks | `task.create`, `task.update`, `task.resolve`, `task.reopen`, `task.retry`, `task.interrupt`, `task.archive`, `task.delete`, `task.items`, `task.item_detail` |
| Waiting on the user | `approval.answer`, `decision.answer` |
| Memory | `memory.list`, `memory.read`, `memory.write`, `memory.delete`, `memory.history` |
| Channel files | `files.list`, `files.put` (after a blob upload), `files.move`, `files.delete` |
| Routines | `routine.create`, `routine.update`, `routine.delete`, `routine.run_now`, `routine.runs` |
| Workers | `worker.list`, `worker.pairing_code`, `worker.rename`, `worker.set_labels`, `worker.revoke`, `worker.refresh_inventory` |
| Clients | `client.pairing_code`, `client.list`, `client.revoke`, `client.push_register` |
| Auth | `auth.http_token` |

`message.send` with a `client_message_id` that the hub already stored returns the stored message. A client can retry a send without making a duplicate.

### Push notifications

The hub sends a push notification to each mobile client when:

- an approval or a decision opens,
- a task goes to `review` or `failed`,
- the assistant posts in a channel and no client has that channel open.

The payload holds a title, the channel ID and the entity ID. It holds no message text unless the user turns on message previews.
