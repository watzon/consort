# Plan: finishing the app

Status: written on 2026-10-29 from a gap analysis that compared three things: what the design docs promise (about 200 items), what the code does today, and every open item in the E2E notes, the build notes and the plans. The user sets the order.

## Where things stand

- **The hub is nearly complete.** All 93 client requests and all 62 hub tools work. The gaps on the hub are few: it sends no push notifications, it has no backup, it has no TLS of its own, and some connector and skill details are missing (see "Smaller items").
- **The desktop app reaches about half of the hub.** The app never sends these requests:
  - `memory.*`, `routine.*`, `files.*`
  - `message.react`, `message.search`
  - `task.create`, `task.update`, `task.delete`, `task.items`, `task.item_detail`
  - `repo.create`, `group.delete`, `group.reorder`
  - `client.*`, `auth.http_token`, `worker.refresh_inventory`

  The client cache drops the feed entries for routines, memory notes, files and clients (`consort-client/src/cache.rs`).
- **Five Settings pages are placeholders:** Notifications, Clients, Hub, Memory and Routines.
- **Stubs that the user sees:**
  - Attachments in the composer and in the task reply box.
  - Voice input.
  - Reply in thread.
  - "Add a repo" in the New channel dialog.
  - Skill zip upload.
  - New task, which only fills the composer.
- **The worker does not download attachments or channel files.** The harness gets only a list of file names (`consort-harness/src/lib.rs` `turn_text`, `consort-worker/src/sessions.rs`). No adapter sends images.
- **Nothing to install yet.** There is no app bundle, no worker installer, no service install, and no mobile client.

## Phases

Each phase starts with a Paper design where it has new screens, as in the earlier builds. Then contract notes (if the proto changes), then parallel builds with disjoint crates, then a check in the running app.

### Phase 1: files and attachments

Many other features need this, so it comes first.

