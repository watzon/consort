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

A frame must be a JSON object. A missing `body` reads as `{}`. Readers ignore fields that they do not know, so a newer peer can add a field. `consort-proto` has the types for every frame on this page.

`v` is the frame format version. The protocol version is agreed in the `hello` exchange. A frame larger than 1 MiB is a protocol error. Large data goes through the blob endpoints.

### Size limits

The hub keeps every frame that it sends under 1 MiB. Limits count the bytes of a text as a JSON string: a quote or a backslash counts 2 bytes, and a control character counts 2 or 6 bytes.

| What | Limit | When it is larger |
|---|---|---|
| A message body | 256 KiB | The hub refuses it with `invalid` |
| A task brief, a routine prompt | 32 KiB | The hub refuses it with `invalid` |
| A channel purpose | 4 KiB | The hub refuses it with `invalid` |
| Channel instructions | 16 KiB | The hub refuses it with `invalid` |
| A notice or a task status reason with text from a worker | 4 KiB | The hub cuts it and says so at the end |
| A review summary (`submit_for_review`, or the last assistant message) | 16 KiB | The hub cuts it and says so at the end |
| A message from a worker, for example the fallback post or the card of a request | 256 KiB | The hub cuts it and says so at the end |
| A command to a worker | 1 MiB | The hub refuses to queue it, and the action fails with `invalid` |

