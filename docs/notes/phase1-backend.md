# Phase 1, backend part: files and attachments

These notes are for the merge into `docs/DECISIONS.md`. The plan is [FINISH-PLAN.md](../FINISH-PLAN.md), phase 1, items 3 and 4. The client and desktop part is another build. Written on 2026-10-29.

## What was built

### Hub (`crates/consort-hub`)

- **User messages.** `message.send` refuses an attachment whose blob the hub does not have (`invalid`, "Upload it, then send the message again"). It takes the `size` from the stored blob. The attachments of the user messages in a batch of wake events go into the coordinator's turn (`event_attachments` in `coordinator.rs`): the first turn in `session.start` and each later `session.turn`. The hub reads them from the stored messages, so a deleted message gives none.
- **Messages to a task.** A user reply in a task thread and `message_task` carry their attachments to the task session: in a `session.turn`, in a `session.steer`, or in the task inbox (`ops/flow.rs`, `deliver`). The inbox keeps the attachments (new column `task_inbox.attachments`), and the next turn after `turn.ended` carries them (`inbox_batch`). A steer that the worker refuses goes to the inbox with its attachments.
- **Coordinator tools.** `start_task`, `message_task` and `reply` take `attachments` (channel file paths), as `post_message` did. The hub turns each path into a `BlobRef` (`resolve_attachments`). An unknown path is an error result. The attachments of `start_task` go with the first turn of each session of the task, also after Retry (new table `task_attachments`). `message_task` posts its message in the task thread with the attachments.
- **Channel files.** `session.start` gets the channel files without the files under `artifacts/<own task id>/` (`knowledge::session_channel_files`). `session.turn` gets `channel_files` only when the list changed since that session got it (see "Channel file refresh").
- **Artifacts.** `publish_artifact` already stored the channel file with `task_id` and `title`, and the feed already carried the upsert. One fix: the hub takes the size from the stored blob, not from the call.
- **Notices.** A worker error event with the code `attachment_failed` also becomes a notice: in the task thread, or in the channel timeline for a coordinator.
- **Store.** Migration `schema_v7.sql`: `task_inbox.attachments`, `task_attachments`, `session_channel_files`. Task and channel deletes remove the new rows.

### Worker (`crates/consort-worker`)

- **Download and cache** (`attachments.rs`). `BlobCache` keeps checked blobs in `<consort home>/cache/blobs/<sha256>` (folder 0700, files 0600 on Unix). A download goes to a temporary file with the signed `GET /v1/blobs/{sha256}` (`http::download_blob_to_file`, streamed, at most 256 MiB, 30 s connect limit and 60 s read limit). The worker checks the SHA-256, then renames the file into the cache. Each use checks the hash of the cached file again, and a bad cached file is fetched again.
- **Attachments.** Before the harness gets a turn or a steer, `save_attachments` copies each attachment from the cache to `<workspace>/.consort/attachments/<12 hex>-<name>`. A failure removes any old file at that path, so the adapter says that the file is not available. The worker sends one `attachment_failed` error event for each failure, and the turn goes on.
- **Channel files.** `ChannelCopy` makes `<workspace>/.channel/files/` match the list. It writes new and changed files as read-only copies (0444, or the read-only flag on Windows), removes files that are no longer in the list, and replaces a file that the session changed. It removes links and other odd entries and never follows them. It refuses an unsafe path (`..`, absolute, backslash, colon, control characters). After a worker restart it checks the hash of each present file once, so it does not download again.
- **Session runtime** (`sessions.rs`). The start makes the copy before the harness opens and saves the first turn's attachments before the first turn. `session.turn` refreshes the copy when it has a list, saves the attachments, then starts the turn. A steer saves its attachments before it goes to the harness. A new list is also stored in the session record (`Store::set_channel_files`), so a resumed session knows it.
- **Git.** In a workspace that is a git checkout, the worker adds `/.channel/` and `/.consort/` to the repo's `info/exclude` (`workspace::exclude_worker_folders`). So `git add -A` in a session does not commit them.

### Harness (`crates/consort-harness`)