1. **Client blob transfer.** Use `auth.http_token`, and add upload and download with a progress display.
2. **Composer attachments.** The "+" button, drag and drop, and paste of images. Attachment chips in the composer. Attachments in bubbles: image thumbnails and file chips, with open and save.
3. **The worker downloads attachments and channel files** into the session (`.channel/files/`, and the turn's attachments). This replaces the name-only list.
4. **Images to the harnesses** that take them: Claude Code over ACP, Codex and Gemini CLI. The `images` capability is set only after a test.
5. **Channel Files.** A panel or a dialog per channel: list, upload, move, delete, and download. Task artifacts are grouped by task.
6. **Attachments in the task reply box**, and **skill zip upload** (it reuses the blob upload).

Design first: the composer with chips, attachments in bubbles, the Files panel.

### Phase 2: routines and memory

1. **Routines.**
   - A list per channel, and a Settings > Routines page with every routine.
   - A create and edit dialog with a schedule picker. The picker covers "every weekday at 8:00", every N minutes or hours, once at a time, and raw cron for experts, with a time zone.
   - The action: an assistant turn or a task.
   - Run now, pause, and the run history (`routine.runs`).
   - Routine rows use the metronome icon.
2. **Memory.**
   - Settings > Memory for global notes, and a Memory view per channel.
   - Read, edit (with the stale-write check), delete, and the version history with a restore.
   - Show memory suggestions, so that the user sees what the assistant chose to keep.
3. **The client cache keeps these entities** and reduces them.

Design first: the routine dialog and list, and the memory pages.

### Phase 3: hub, clients and notifications settings

1. **Settings > Hub.**
   - The address, version and protocol version, the data folder, and storage use (database, blobs, skills).
   - Backup: "Back up now", and a schedule to a folder on the hub machine, with SQLite online backup plus the blobs. Restore is a documented CLI command.
   - This needs new hub requests (`hub.status`, `hub.backup`, `hub.backup_settings`) and a CLI `consort hub restore`.
2. **Settings > Clients.** The paired apps (`client.list`), Revoke, and "Pair a new app" with a setup code (`client.pairing_code`) and a QR code.
3. **Notifications on the desktop.**
   - OS notifications for an approval or a decision that waits, a task in Ready for review or Failed, and a message in a channel that is not open.
   - A dock badge with the count that needs the user.
   - Message previews are off by default.
   - Settings > Notifications holds these switches.
   - Check how GPUI can post a notification without `unsafe` code before the design.

Design first: the three pages.

### Phase 4: conversation and tasks, complete

1. **Threads on messages.** Reply in thread opens a thread panel. The reply count shows under the message.
2. **Reactions.** Show them, and add one from the message menu.
3. **Search.** Cmd+K also searches messages, memory and tasks on the hub (`message.search`, the `search` index). Results open in place.
4. **New task dialog** (`task.create`): title, brief, channel, model and effort, placement, approval mode, and start now or at a time. Task delete. Retry also for a task in review or resolved whose session ended.
5. **Task activity in full.** Open an item to see its whole command output or diff (`task.items`, `task.item_detail`).
6. **Channel settings in one dialog:**
   - purpose, instructions, model and tags
   - repos (`repo.create`, attach and detach)
   - default isolation and approval mode
   - allowed local MCP servers
   - memory on or off

   Groups: delete and reorder.
7. **Task thread in bubbles**, the same as the timeline. And an **About** screen with the mark.
8. **Voice input.** A decision for the user: system dictation, a local Whisper model on the worker, or later.

Design first: the thread panel, the search results, the New task dialog, the channel settings dialog, and the task item view.

### Phase 5: install and run anywhere

1. **Desktop bundle:**
   - macOS: a `.app` with the `.icns` from `design/logo/`, signed and notarized when the user has a Developer ID.
   - Windows and Linux: packages.
   - App updates.
2. **Worker install.** A real installer: `consort worker install` sets up a launchd agent, a systemd user unit or a Task Scheduler task. This replaces "There is no installer yet" in the Add worker dialog.
3. **Hub deployment.** A systemd unit, TLS (built-in ACME or a documented reverse proxy), and a Docker image if wanted.
4. **A persistent client cache** (SQLite), so the app opens with its data while the hub is offline.
5. **Windows checks** of everything above, on bigboi.

### Phase 6: mobile and push

1. **The `client-ffi` crate** with UniFFI over `consort-client`.
2. **A spike** of Expo and native, then a choice (ADR 0006 leaves this open).
3. **Hub push** through APNs and FCM (or Expo), on the triggers in `protocol.md`.
4. **The mobile app:** Tutti, channels, Tasks, the task thread, and answers to approvals and decisions.

### Phase 7: harness breadth

- The native Pi adapter.
- ACP agents that use `uvx`.
- The `fork` and `plan_mode` capabilities.
- MCP elicitation.
- The Gemini CLI skill tools on Windows (the hub hides `list_skills` from all Gemini CLI sessions; the Windows worker needs them).
- Extras counts for Codex and OpenCode.

## Live checks that need the user

These can run at any time, and they find real bugs early.

1. **Composio run.**
   - Your key, one connection, and a connector call that asks for approval.
   - Check the tool hint names and the identity field.
   - Check two Gmail accounts at the same time.
2. **A real sign-in** to Claude and to ChatGPT in the app, then one model turn with the model and effort from the picker.
3. **Skills at the model level.** Do Codex and Gemini CLI models see the Consort skills? Does Claude Code ask before it uses the `Skill` tool in Ask mode?
4. **bigboi.** Sign in again (Claude Code and Codex), run a task on Windows, and check the OpenCode and Gemini CLI paths there.
5. **The extras switches** for Codex and OpenCode with signed-in accounts. Also explain the `deep-research` skill that stays visible with extras off.

## Smaller items

These are fixed during the phases that touch the same code.

- **Hub:**
  - Composio status: plan, usage, and a key that Composio refuses later.
  - The logo cache.
  - Connections after a change to another Composio project.
  - Private git repos and ssh URLs for skills.
  - The skill branch for directory entries.
  - Marketplace plugin sources (`archive`, `npm`, `command`).
  - A list and restore for archived tasks.
- **Worker:**
  - A prune of the skill cache.
  - An expired Claude login that the probe cannot see.
  - The OpenSSH `HOME` note for OpenCode.
- **Desktop:**
  - The approval card "Folder" label alignment.
  - The bottom fade of dialog lists before the first scroll.
  - Reconnect for form-based connections without a loaded catalog.
  - A diff view for a new skill version.
- **Docs:**
  - `protocol.md`: the connector wait is 4 minutes 45 seconds, and the `assistant_change` notice.
  - `data-model.md`: `hub-secret.key`, `composio.key` and the logo cache.
  - Remove the step 2 placeholder list from the connectors plan.
- **Tests:** git and HTTP time limits with a slow server.