A response that does not fit in one frame becomes an error response with the code `internal` and a message that names the size. A feed entry that does not fit becomes a `feed.skipped` event (see [Sync](#sync)).

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
| `sign_in_failed` | A provider's sign-in flow failed. `message` says why |
| `tool_install_failed` | The worker could not install or update a managed tool. `message` says why |
| `invalid_tag` | A tag is not 1 to 32 lowercase letters, digits or `-`. `message` names the tag |
| `unsupported` | The peer knows the request type but does not carry it out yet. `message` names the type, for example "Not built yet: account.create." |
| `connector_not_available` | The connection does not exist, or the session's channel cannot use it. `message` names the connectors that the channel can use |
| `composio_not_configured` | The hub has no Composio key. The user adds one in Settings > Connectors |
| `composio_error` | Composio refused a request or did not answer. `message` says why |
| `needs_user` | The action needs the user first, for example a connection that needs a new sign-in. `message` says what the user must do |
| `skill_blocked` | The skill is blocked, for example because its audit failed. `message` says why |
| `skill_invalid` | The skill does not follow the Agent Skills format or breaks a limit. `message` names the file or the limit |

A reader that gets a code it does not know treats it as `internal` and shows the message.

## Pairing and authentication

Workers and clients pair the same way, and both prove their identity with an Ed25519 key.

### Pair a worker

1. In a client, the user chooses **Add worker**. The hub makes a pairing code: 8 characters, single use, valid for 10 minutes. The characters come from the Crockford base32 alphabet (`0-9` and `A-Z` without `I`, `L`, `O` and `U`). The hub shows the code as `ABCD-EFGH`. It ignores case, hyphens and spaces in a typed code, and it reads `O` as `0` and `I` or `L` as `1`.
2. On the new machine, the user runs `consort worker install --hub https://hub.example.com --code ABCD-EFGH` (the Add worker dialog gives a command that does this). `consort worker pair <url> --code <code>` pairs without installing a service.
3. The worker makes an Ed25519 key pair. The private key stays in the OS keychain, or in a file with mode 0600 when no keychain is available.
4. The worker sends `POST /v1/pair/worker` with `code`, `public_key`, `hostname`, `os`, `arch` and an optional `name`. Public keys, signatures and nonces are lowercase hex on the wire.
5. The hub checks the code, stores the worker, and returns the `worker_id`. An HTTP error has the body `{"error": {"code", "message"}}`. The hub allows 10 pairing requests per minute from one IPv4 address or one IPv6 /64 network, and 60 per minute from all addresses together. After that, it answers HTTP 429 with the code `policy_denied`.
6. The worker writes `hub_url` and its ID to `worker.toml`.

### Pair a client

The first client uses the setup code that the hub prints on its first start. To pair a later client, a paired client shows a QR code that holds the hub URL and a new pairing code. The new client then follows the worker steps with `POST /v1/pair/client`, with `code`, `public_key`, `name` and `kind` (`desktop`, `ios` or `android`). The hub returns the `client_id`.

### Who made a code, and removing a client or a worker

The hub records who made each pairing code: a client, or the hub machine (the setup code, `consort hub setup-code` and the dev mode). It also records who made the code that each client and worker paired with, and who removed it. A client can remove another client with `client.revoke`. When a client is removed, the codes that it made and that are not used yet stop working.

A stolen client key could remove every other client. So the hub machine has its own commands, which work on the data folder while the hub runs:

- `consort hub client list` and `consort hub worker list` show each client or worker with its ID, who paired it, when it was last seen, and who removed it.
- `consort hub client revoke <id>` and `consort hub worker revoke <id>` remove one. A running hub reads the change within 2 seconds. It closes the connection of the client or the worker, ends the open sessions of a removed worker, and sends the entity in the feed again. A hub that is stopped does this when it starts.

To recover from a stolen client: remove that client on the hub machine, then make a new setup code with `consort hub setup-code`.

### Connect

1. The worker opens `wss://<hub>/v1/worker`. A client opens `wss://<hub>/v1/client`.
2. The hub sends the event `auth.challenge` with a 32-byte random `nonce`.
3. The peer sends the request `auth.response` with `peer_id` (its worker ID or client ID) and `signature`, an Ed25519 signature over the UTF-8 bytes `consort-auth-v1\n<origin>\n<nonce in hex>`.
4. The hub checks the signature against the stored public key. It answers `ok` with an empty body, or `unauthorized` and closes the connection.
5. The peer sends `hello`. The hub answers with the protocol version. If the worker or client was revoked after step 4, the hub answers `unauthorized` and closes the connection.

At most 64 connections can be between steps 1 and 5 at the same time, and at most 16 from one address (an IPv4 address, or an IPv6 /64 network). The hub closes a connection over a limit at once, with the WebSocket close code 1013. The peer tries again later. Behind a reverse proxy, every connection has the address of the proxy, so the limit of 16 then applies to all connections together.

Steps 2 to 5 must end in 15 seconds in total. Pings and pongs do not make this time longer. When the time runs out, the hub closes the connection with the close code 1008.

The origin is `scheme://host[:port]` of the hub URL, in lowercase, with `ws` read as `http` and `wss` as `https`, and without a default port, a path or a query. The peer takes it from the hub URL in its own configuration. The hub takes it from its configured public URL. So a peer cannot pass a challenge from one hub to another.

A client sends `hello` with `client_id`, `version` and `protocol_versions`. The hub answers with `protocol_version`, `hub_version` and the current feed `seq`.

A client that needs the blob endpoints sends `auth.http_token` and gets a bearer token that is valid for 10 minutes.

## Worker link

### Worker to hub

| Type | Body | Response |
|---|---|---|
| `hello` | `worker_id`, `version`, `protocol_versions`, `sessions` (each with `session_id`, `last_seq` and `state`) | `protocol_version`, `hub_version`, and `sessions` with the hub's stored `last_seq` for each open session that it knows. The hub sends `session.stop` for a session that it does not know, and for a session that has ended or is lost on the hub |
| `inventory` | See [Inventory](#inventory) | None |
| `session.events` | `session_id` and `events` (each with `seq`, `at` and `event`) | The ack: `session_id` and `last_seq`, the highest contiguous `seq` that the hub stored |
| `tool.list` | `session_id` | `tools`: the hub tools for that session's role, each with `name`, `description` and `input_schema`. A session that has ended or is lost gets `conflict` |
| `tool.call` | `session_id`, `call_id`, `name`, `arguments` | `content` (MCP text blocks, `{"type": "text", "text": ...}`), `is_error`, and optional `structured_content`. A session that has ended or is lost gets an error result with `conflict`. The hub runs at most 16 tool calls of one worker at the same time; more calls wait |
| `sign_in.update` | `sign_in_id`, `state` (see [Sign-in](#sign-in)) | None. The hub stores the new state and sends a feed upsert of the sign-in |
| `skill_bundle.get` | `content_hash` | `skill_id`, `name`, `content_hash` and `files` (each `path`, `sha256`, `size` and `executable`, sorted by path). The hub answers only for a hash that a `session.start` to this worker named, and `not_found` otherwise. The worker downloads each file with `GET /v1/blobs/{sha256}`, checks the content hash (see [data-model.md](data-model.md#skill)), and keeps the bundle in a cache by its hash |

### Hub to worker

Every hub command is a request. The envelope `id` of a hub command is its command ID. The worker stores the result of each command ID for 8 days: the 7 days that the hub keeps a queued command, and one more day. If the same command arrives again, the worker returns the stored result and does not run the command again. A command whose result is "None" in this table gets `ok` with an empty body.

| Type | Body | Result |
|---|---|---|
| `session.start` | `session_id`, `role`, `channel_id`, `task_id` (for a task session), `account_id`, `model`, `options`, `approval_mode`, `workspace` (see below), `system_prompt_append`, `allowed_local_mcp_servers`, `channel_files` (a list of `path` and `sha256`), optional `extensions` (see [Connectors and skills of a session](#connectors-and-skills-of-a-session)), optional `resume` with a `native_ref`, optional first `turn` | `workspace_path` and optional `native_ref` |
| `session.turn` | `session_id`, `turn` (`text` and `attachments`), optional `channel_files` (a list of `path` and `sha256`). The hub sends `channel_files` only when the list changed since the session got it last (see [Blobs](#blobs)) | `turn_id` |
| `session.steer` | `session_id`, `turn` | None. The worker sends `policy_denied` if the harness cannot steer. The worker refuses a steer when no turn runs, for example because the turn ended first. The hub does not lose the text of a refused steer (see [Delivery rules](#delivery-rules)) |
| `session.interrupt` | `session_id` | None |
| `session.respond` | `session_id`, `request_id`, `answer` (`option_id` and, for a question, `text`) | None. `not_found` means that the harness has closed the request already: the hub shows the approval or decision as `canceled`, posts a notice, and lets `turn.ended` or the session state decide the task status. Any other refusal fails a task that waits on the answer, with Retry |
| `session.stop` | `session_id`, `reason` | None |
| `workspace.release` | `task_id`, `delete` (bool) | None. The hub sends it with `delete` true when a task is deleted, when a resolved task is archived, and when its channel is deleted, to each worker that ran the task, after the `session.stop` of the task's session. It does not send it when a task is resolved, because Not done needs the workspace. After a release, the hub refuses Not done and offers Retry, which starts a new session in a new workspace. The hub refuses Retry while a release of the task still waits in a queue |
| `item.detail` | `session_id`, `item_id` | `sha256`, `size` and `mime` of a blob with the full command output or diff |
| `inventory.refresh` | Empty | None |
| `account.create` | `account_id` (the hub chooses it), `harness`, `label`, optional `acp`, `env` | None. The worker makes the profile folder, writes `env` to `env.json` in it, adds the account to `worker.toml`, and sends a new inventory |
| `account.update` | `account_id`, optional `label`, `env_set`, `env_remove`, `extras_enabled` | None. `extras_enabled` turns the provider extras of the account on or off from the next session or resume. The worker keeps the setting for the account. A running session keeps what it started with |
| `account.remove` | `account_id` | None. The worker signs out where it can and deletes the profile folder |
| `account.sign_out` | `account_id` | None |
| `sign_in.start` | `sign_in_id`, `account_id`, `method` | None. The worker sends each state as `sign_in.update` |
| `sign_in.code` | `sign_in_id`, `code` | None. The worker writes the code to the harness and does not store it |
| `sign_in.cancel` | `sign_in_id` | None |
| `tools.check` | Empty | None. The result comes in the next inventory |
| `tools.install` | `tool` | None. The progress comes in the inventory |

The hub never queues the account, sign-in and tool commands for an offline worker. The client request fails with `worker_offline` instead.

`env` and `env_set` are JSON objects of variable names to values, such as `{"ANTHROPIC_API_KEY": "sk-..."}`. The hub passes them through once and never stores or logs them. The `Debug` output of every type that holds them shows only the names. The same is true for the `code` of `sign_in.code`.

The `workspace` object of `session.start` has these fields:

| Field | Notes |
|---|---|
| `repos` | A list. Each repo has `repo_id`, `url`, optional `branch` and optional `checkout_path` |
| `isolation` | `in_place` runs in the checkout. `worktree` makes a git worktree from the checkout's `HEAD`. `clone` makes a fresh clone in `workspaces/` |
| `carry_uncommitted` | Copy uncommitted changes from the checkout into the worktree. Allowed only with `worktree` |

A workspace with no repos is an empty folder in `workspaces/`. Coordinators usually run in one.

#### Connectors and skills of a session

The `extensions` object of `session.start` holds the session's effective set when the session starts (see [data-model.md](data-model.md#effective-set)). A change reaches the next session. The hub ends an idle coordinator session when the effective set of its channel changes, as it does for a new model.

| Field | Notes |
|---|---|
| `connectors` | One entry per connection: `handle`, `service_name`, optional `identity` and `needs_sign_in` |
| `skills` | One entry per skill: `skill_id`, `name` and `content_hash`. The worker gets a bundle that it does not have with `skill_bundle.get`, and writes each skill into a folder per session, where its harness reads skills |
| `skill_context` | The `name` and `description` of each skill, in the order of `skills` |

The worker appends the context blocks to the system prompt with `SessionExtensions::prompt_blocks` in `consort-proto`, so every adapter shows the same text:

```text
Connectors in this channel:
- gmail_work: Gmail, chris@work.example
- gmail_personal: Gmail, chris@example.com
To use one, call list_connector_tools with its handle, then run_connector_tool.

Skills in this channel:
- pdf-forms: Fill and read PDF forms.
They are installed as skills. Use one when the task fits it.
```

A harness that does not load skills itself gets the last line "Before you use one, call read_skill with its name and follow its SKILL.md.", and the hub lists `list_skills` and `read_skill` for its session. A hub that sends no `extensions` leaves the field out, and the session has no connector and no skill.

The hub waits 60 minutes for the answer to `session.start`, because the worker prepares the workspace first and a clone can take 15 minutes. It waits 10 minutes for any other queued command. When the time runs out, the hub keeps the link open (the read loop closes a link only when the worker sends nothing, not even a ping, for 60 seconds) and sends the same command again. The worker runs a command ID once and answers when the first run ends.

### Session events

The `event` field of a session event is one of these objects:

| `type` | Fields | Notes |
|---|---|---|
| `state` | `state` | `starting`, `active`, `idle`, `ended` or `lost` |
| `native_ref` | `ref` | The harness's own session ID |
| `turn.started` | `turn_id` | |
| `item` | A full session item | Replaces any earlier item with the same `id` |
| `output.delta` | `item_id`, `text` | Command output. The worker merges deltas into at most one event every 250 ms |
| `request.opened` | `request_id`, `kind` (`approval` or `question`), `summary`, `details`, `options` | The harness waits for `session.respond`. See below |
| `request.closed` | `request_id`, `outcome` | `allowed`, `denied`, `answered`, `canceled` or `expired` |
| `usage` | `input_tokens`, `output_tokens`, `cached_tokens`, optional `cost_usd` | Totals for the session |
| `turn.ended` | `turn_id`, `reason` (`completed`, `interrupted` or `failed`), optional `error` | |
| `reopened` | | The worker opened the session again after the worker restarted. The hub cancels the session's pending approvals and the decisions that came from a harness question. Decisions from `ask_decision` stay. The hub ends an open turn as failed, but not a turn that the worker never got: a turn whose `session.turn` has not gone out yet, or one that the hub sent on the connection that carries this `reopened` event (after a reconnect the hub sends its queue at once, so the turn can arrive first). The hub keeps such a turn open. Only a turn that went out on an earlier connection was in flight when the worker restarted |
| `error` | `code`, `message` | |

The worker answers an approval request for a tool of the `hub` MCP server itself, with the allow-once option, and does not send it, but only when the tool is in the session's last `tool.list` result. Any other approval, also one that claims the server `hub`, goes to the hub like any other approval. The hub checks its own tools. The name `hub` is kept for this server: the worker refuses a local MCP server whose name is `hub` in any case or contains `__`.

The worker cuts a session event to at most 192 KiB before it spools the event, and marks the cut. A large tool argument becomes `{consort_truncated, original_bytes, preview}`. A spool row that the worker cannot read goes out as an `error` event with the code `event_unreadable` and the same sequence number.

The worker runs a command ID once. When the hub sends a command again while the first run still goes on, the worker answers once, when the first run ends.

The `code` of an `error` event is one of these:

| Code | From | Meaning |
|---|---|---|
| `event_unreadable` | Worker | The worker could not read a spool row. The event has the sequence number of that row |
| `event_too_large` | Worker | An event was still larger than 192 KiB after the cut |
| `start_failed` | Worker | The session could not start: the harness did not open, or the first turn failed. The state `lost` follows, and `session.start` gets an error |
| `resume_failed` | Worker | After a worker restart, the worker could not open the session again. The state `lost` follows |
| `harness_unresponsive` | Adapter | A turn did not end 30 s after an interrupt, or a request did not get an answer in time. The worker stopped the harness and reports the session lost |
| `signed_out` | Adapter | A turn failed, or the session could not open, because the harness login is gone. The message says how to sign in again. When the session could not open, the worker also puts the sign-in step in the `session.start` error, and the event can reach the hub after that error. The hub marks the account signed out in both cases |
| `option_failed` | ACP adapter | The harness refused a model option, for example the model or the effort. The session goes on with the harness default |
| `elicitation_declined` | Codex adapter | An MCP server asked for input. The worker declined it, because Consort does not support MCP elicitation yet |
| `codex_error` | Codex adapter | Codex reported an error that it will not retry |
| `attachment_failed` | Worker | The worker could not get an attachment or a channel file for the session: the download failed, the hash did not match, the file is larger than 256 MiB, or the path is not safe. The message names the file. The session goes on without it. The hub also posts the message as a notice, in the task thread or in the channel timeline |
| `mode_reset` | ACP adapter | The agent raised its permission mode above the worker's limit, and the adapter set it back |
| `mode_reset_failed` | ACP adapter | The adapter could not set a raised permission mode back, so it stopped the agent. The session is lost |
| `approval_too_long` | Worker | An approval was too long to show in full, so the worker denied it. The model gets the same reason as a steer when the harness can steer |
| `approval_mode_lowered` | Worker | The session asked for Full access, but the agent offers no bypass mode, so the session runs in Auto |

The hub stores each `error` event as a failed item of the session. After `signed_out`, the hub also marks the account `signed_out` in the stored inventory of the worker, so placement does not choose it until the next inventory from the worker. A coordinator session on that account ends as `lost`: its turn's events go back to the queue, and placement starts a new session on another signed-in account. When no account fits, the channel gets a notice that names the account and the worker to sign in on, and the events wait for the next inventory. A failed coordinator turn on an account that the inventory shows as signed out has the same result.

The `details` of an approval request has a `kind` and the fields for that kind: `command` (`command`, `cwd`), `file_change` (`paths`), `tool` (`server`, `tool`, `arguments`), `network` (`target`) or `other` (`text`). The hub card kinds (`connector`, `management`, `connect_service`, `sign_in` and `skill_install`) are for approvals that the hub makes itself. The hub reads one of them from a worker as `other`, with the summary as the text. Every `command` approval has its `cwd`. A Codex network approval comes as `command` details, with the command and its folder, and its summary names the host, for example "Run `git fetch` with network access to github.com". The `details` of a question has `question`, `context` and `allow_free_text`. Each option has `id`, `label`, `kind` (`allow_once`, `allow_always`, `deny_once`, `deny_always` or `choice`) and an optional `description`.

The hub makes an approval for an approval request. It makes a decision with a `request_id` for a question, and it sends the answer back with `session.respond`, not as a new turn.

### Delivery rules

- The worker numbers each session's events from 1, with no gaps.
- The worker writes every event to its spool before it sends the event. The worker deletes an event from the spool after the hub acknowledges it.
- The worker sends batches of at most 256 events or 256 KiB.
- On reconnect, `hello` lists the open sessions. The hub answers with its stored `last_seq` for each session, and the worker sends every event after that number.
- The hub keeps commands for an offline worker in a per-session queue and sends them in order on reconnect. A queued command expires after 7 days. Then the hub ends its session as `lost`: a task fails with Retry, and the channel or the task thread gets a notice. An answer whose `session.respond` expired shows as `expired`.
- When the hub ends a session that the worker can still run, it sends `session.stop`. For example: the hub marks a session `lost`, or the user retries a failed task. The hub deletes the other queued commands of that session first. Only a command that is in flight stays. When the session's `session.start` never went out, the hub deletes it and sends no `session.stop`, because the worker does not know the session.
- Events of a session that has ended on the hub, or that is not the current session of its task (for example the old session of a retried task), are stored, but they do not change the task: a `turn.ended` does not move it and does not take its inbox, and a `request.opened` makes no approval or decision. A coordinator session that is not the current one of its channel opens no request either.
- When the worker refuses a `session.steer`, the hub gives the text to the task in another way: while the hub sees the turn as open, the text waits in the task inbox; when the turn is closed, the text starts a new turn and a task in Ready for review goes back to Working. When the task cannot take it (it failed, or its session ended), the task thread gets a `message_not_sent` notice.
- A task turn that sends no event for 2 hours while its worker is online fails the task with Retry, and the hub marks the session `lost`. The time that a task waits in Needs input does not count: an answer starts the 2 hours again. A coordinator turn that runs for more than 2 hours while its worker is online makes the hub replace the session. The time that the worker is offline does not count, because its events wait in the spool.
- When a coordinator session ends in the middle of a turn (it is lost, the harness does not answer, or the turn expired), the events of the turn go back to the queue only when the turn called no hub tool. A turn that called one may have acted already, for example started a task, so a new session must not get the same events. The channel then gets a `turn_failed` notice that asks the user to check what the assistant did and to send the message again if something is missing.
- When the worker refuses a `session.turn` of a task, no turn runs. A task that the message moved from Ready for review goes back to Ready for review, with a reason that asks the user to send the message again. Any other task in Working fails with Retry. When the worker refuses a coordinator turn, its events go back to the queue. The hub tries again after the start backoff below, or at once when the session sends a `turn.ended`, because the session is free then.
- When the worker answers `session.start` with `conflict`, the worker has a session with that ID but no stored result. The hub marks the session `lost` and sends `session.stop`, so the harness does not keep running.
- A task in Ready for review whose session ends stays in Ready for review, with a reason that says that it cannot take a message. Retry starts a new session, from Ready for review or Resolved, when the session of the task has ended. The open decisions of the old session are canceled then.
- After a coordinator session fails to start, the channel waits before it starts a new one: 10 s after the first failure, then twice as long after each new failure, at most 10 minutes. A session that never became active does not start a new one by itself; the next wake event or a worker that comes online does.
- While a task session has an open turn, a new message for the task steers the turn (when the worker is online and the harness can steer) or waits for the turn to end. The hub never sends a second `session.turn` while a turn is open, also not to an offline worker.
- A coordinator turn holds as many queued events as fit in 512 KiB. The other events wait for the next turn. An event that alone does not fit is cut, with a note that names `fetch_thread` and the message ID. Task inbox messages split over turns in the same way.
- The worker sends a WebSocket ping every 20 seconds. The hub marks the worker offline after 60 seconds with no frames.

### Inventory

The worker sends a full inventory after `hello` and again when any part changes.

| Field | Notes |
|---|---|
| `hash` | Hash of the rest of the inventory: `fnv1a64:` and 16 hex characters over the JSON of the inventory with an empty `hash`. It detects changes. It is not a security hash |
| `hostname`, `os`, `arch` | |
| `tags` | The detected tags (`macos`, `windows`, `linux`, `arm64`, `x64`, `ram-<n>gb`, `cpu-<n>`, `gpu-nvidia`, `gpu-amd`, `gpu-apple`) and the `tags` in `worker.toml`. An older worker sends `labels`, which reads as `tags` |
| `accounts` | Each harness account: `account_id`, `harness`, `label`, `harness_version`, `auth_state`, `auth_detail`, `models`, `capabilities`, `enabled`, `profile` (true for an account that Consort made, with a profile folder), `email`, `plan`, `secret_names` (the names in `env.json`, never the values), `sign_in_methods`, `extras_enabled` (true when the field is missing) and optional `extras` (what the account syncs from the provider, see [data-model.md](data-model.md#harness-account)). Never a credential |
| `tools` | Each managed tool: `tool`, `version`, `latest`, `state` (`missing`, `installing`, `ready`, `update_available` or `failed`) and `detail`. `tool` is `node`, `claude-code`, `claude-agent-acp`, `codex`, `opencode`, `gemini-cli` or `acp:<registry id>` |
| `local_mcp_servers` | Each server's `name`, `description` and tool names. Never a URL with a secret, and never a credential |
| `checkouts` | Each checkout's `repo_url`, `path` and `managed` flag |
| `policy` | `max_approval_mode`, `allowed_roots` and `allow_shell` |
| `load` | `active_sessions` |

### Sign-in

A sign-in is one run of a provider's sign-in flow for one account. The user starts it from a client with `sign_in.start`. The hub makes a `SignIn` record (ID prefix `sgn_`) in the `starting` state, returns it, and sends `sign_in.start` to the worker. Each `sign_in.update` from the worker becomes a feed upsert of the record.

`method` has a `kind` and the fields for that kind:

| Kind | Fields | Flow |
|---|---|---|
| `claude_plan` | | `claude auth login --claudeai`. The user opens a URL and pastes a code |
| `claude_console` | | `claude auth login --console`. The same flow |
| `chatgpt_device` | | Codex device code. The user enters a code on a page |
| `chatgpt_browser` | | Codex browser sign-in on the worker machine |
| `api_key` | `variable` | No flow. The key arrives in `env` of `account.create` or `env_set` of `account.update` |
| `acp_agent` | `method_id`, `name` | ACP `authenticate` with that method |
| `terminal` | `command` | The app cannot run it. The app shows the command, and the user runs it on the worker |

`state` has a `kind` and the fields for that kind:

| Kind | Fields | What the client shows |
|---|---|---|
| `starting` | | A spinner |
| `open_url` | `url` | Open sign-in page |
| `device_code` | `url`, `user_code` | The code in large type, Open sign-in page and Copy code |
| `needs_code` | `url` | Open sign-in page, Copy link, a code field and Send code. The client sends the code with `sign_in.submit_code` |
| `verifying` | | A spinner |
| `succeeded` | optional `detail` | "Signed in as" the detail, and Done |
| `failed` | `message` | The message and Try again |
| `cancelled` | | |
| `expired` | | No update came for 15 minutes |

`succeeded`, `failed`, `cancelled` and `expired` are final. The hub deletes a sign-in one day after its last update.

### Blobs

`PUT /v1/blobs/{sha256}` uploads a blob. `GET /v1/blobs/{sha256}` downloads it. `HEAD /v1/blobs/{sha256}` says whether the hub has it (`200` with `Content-Length`, or `404`) without reading it, so a client can skip an upload. Clients authenticate with `Authorization: Bearer <token>`, with the token from `auth.http_token`. The hub checks the headers before it reads the body, so a request without valid credentials gets `401` at once. It rejects an upload whose content does not match the hash, and an upload larger than 256 MiB.

Workers authenticate with three headers:

| Header | Value |
|---|---|
| `X-Consort-Worker` | The worker ID |
| `X-Consort-Timestamp` | The request time in Unix milliseconds. The hub refuses a time more than 5 minutes from its own clock |
| `X-Consort-Signature` | The hex Ed25519 signature over the UTF-8 bytes `consort-http-v1\n<METHOD>\n<origin>\n<path>\n<timestamp>\n<worker id>` |

#### Attachments and channel files on the worker

The worker gets every file of a session with the signed `GET /v1/blobs/{sha256}`. It checks the SHA-256 and keeps the file in a cache by hash, `<consort home>/cache/blobs/<sha256>`. Each use checks the hash again. A file larger than 256 MiB, a download that fails and a hash that does not match fail only that file, with an `attachment_failed` error event.

- **Attachments.** Before a turn or a steer reaches the harness, the worker saves each attachment at `<workspace>/.consort/attachments/<first 12 hex characters of the hash>-<name>`. The name loses path separators, control characters and the characters that Windows refuses, loses dots and spaces at its ends, and has at most 100 bytes. The adapter tells the model the name, the type, the size and the path of each attachment, and sends images as image input where the harness takes them (see [harness-adapters.md](harness-adapters.md#attachments-and-images)).
- **Channel files.** At `session.start`, the worker makes a read-only copy of `channel_files` in `<workspace>/.channel/files/`. A task session's list has no files under `artifacts/<its own task id>/`. When a `session.turn` has `channel_files`, the worker makes the copy match the new list before the turn starts: it writes new and changed files, and removes files that are gone and files that the session added or changed. The worker never sends the copy back to the hub. A session publishes a file with `publish_artifact`, and a coordinator writes one with `files_write`.
- In a workspace that is a git checkout, the worker adds `/.channel/` and `/.consort/` to the repo's `info/exclude`, so git does not see them.

## Hub tools

The worker runs one MCP endpoint per session at `http://127.0.0.1:<port>/mcp/<session_id>`, with streamable HTTP transport. The worker makes a random 32-byte bearer token for each session and gives it only to that session's harness.

The worker is the MCP server. It gets the tool list from the hub with `tool.list` when the session starts, and it sends each tool call to the hub as `tool.call`. The hub runs the tool logic but does not speak MCP. The hub knows the session from the authenticated worker link, so the token never leaves the worker.

The worker answers part of `publish_artifact` itself: it uploads the file from the workspace, then sends the call to the hub with the blob hash added.

The hub gives each session the tools for its role. Each tool's `input_schema` is an inline JSON Schema object with no `$ref`. Times in tool arguments are RFC 3339 strings. Results are JSON objects. A tool that only confirms returns `{"ok": true}`, and a failed call returns `is_error` with `{"error": {"code", "message"}}`.

### Tools for every session

| Tool | Arguments | Result |
|---|---|---|
| `memory_list` | optional `scope` | Paths, hashes and update times |
| `memory_read` | optional `scope`, `path` | `content` and `sha256` |
| `files_list` | optional `prefix` | Channel file paths, sizes and hashes |
| `list_workers` | None | Each worker's ID, name, OS, state, tags, account labels with their models, local MCP server names and repo checkouts |
| `ask_decision` | `question`, `options` (2 to 4, each with `label` and `consequence`), optional `recommended_index`, `context` and `allow_free_text` | `decision_id`. The answer arrives as input to a later turn |
| `search` | `query`, optional `channel_id` | Matching messages, memory notes and tasks |

### Tools for coordinators

| Tool | Arguments | Result |
|---|---|---|
| `post_message` | `body`, optional `attachments` (channel file paths) | `message_id` |
| `reply` | `message_id`, `body`, optional `in_timeline` and `attachments` (channel file paths) | `message_id` |
| `update_message` | `message_id`, `body` | Only for the session's own messages |
| `react` | `message_id`, `emoji` | |
| `no_reply_needed` | None | Ends the turn with no visible reply |
| `fetch_timeline` | optional `before`, `limit` | Messages, newest first, and `has_more`. The page stops early when its messages would take more than 320 KiB |
| `fetch_thread` | `message_id` (the root or any reply), optional `before` and `limit` | `root`, a page of `replies` (oldest first), `has_more`, and `message`: the reply that `message_id` names, when it is not in `replies`. All of it takes at most 320 KiB. The named reply is always whole. A long root is cut when the call names a reply, and the newest reply is cut when it alone does not fit. A cut message ends with a note that names its ID. With `before`, the page has the replies older than that reply, and no `message`. The default `limit` is 50, and the limit is 200 |
| `memory_write` | `path`, `content`, `expected_sha256` or `"not_exists"` | New `sha256`, or `stale_write` |
| `memory_delete` | `path`, `expected_sha256` | |
| `files_write` | `path`, `content` | Text files only |
| `start_task` | `title`, `brief`, optional `model_selection`, `worker_id`, `require`, `repos`, `isolation`, `approval_mode`, `start_at` (RFC 3339) and `attachments` (channel file paths, which go with the first turn of each session of the task) | `task_id`, `status`, optional `status_reason`, and `notes` when the hub lowered the request (see below) |
| `message_task` | `task_id`, `body`, optional `attachments` (channel file paths) | Starts a turn, or steers the running turn when the harness can steer. `delivery` is `turn`, `steer` or `queued` |
| `list_tasks` | optional `status` | Tasks in this channel. A brief or review summary longer than 1 KiB is cut to a preview |
| `read_task` | `task_id`, optional `since_item_id`, `before_item_id` and `limit` | The full task, a page of its session items (oldest first) and `has_more`. The task and the items take at most 320 KiB. Without `since_item_id`, the page has the newest items, or the newest ones older than `before_item_id`, and `has_more` says that older items exist. With `since_item_id`, the page has the oldest items after it, and `has_more` says that newer items exist. The default `limit` is 50, and the limit is 200 |
| `interrupt_task`, `retry_task` | `task_id` | |
| `routine_list`, `routine_create`, `routine_update`, `routine_delete` | Routine fields | `routine_create` and `routine_update` give `routine`, and `notes` when the hub lowered the placement of a `start_task` action |
| `suggest_memory` | `note`, `reason` | Channel coordinators only. Sends the note to the Tutti coordinator as a global memory suggestion |

Text in a web page, a file or a message can steer a coordinator. So a task that a coordinator starts, with `start_task` or with a `start_task` routine, gets at most what its channel allows:

- The approval mode is at most the channel's `default_approval_mode`.
- The isolation is `in_place` only when the channel's `default_isolation` is `in_place`. Otherwise the task uses the channel's default isolation.
- `require.local_mcp_servers` keeps only the servers in the channel's `allowed_local_mcp_servers`. The default list is empty.

The hub does not refuse the task. It runs with less, the tool result lists each change in `notes`, and the task thread gets a `task_limited` notice, so the user sees it too. A task that the user starts from a client has no such limits.

The wire value of the main channel is `tutti` (`"kind": "tutti"`). Old data and older peers say `everything` (also `everything_coordinator` and `everything_message`). The hub and the clients still read those values. See [ADR 0012](adr/0012-the-main-channel-is-called-tutti.md).

The Tutti coordinator also has these tools:

| Tool | Arguments | Result |
|---|---|---|
| `list_channels` | None | Each channel's ID, name, purpose and counts of active tasks |
| `read_channel` | `channel_id`, optional `since` | The timeline, the tasks and the memory index of that channel |
| `message_channel` | `channel_id`, `body` | Posts in that channel as a message from Tutti and wakes its coordinator |

Memory tools in the Tutti coordinator accept any `scope` for reads. Writes go to `global` only.

The Tutti coordinator also has the management tools in [Management tools](#management-tools).

### Tools for task sessions

| Tool | Arguments | Result |
|---|---|---|
| `post_update` | `body` | Posts in the task thread without ending the turn |
| `submit_for_review` | `summary` | The task goes to Ready for review when the turn ends |
| `publish_artifact` | `path` (in the workspace), `title` | The artifact's channel file path |
| `suggest_memory` | `note`, `reason` | Sends the note to the channel coordinator as input to its next turn. The coordinator decides whether to write it |

### Connector and skill tools

Every session can have these tools. The hub lists the connector tools only for a session whose channel can use at least one connection, and the skill tools only for a harness that does not load skills itself. A connector is named by its handle, as the "Connectors in this channel" block shows it.

| Tool | Arguments | Result |
|---|---|---|
| `list_connector_tools` | `connector`, optional `search` | `connector`, `service_name`, `tools` (each `slug`, `description`, `kind` and `approval`, at most 200) and `has_more` |
| `get_connector_tool` | `connector`, `tool` | `connector`, the tool fields, and `input_schema` |
| `run_connector_tool` | `connector`, `tool`, `arguments` (an object) | `status`: `ran` with `call_id`, `successful`, `data`, optional `error` and `truncated`; `waiting_for_user` with `approval_id` and `message`; or `denied` with `call_id` and `message` |
| `list_skills` | None | `skills`, each `name` and `description` |
| `read_skill` | `skill` (the name), optional `path` (default `SKILL.md`) | `skill`, `path`, `content` (at most 512 KiB), `truncated`, and `files` (the other paths) for `SKILL.md` |

`run_connector_tool` works like this:

1. The hub finds the connection by its handle or ID in the session's effective set. A connection outside the set fails with `connector_not_available`, also when it exists. A connection that needs a new sign-in fails with `needs_user`, and the message asks the assistant to tell the user to reconnect it in Settings > Connectors. No key fails with `composio_not_configured`.
2. The tool kind picks the rule. "Ask every time" posts a connector approval card. "Ask once per task" posts one for the first call of that kind in the task (or in the coordinator session), and runs later calls of that kind without asking.
3. The hub waits up to 5 minutes for the answer. Allow runs the call. Don't returns `denied`. When the time runs out, the call returns `waiting_for_user`. When the user allows the card later, the call runs then, and the result arrives as a `card_answered` event (a coordinator) or a message to the task.
4. The hub runs the tool with the connection's Composio connected account, writes a connector call record (see [data-model.md](data-model.md#connector-call)), and returns the result. A `data` larger than 256 KiB is cut, with `truncated` set.

### Management tools

The hub enforces the scope and the risk rule. A change runs at once and posts a short activity line when it adds something new and narrow, or makes access smaller or approvals stricter. It waits for the user on a management approval card when it deletes or archives, widens access (more channels or workers, or every channel), makes approvals looser, turns on a skill with scripts, changes worker or channel tags, or turns provider extras on. A sign-in or a key always goes through a card that the user completes in the app or the browser. The assistant never handles a secret.

Every tool that changes something returns `status` (`applied`, `waiting_for_user` or `refused`), `summary`, and the IDs that apply: `approval_id` (the card, for `waiting_for_user`), `channel_id`, `connection_id`, `skill_id` or `account_id`. The answer to a card arrives as a `card_answered` event. A connection is named by its handle or its ID, and a skill by its name or its ID.

For the coordinators of every channel:

| Tool | Arguments | Notes |
|---|---|---|
| `list_connections` | None | `connections`. A channel's coordinator sees the ones that its channel can use, and `off_in_this_channel` |
| `list_installed_skills` | None | `skills`, with the same rule, and `off_in_this_channel` |
| `connect_service` | `service`, `name`, optional `use_in`, `approvals` | Posts a connect card. In a channel, the default `use_in` is the channels with this channel's tags, or this channel alone when it has no tags. In Tutti, every channel |
| `add_skill` | `source` (a git URL, `owner/repo` or a `search_skills` entry ID), optional `path`, `use_in` | Posts an install card with the license, the scripts and the audits. Same defaults for `use_in` |

For the Tutti coordinator only:

| Tool | Arguments | Notes |
|---|---|---|
| `create_channel` | `name`, `purpose`, optional `group` (a group name, made when missing), `tags`, `model`, `task_model` | Runs at once |
| `update_channel` | `channel`, optional `name`, `purpose`, `instructions`, `group`, `tags`, `model`, `task_model`, `repos` (git remote URLs) | A change to `tags` asks first |
| `archive_channel` | `channel` | Asks first |
| `set_default_model` | `model` (a model selection) | Tutti's selection |
| `set_worker_tags` | `worker`, `tags` | Asks first |
| `install_tool` | `worker`, `tool` | As `worker.tool_install` |
| `add_account` | `worker`, `provider`, `name` | Makes the account, then posts a sign-in card |
| `start_sign_in` | `worker`, `account`, optional `method` (a method kind) | Posts a sign-in card |
| `set_account_extras` | `worker`, `account`, `enabled` | Turning them on asks first |
| `search_connector_catalog` | `search`, optional `category` | `services` |
| `update_connection` | `connection`, optional `name`, `use_in`, `approvals` | Follows the risk rule |
| `disconnect` | `connection` | Asks first |
| `search_skills` | `search`, optional `source` | `entries` |
| `update_skill` | `skill`, optional `enabled`, `use_in`, `accept_new_version` | Follows the risk rule |
| `remove_skill` | `skill` | Asks first |

For a channel's coordinator, only for its own channel:

| Tool | Arguments | Notes |
|---|---|---|
| `update_this_channel` | optional `purpose`, `instructions`, `model`, `task_model`, `repos` | Runs at once |
| `set_channel_tags` | `tags` | Asks first |
| `set_channel_connector` | `connection`, `enabled` | Only for a connection whose `use_in` gives it to the channel. Otherwise `refused` |
| `set_channel_skill` | `skill`, `enabled` | The same rule |
| `request_access` | `connection` or `skill` | Posts a management approval card that asks the user to widen the item's `use_in` to this channel |

Task sessions have no management tools.

## Coordinator turn input

The hub wakes a coordinator when one of these events happens in its channel: a user message, a task status change to `review`, `needs_input` or `failed`, a decision answer, a routine run, a message from Tutti, a memory suggestion, or the answer to a hub card that the coordinator posted. A memory suggestion comes from a task to its channel coordinator, or from a channel coordinator to the Tutti coordinator. The hub runs one turn at a time for each channel. Events that arrive during a turn go into the next turn.

The hub writes the events into the turn text in this form:

```xml
<events>
  <user_message id="msg_..." thread="msg_..." at="2026-10-28T21:49:00Z">Explain what makes a channel special.</user_message>
  <task_update task="tsk_..." title="Go through open Sayso issues" status="review">PR #62 is up. Windows jobs still need a manual run.</task_update>
  <decision_answered id="dec_..." option="Not now"/>
  <routine id="rtn_..." name="Morning brief">Summarize my calendar and open tasks.</routine>
  <memory_suggestion from="tsk_..." reason="The user said this twice in the thread">Never push to main in sayso. Open a PR.</memory_suggestion>
  <tutti_message id="msg_..." at="2026-10-28T21:49:00Z">Check the release notes before you ship.</tutti_message>
  <card_answered id="apr_..." kind="management" outcome="allowed">GitHub Work is now available in 4 more channels.</card_answered>
</events>
```

A suggestion from a channel coordinator has `from="chn_..."`. A decision answer with free text puts the text inside the element. The hub escapes `&`, `<` and `>` in text, and also `"`, `'`, tabs and line breaks in attribute values, so user text cannot close an element or add an event.

A coordinator speaks only through `post_message`, `reply` and `no_reply_needed`. If a turn ends with assistant text and none of those three calls, the hub posts the last assistant message to the timeline.

## Client API

The client API uses the same frames, errors and authentication as the worker link.

### Sync

| Type | Body | Result |
|---|---|---|
| `sync.snapshot` | `scope`: `all` or a channel ID | The entities in that scope and the current feed `seq`. For `all`: every group, channel, repo, worker and read marker, the tasks that are not archived, the pending approvals and decisions, every routine, and `sign_ins`: the sign-ins that have not ended (an ended sign-in is left out, and the field is left out when it is empty). A client that loads during a sign-in shows it from there and then follows its feed entries. It also has `preferences`, the user's preferences (see [data-model.md](data-model.md#preferences)), and `connections` and `skills`, every connection and installed skill (each left out when empty). A channel snapshot leaves `preferences` out. For a channel: the channel, its newest 50 timeline messages (oldest first), its tasks that are not archived, its pending approvals and decisions, its routines and its channel files. Large messages can make the page shorter than 50. `messages_has_more` is true when older timeline messages exist. See [Tasks in a snapshot](#tasks-in-a-snapshot) |
| `sync.subscribe` | `after_seq`, optional `channel_ids` | `ok`. The hub then sends `feed` messages |
| `feed` (from the hub) | `seq`, `at`, `channel_id`, `entity`, `entity_id`, `op` and `payload` | None. `payload` is `null` for `delete` |
| `feed.skipped` (from the hub) | `seq`, `channel_id`, `entity` and `entity_id` | None. The hub could not send the entry `seq`, because it does not fit in one frame. The feed goes on after it. Get the entity again, for example with `message.list` or a new snapshot. A client that does not know this event ignores it |

When a channel is deleted, the feed has a `delete` for each entity of the channel that a client can hold (messages, read markers, tasks, session items, approvals, decisions, routines, channel files and memory notes), and then one for the channel.

#### Tasks in a snapshot

A snapshot cuts the `brief` and the `review_summary` of a task to a preview of 1 KiB when they are longer, and puts the task ID in `tasks_cut`. Every task that is not resolved is in the snapshot. Resolved tasks follow, newest first, while the snapshot fits in one frame. When some resolved tasks do not fit, `tasks_has_more` is true. The two fields are left out when they are empty or false.

A client does this:

1. For a task in `tasks_cut`, mark its `brief` and `review_summary` as previews. Show the preview in lists and on task cards.
2. Before you show the full brief or review summary of a task with previews, for example in the task thread header or the review bar, send `task.get`. Its result has the full task. A feed entry for a task also has the full task, so it ends the preview state.
3. When `tasks_has_more` is true, a task card in the timeline can name a task that is not in the snapshot. Send `task.get` for it.

A client that does not know these fields shows the preview, which ends with a note that says that the hub cut it.

### Requests

| Area | Types |
|---|---|
| Channels and groups | `channel.create`, `channel.update`, `channel.archive` (with `archived`, default true), `channel.delete`, `channel.move`, `group.create`, `group.update`, `group.delete`, `group.reorder`, `repo.create` |
| Messages | `message.send` (with `client_message_id`), `message.list` (a page of a timeline or a thread, newest first; the page stops early when its messages would take more than 768 KiB, and `has_more` is then true), `message.edit`, `message.delete`, `message.react`, `message.search`, `read.mark` |
| Tasks | `task.create`, `task.update`, `task.get` (with `task_id`: the full task), `task.resolve`, `task.reopen`, `task.retry`, `task.interrupt`, `task.archive` (with `archived`, default true; archiving a resolved task stops its session and deletes its workspace), `task.delete`, `task.items`, `task.item_detail` (with `session_id` and `item_id`) |
| Waiting on the user | `approval.answer`, `decision.answer` |
| Memory | `memory.list`, `memory.read`, `memory.write`, `memory.delete`, `memory.history` |
| Channel files | `files.list`, `files.put` (after a blob upload), `files.move`, `files.delete` |
| Routines | `routine.create`, `routine.update`, `routine.delete`, `routine.run_now`, `routine.runs` |
| Workers | `worker.list`, `worker.pairing_code`, `worker.rename`, `worker.set_tags` (with `worker_id` and `tags`), `worker.set_paused`, `worker.revoke`, `worker.refresh_inventory`, `worker.tools_check` (with `worker_id`), `worker.tool_install` (with `worker_id` and `tool`) |
| Accounts | `account.create` (with `worker_id`, `harness`, `label`, optional `acp`, `env`; the result has `account_id`), `account.update` (with `worker_id`, `account_id`, optional `label`, `env_set`, `env_remove`, `extras_enabled`), `account.remove` and `account.sign_out` (with `worker_id` and `account_id`), `acp_registry.list` (the result has `agents`, each with `id`, `name`, `description`, `version`, `icon_url` and `supported`) |
| Sign-in | `sign_in.start` (with `worker_id`, `account_id` and `method`; the result has `sign_in`), `sign_in.submit_code` (with `sign_in_id` and `code`), `sign_in.cancel` (with `sign_in_id`) |
| Preferences | `preferences.set_favorite_model` (with `account_label`, `model` and `favorite`; the result has `preferences`) |
| Composio | `composio.set_key` (with `api_key`), `composio.remove_key`, `composio.status`. Each result has `status`: `key_present`, `key_hint` (the last 4 characters), `plan`, `usage` (`used`, `limit`, `resets_at`), `error` and `checked_at`. Never the key |
| Connector catalog | `connector_catalog.search` (with optional `search`, `category`, `cursor`, `limit`; the result has `services` and `next_cursor`), `connector_catalog.categories`, `connector_catalog.service` (with `service`; the result has `service` and its `tools`) |
| Connections | `connection.connect` (with `service`, `name`, optional `use_in`, `approvals`, `auth_scheme`, `credentials`, `approval_id`; the result is a connect attempt: `connection`, `redirect_url` and `expires_at`), `connection.update` (with `connection_id`, optional `name`, `use_in`, `approvals`), `connection.disconnect`, `connection.reconnect` (with `connection_id`, optional `credentials`), `connection.calls` (with `connection_id`, optional `before`, `limit`; the result has `calls`, newest first, and `has_more`) |
| Channel exceptions | `channel.set_connection_enabled` (with `channel_id`, `connection_id`, `enabled`), `channel.set_skill_enabled` (with `channel_id`, `skill_id`, `enabled`). The result has `channel` |
| Skills | `skill_directory.search` (with optional `search`, `source`, `cursor`, `limit`; the result has `entries`, `next_cursor` and `unavailable`), `skill_directory.sources`, `skill_directory.add_source` (with `url`), `skill_directory.remove_source` (with `id`), `skill.preview` (with `source`), `skill.install` (with `source`, optional `use_in`, `enabled`, `approval_id`; the result has `skill`), `skill.update` (with `skill_id`, optional `enabled`, `use_in`, `accept_new_version`), `skill.remove`, `skill.files` (with `skill_id`, optional `version`: `installed` or `pending`), `skill.read_file` (with `skill_id`, `path`, optional `version`) |
| Clients | `client.pairing_code`, `client.list`, `client.revoke`, `client.push_register` |
| Auth | `auth.http_token` |

`message.send` with a `client_message_id` that the hub already stored returns the stored message. A client can retry a send without making a duplicate.

A client uploads each attachment with `PUT /v1/blobs/{sha256}` before `message.send`. The hub refuses an attachment whose blob it does not have, with `invalid`, and takes the `size` from the stored blob. The attachments of a user message go with the message to the session: in the coordinator's turn, or in the task's turn, steer or inbox for a message in a task thread.

In `channel.update` and `task.update`, a field that is left out does not change. For a field that can be cleared, such as `icon` or `start_at`, `null` clears it. `channel.update` also takes `allowed_local_mcp_servers`, the list of local MCP servers that a task from the assistant may use. The hub trims the names, drops copies, and refuses `hub` and names with `__`.

`channel.create` and `channel.update` take `coordinator_selection` and `task_selection`. In `channel.update`, `null` takes the channel's own selection away, so the channel uses Tutti's. A change to the coordinator selection that a channel uses (its own, or Tutti's for a channel that has none) ends the channel's idle coordinator session at once, and a running one after its turn. The next turn starts a session with the new model and options. A model option such as `effort` goes to the worker in `session.start` `options`.

`channel.create` and `channel.update` take `tags`. `worker.set_tags` replaces the user tags of a worker. The hub trims each tag, drops repeats, and refuses a tag that is not 1 to 32 lowercase letters, digits or `-` with `invalid_tag`. A channel's tags are requirements: a coordinator turn, a task or a routine run of the channel runs only on a worker that has every tag, from its user tags or its inventory tags. This is added to any `require.tags` of a task. `worker.set_labels` and the `labels` fields are gone; a `labels` field in an old body still reads as `tags`.

`preferences.set_favorite_model` adds a star to a model when `favorite` is true, and takes it away when it is false. The hub trims the account label and the model. It refuses an empty account label, a field of more than 200 characters, and a new favorite when the list has 200, with `invalid`. The account and the model do not have to exist now. A request that changes nothing writes nothing. A change sends a feed upsert of the `preferences` entity with the whole record, so every client updates. A client sets one favorite for each request, not the whole list, so two clients that star at the same time do not undo each other.

The `acp` of `account.create` has a `kind`: `registry` with `id`, or `command` with `program` and `args`. `acp_registry.list` reads the ACP registry (`https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`), which the hub caches for one hour. An agent is `supported` when it has a distribution that the worker can install for at least one platform: `binary` with a SHA-256, or `npx`.

Account, sign-in and tool requests go straight to the worker. The hub answers with the result of the worker. A request for a worker that does not exist or was removed fails with `worker_unknown`. A request for a worker that is offline fails with `worker_offline`. The hub never queues these commands.

The hub makes the `account_id` of `account.create` and sends it to the worker. It checks the label, the harness, the `acp` spec and the names of the variables before it asks the worker, and it fails with `invalid`. It maps a worker error to a client error: `invalid`, `not_found`, `conflict`, `policy_denied`, `no_matching_account`, `sign_in_failed`, `tool_install_failed` and `unsupported` pass through with the message of the worker. `unauthorized`, `protocol_mismatch`, `stale_write`, `resync_required`, `internal` and a code that the hub does not know become `sign_in_failed` for `sign_in.*`, `tool_install_failed` for `worker.tools_check` and `worker.tool_install`, and `internal` for the `account.*` requests. If an error message of the worker repeats a secret value of the request, the hub replaces the value with `<redacted>`.

`sign_in.start` fails with `worker_offline` before the hub stores anything. If the worker refuses the command, the hub sets the sign-in to `failed` with the message of the worker, and returns the error. `sign_in.submit_code` works only while the sign-in is in `needs_code`, and fails with `conflict` in other states. `sign_in.cancel` asks the worker to stop, sets `cancelled`, and does nothing for a sign-in that has ended. A `sign_in.update` for an unknown sign-in, for a sign-in of another worker, or for a sign-in that has ended changes nothing.

The hub ends a sign-in with `expired` when no update comes for 15 minutes (it checks every 5 seconds). When a worker disconnects, each of its sign-ins that has not ended becomes `failed` with "The worker went offline. Start the sign-in again." The hub deletes a sign-in one day after its last update.

`task.retry` works for a Failed task, and for a task in Ready for review or Resolved whose session has ended. `task.reopen` (Not done) fails with `conflict` when archive deleted the workspace of the task. Retry is the way on then.

#### Connectors and skills

The connect flow:

1. The client sends `connection.connect`. The hub checks the name and the scope, makes or reuses a Composio auth config (Composio-managed first), and calls Composio's `connected_accounts/link` with an alias.
2. The hub stores the connection in `connecting` with the `redirect_url`, and answers with the connect attempt. The client opens the URL in the system browser.
3. The hub polls Composio until the account is active, for at most 10 minutes. Each change is a feed upsert of the connection: `active` with the `identity`, or `failed` with a message.

A scheme with no browser step, such as an API key, sends the form values in `credentials`. They go to Composio once and Consort does not store them. The `Debug` output of a request with `credentials`, and of `composio.set_key`, hides the values. The hub keeps the Composio key encrypted at rest and never returns it.

`connection.connect` with an `approval_id` answers a connect card: the hub marks the card allowed and puts the `connection_id` on it. `skill.install` with an `approval_id` answers an install card in the same way. `approval.answer` on any hub card works too: the hub then acts with the settings on the card.

`skill.install` fetches the skill at a commit, checks the format and the limits, and refuses path traversal, symlinks that leave the folder, and absolute paths, with `skill_invalid`. A skill whose audit failed is refused with `skill_blocked`. A skill with scripts is installed off. The hub removes Claude-specific `hooks`. An upload is a zip file that the client sends first with `PUT /v1/blobs/{sha256}`, at most 25 MiB. `skill.preview` does the same checks without installing, so the add dialog can show the facts first.

The hub checks the sources of installed skills once a day. A new commit sets `update` to `available` with the changed files. Only `skill.update` with `accept_new_version` moves the pin. A new version that adds or changes a script turns the skill off until the user turns it on again. Running sessions keep their version.

`channel.set_connection_enabled` and `channel.set_skill_enabled` change one exception for each request, so two clients do not undo each other. Turning an item on removes the exception only. An item whose `use_in` does not give it to the channel fails with `conflict`.

### Push notifications

The hub sends a push notification to each mobile client when:

- an approval or a decision opens,
- a task goes to `review` or `failed`,
- the assistant posts in a channel and no client has that channel open.

The payload holds a title, the channel ID and the entity ID. It holds no message text unless the user turns on message previews.