- **`attachments.rs`.** The path rule (`attachment_path`, `sanitize_file_name`), the image check from the first bytes (`sniff_image`: PNG, JPEG, GIF, WebP), and `prepare_turn`, which gives the turn text with the attachment block and the images to send. `turn_text(input, workspace)` replaces the old name-only list.
- **The attachment block.** After the turn text, one line for each attachment: the sanitized name, the media type (cleaned), the size, and the absolute path. A file that the worker could not get says so. An image that goes as image input says "also in this message as an image".
- **ACP** (`acp/mod.rs`, `prompt_blocks`). An `image` content block with base64 `data` and the `mimeType` from the bytes, after the text block, in `session/prompt` and in `_session/steering`. Only when the agent reports `agentCapabilities.promptCapabilities.image`, and only up to 5 MiB per image. The session's `images` capability comes from that report. Base64 is a small local function, so no new crate.
- **Codex** (`codex/mod.rs`, `turn_input`). A `localImage` item with the absolute path, after the text item, in `turn/start` and `turn/steer`, up to 20 MiB per image.
- **`images` capability.** On for Claude Code, Gemini CLI, OpenCode (static, refined by the agent's report at probe and at open) and Codex. Off for other ACP agents until they report it.

### Proto (`crates/consort-proto`), additive only

- `SessionTurn.channel_files: Option<Vec<FileRef>>` (left out when `None`).
- `attachments: Vec<String>` (channel file paths, default empty) on `StartTaskArgs`, `MessageTaskArgs` and `ReplyArgs`. The tool descriptions of `post_message`, `reply`, `start_task` and `message_task` say how to attach.

## Image input per harness

I read the source of each harness. A research subagent found the places, and I checked the Gemini CLI and Codex claims again on this machine. No live model turn was run (see "Checked by hand").

| Harness | What the adapter sends | Capability report | Source |
|---|---|---|---|
| Claude Code, `@agentclientprotocol/claude-agent-acp` 0.99.1 (`npm pack`) | `{"type":"image","data":<base64>,"mimeType":"image/png"}`. `promptToClaude` turns it into `{type:"image", source:{type:"base64", data, media_type}}`. A `file://` `uri` in an image block is dropped, so the adapter always sends `data`. `_session/steering` uses the same `promptToClaude`, so a steer can hold images. The adapter does not check the type, and the Claude API takes only PNG, JPEG, GIF and WebP | `initialize` gives `promptCapabilities: {image: true, embeddedContext: true}` | `dist/acp-agent.js` lines 1428 to 1431 (capabilities), 10316 and 10363 to 10385 (`promptToClaude`), 2706 to 2709 (steer calls `promptToClaude`); `dist/blob-resources.js` line 2 (the four types). Tag `v0.99.1` of agentclientprotocol/claude-agent-acp |
| Gemini CLI 0.46.0 (Homebrew, `/opt/homebrew/lib/node_modules/@google/gemini-cli/bundle/`) | The same image block. `#resolvePrompt` turns `image` and `audio` into `{inlineData: {mimeType, data}}` and ignores `uri` | `promptCapabilities: {image: true, audio: true, embeddedContext: true}` | `gemini-YXO2QQ66.js` lines 15285 to 15289 and 14460 to 14466 (checked again by hand) |
| OpenCode 2.2.2 | The same image block. With `data`, OpenCode makes a `data:<mime>;base64,` URL and sends it as a file part. A block with only `uri` becomes a text link or a file read | `service.ts:124`: `promptCapabilities: {embeddedContext: true, image: true}` | <https://github.com/anomalyco/opencode/blob/v2.2.2/packages/cli/src/acp/> `service.ts:124`, `prompt.ts:22-24`, `content.ts:28-34` |
| Codex, codex-cli 0.166.0 | `{"type":"localImage","path":"/abs/path.png"}` in `turn/start` and `turn/steer`. Codex reads the file, scales it to fit 2048 px, and takes PNG, JPEG, GIF and WebP. An `http` image URL is refused, and a relative path would resolve against the app-server folder, so the adapter sends absolute paths | No flag. `UserInput` has `localImage` | `codex app-server generate-json-schema` on this machine: `UserInput` has `TextUserInput`, `ImageUserInput`, `LocalImageUserInput (path, detail)` and others (checked again by hand). Source: `codex-rs/app-server-protocol/src/protocol/v2/turn.rs` lines 456 to 490 at tag `rust-v0.166.0`; `codex-rs/utils/image/src/lib.rs` lines 26, 31 and 453 to 456 |

`CLAUDE_AGENT_ACP` in `acp/mod.rs` names 0.99.0, and the research read 0.99.1. The image path did not change between the two as far as the research could see, but this is not checked.

## Channel file refresh

**Choice: a field on `session.turn`, sent only when the list changed.**

- At `session.start`, the hub sends the full list and stores a SHA-256 digest of it (paths and hashes) per session in `session_channel_files`.
- At each `session.turn`, the hub builds the list again (`Tx::channel_files_for_turn`). When the digest is the same, `channel_files` is left out. When it changed, the turn carries the full list and the hub stores the new digest.
- The worker makes its copy match a list before the turn starts. Without a list it does nothing, except that it tries the files again that failed last time.
- A steer does not refresh the copy. The next turn does.

Why not the other ways:

- **A version number** needs a second request so that the worker can get the list, and a new worker request type.
- **The feed** goes to clients, not to workers. The worker would need a new subscription.
- **The full list on every turn** is simple, but it sends the same list again and again, and a large list takes room in the 1 MiB frame on every turn. With the digest it goes only after a change, as in `session.start` today.

A known gap: when the worker gets a new list but the turn then fails on the hub side before the worker sees it, the hub has the new digest and the worker does not have the list. The next change sends the full list again. A start of a new session always sends the full list.

## Decisions

| # | Area | Decision | Why | Easy to change? |
|---|---|---|---|---|
| P1B-1 | Proto | `session.turn` has an optional `channel_files`. The hub sends it only when a digest of the list changed since the session got it. | No second request, no new worker subscription, and no repeated list in every frame | Yes |
| P1B-2 | Proto | `start_task`, `message_task` and `reply` take `attachments` as channel file paths, like `post_message`. | A model cannot upload a blob, but it can see channel files (decision 22). protocol.md listed attachments only for `post_message` | Yes |
| P1B-3 | Hub | The attachments of `start_task` go with the first turn of every session of the task, also after Retry. They are not on the task card. | The brief goes with every first turn too. The card shows only the title | Yes |
| P1B-4 | Hub | The coordinator's turn reads attachments from the stored messages, not from the wake event. `WakeEvent` did not change. | No proto change, and a deleted message gives no attachments | Yes |
| P1B-5 | Hub | `message.send` refuses an attachment with no stored blob, and takes the size from the stored blob. | The worker would fail the download later, where the user sees it less clearly. `files.put` does the same check | Yes |
| P1B-6 | Worker | Attachments go to `<workspace>/.consort/attachments/<12 hex of the hash>-<sanitized name>`. Names lose separators, control characters and Windows-reserved characters, lose dots and spaces at the ends, and have at most 100 bytes with the extension kept. | The hash prefix makes names unique and never a Windows device name. The workspace is a place every harness may read without an approval | Yes |
| P1B-7 | Worker | One blob cache for attachments and channel files, `<consort home>/cache/blobs/<sha256>`, with the hash checked at each use. No prune yet. | A file is downloaded once per worker. A check at each use catches a changed cache file | Yes |
| P1B-8 | Worker | The channel file copy is a real copy with read-only files, not a link to the cache. The worker replaces a file that the session changed, and removes files that the session added. | A write to a link would change the cache. Read-only files show the session that the copy is not the place to write | Yes |
| P1B-9 | Worker | The worker adds `/.channel/` and `/.consort/` to `info/exclude` of a git workspace. | A session that runs `git add -A` must not commit the channel files or the attachments into the user's repo | Yes |
| P1B-10 | Worker | A failed attachment or channel file sends an `attachment_failed` error event, and the hub posts its message as a notice. The turn still runs, and the attachment block says that the file is not available. | One failure must not stop a turn. The user sees which file is missing, and the model can ask for it | Yes |
| P1B-11 | Worker | A steer with attachments downloads them in the session actor before the steer goes to the harness. An interrupt that comes during that download waits for it. | The harness must be able to open the files. A cached file takes milliseconds | Yes, with a separate download task |
| P1B-12 | Harness | Images go as image input only for PNG, JPEG, GIF and WebP, read from the first bytes, up to 5 MiB for ACP and 20 MiB for Codex. Other files go by path only. | All four harnesses take these types. The Claude API limit is 5 MB per image, and Codex scales large images itself | Yes |
| P1B-13 | Harness | An ACP session sends image blocks only when the agent reports `promptCapabilities.image`. The static `images` is on for Claude Code, Gemini CLI and OpenCode, and the probe takes the agent's report. | The report is the ACP way to say it. The static value was checked in the source of each agent | Yes |
| P1B-14 | Harness | The attachment block uses absolute paths. | The path works in every harness and from every folder in the session | Yes |

## Tests

- **Hub** (`tests/attachments.rs`, 9 tests): a user attachment in the first and a later coordinator turn, and a missing blob refused; the channel file list on start and only on change; `start_task` attachments on each session and own artifacts hidden; `message_task` by steer and by turn, with the thread message; a thread reply in the inbox when the account cannot steer; `publish_artifact` end to end with the true size and the feed upsert; `post_message` and `reply` attachments; the `attachment_failed` notice for a task and for a coordinator. Also a migration test for version 7 and an update of `message_send_checks_its_input`.
- **Worker** (`src/attachments.rs`, 7 unit tests; `src/store.rs`, 1 test for the stored list; `tests/attachments.rs`, 4 tests with the fake hub): download, signed GET, hash failure, a missing blob, a file larger than 256 MiB, name sanitizing, the cache and its re-check, links in the workspace, the read-only copy and its refresh, unsafe paths, a restart that keeps a current copy, a steer with an attachment, and the git exclude.
- **Harness** (`src/attachments.rs`, 11 unit tests; `acp/tests.rs`, 3 tests; `codex/tests.rs`, 1 test): names, sizes, base64, image detection, the block text, a link in place of a saved file, inline and path images within the limit, Claude image blocks in a turn and a steer, paths only when the agent does not report images, and Codex `localImage` items.
- **Ignored real-harness tests** (`tests/real_harness.rs`): `claude_image_turn`, `codex_image_turn`, `gemini_image_turn` and `opencode_image_turn`. Each saves a red 32 by 32 PNG where the worker saves attachments and asks for its color. They use the user's plan, so they did not run in this build.

## Checked by hand

- The Gemini CLI 0.46.0 bundle on this machine: the `promptCapabilities` and the `image` case of `#resolvePrompt` (lines above).
- `codex app-server generate-json-schema` from codex-cli 0.166.0 on this machine: the `UserInput` variants, with `LocalImageUserInput` needing `path` and `type`.
- The claude-agent-acp 0.99.1 package: `_session/steering` builds the user message with `promptToClaude`, so image blocks work in a steer.
- No live model turn with an image ran. Run the ignored tests above to check each harness end to end.

## What is left

- **A live image check** with each signed-in harness (the ignored tests). Then the static `images` values have a live test too. OpenCode needs a model with image input.
- **Pruning the blob cache.** It only grows now. A prune by age and total size is next to the skill cache prune in the plan's "Smaller items".
- **Attachment folders are never cleaned.** `.consort/attachments/` keeps the files of every turn of a workspace. A coordinator workspace lives as long as its session. A cleanup when the session ends, or a size limit, is open.
- **Interrupt during a long download.** A steer or a turn with a large uncached attachment holds the session actor until the download ends (P1B-11).
- **The digest gap** in "Channel file refresh": a turn that the hub records but the worker never gets leaves the worker one change behind until the next change.
- **`files_list` and `start_task` sizes.** The hub does not limit how many attachments a message or a tool call has. A frame holds the list, so a very long list fails at the frame limit.
- **Pinned claude-agent-acp version.** The research read 0.99.1, and the constant names 0.99.0. The install resolves `latest` (`consort-tools`). This is outside this build.
- **The client part** of phase 1 (upload with progress, chips, bubbles, the Files panel) is the other build. It must upload each blob before `message.send`, because the hub now refuses a missing blob.
