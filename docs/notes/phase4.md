# Phase 4: conversation and tasks, complete

These notes are for the merge into `docs/DECISIONS.md`. The plan is [FINISH-PLAN.md](../FINISH-PLAN.md), phase 4, without the parts of item 7 that phase 4b builds (the `channel_created` greeting and guided setup, the quick-reply chips and the writing bubble). The designs are in [design-phases-3-5.md](design-phases-3-5.md). Written on 2026-10-30.

## What was built

### Proto (`crates/consort-proto`), additive only

- `Task.session_ended` (serde default false): true when the current session of the task ended. The hub keeps it in the task's JSON document, so no migration.
- `MessageSearch.kinds` (left out when empty): `message`, `memory_note` or `task` (`tools::SearchKind`). `SearchHit::MemoryNote` and `SearchHit::Task` have `updated_at` (left out when `None`). `SearchHit::kind()`.
- `task.list_archived` (`TaskListArchived` with optional `channel_id`, `offset`, `limit`; `TaskListArchivedResult` with `tasks`, `tasks_cut`, `has_more`).
- `FileChange.diff` (left out when `None`): the unified diff of a file change. Only the harness adapter and the worker see it. The worker takes it out before the item goes to the hub.

### Hub (`crates/consort-hub`)

- `session_ended`: set in `finish_session` for the current session of a task (worker ends, losses and hub ends all pass there), and when archive releases a resolved task. A new session (placement, the worker's start result, a late allowed start) and Retry set it back to false. The Retry rule itself was already right (`retry_task`: failed always; Ready for review and Resolved only when the session ended), so it did not change.
- `message.search` with `kinds`: the filter is in the SQL query (`instr` on a list of kinds), so a search for memory notes is not crowded out by messages. Memory note hits read the note's `updated_at`; task hits carry the task's.
- `task.list_archived` (`ops/sync.rs`, `store/work.rs::archived_tasks`): newest archive first, 50 by default, 200 at most, briefs and summaries cut to the 1 KiB preview as in a snapshot. Restore is the existing `task.archive` with `archived: false`.
- No migration (phase 2 owns 8 and phase 4b takes 9).
- Tests: `tests/phase4_tasks.rs` (3: `session_ended` and Retry clearing it, Retry of a resolved task whose session ended, the archived list with pages, the channel filter, the previews and Restore), the kind filter in `memory_and_blobs.rs`, and the updated unit tests.

### Worker and harness (`crates/consort-worker`, `crates/consort-harness`)

The worker did not answer `item.detail` (DECISIONS row 90: "answers `not_found`"). The item view needs it, so:

- `item_details.rs`: the worker keeps the whole output of each command item (`<item>.out`, from the output deltas, or from the finished item when the adapter sends no deltas) and the diff of each file change (`<item>.diff`) in `<consort home>/cache/items/<session>/`, up to 16 MiB per item with a note at the cut. `item.detail` uploads the file as a blob (signed `PUT`) and answers `sha256`, `size` and `mime` (`text/plain` or `text/x-diff`). An item with nothing kept is `not_found` with a sentence for the app.
- The ACP adapter makes a unified diff from the `oldText` and `newText` of a `diff` block (`similar` 2.7.0, already in the tree). The Codex adapter passes the diff that Codex sends.
- Tests: 4 unit tests in `item_details.rs`, `tests/item_detail.rs` against the fake hub (a 500-line output comes back whole as a blob), and the diff checks in the ACP and Codex mapper tests.

### Client (`crates/consort-client`)

- `convo.rs` (pure): reaction chips (`reaction_views`, one chip per emoji, the count only when both chose it), the six quick emoji and the picker groups, reply labels, the search scope and its kinds, local results and `highlight` (bold runs), the item view (`ItemDetailView`, `diff_lines`, `text_diff`, `output_lines`, labels), archived tasks, the New task form (`NewTask`, `RunsOn`, `StartWhen`, `day_choices`, `time_choices`, `start_time`, `placement_caption`, `approval_caption`), the channel settings helpers (`RepoChoice`, `extensions_summary`) and `can_retry`.
- `reduce_convo.rs`: the repos of a channel with their checkouts ("Checked out on bigboi and MacBook Pro"), the loaded message threads, search hits as rows, the archived tasks. `MessageView` has `reactions` and `last_reply_label`; `ChannelView` has `default_approval_mode`, `allowed_local_mcp_servers`, `memory_enabled` and real `repos`; `WorkerView` has `local_mcp_servers`; `TaskView` has `session_ended` and `can_retry()`; activity lines are `ActivityLine` with the session and item IDs. `ViewState.convo` holds the rest. `chat::rows` adds a `Footer` row under a message with reactions or replies.
- Commands (`hub/convo.rs`): `React` (shows at once, undoes on failure), `LoadMessageThread`, `FindMessage` (loads older pages until a search result is in the cache), `ReplyInThread` (uploads with `UploadTarget::Thread`), `Search` and `ClearSearch` (an answer to an older search is dropped), `CreateTask`, `DeleteTask`, `RestoreTask`, `LoadArchivedTasks`, `OpenTaskItem` and `CloseTaskItem` (place and header from `task.items`, text from `task.item_detail` and the blob; `worker_offline` becomes the offline state), `DeleteGroup`, `RenameGroup`, `ReorderGroups`. `ChannelChanges` has the isolation, the approval mode, the local MCP servers, memory and repos (a new URL is made with `repo.create` before the update). `NewChannel.icon`.
- `LoadServiceTools` also puts the service with its sign-in schemes in the catalog (for Reconnect, below).
- `mock_convo.rs`: the design content (the Tutti thread with 4 replies, the reactions, the sayso repos and servers, the item outputs and the diff, the search corpus, two archived tasks) and the mock commands.
- Tests: 14 in `convo.rs`, 1 in `reduce_convo.rs`, 2 in `mock_convo.rs`, a footer test in `chat.rs`, the activity line test in `reduce.rs`, and 8 fake hub tests in `tests/fake_hub_parts/phase4.rs` (reactions, thread load and reply, search scope, New task fields and refusals, archived pages and restore, the item and offline, channel settings with a new repo, groups).

### Desktop (`crates/consort-desktop`)

New files: `convo.rs` (state, inputs, actions, screens), `ui/convo_chat.rs` (quick emoji, chips, reply line, emoji picker, thread panel), `ui/convo_search.rs` (the palette), `ui/convo_dialogs.rs` (New channel, New task, channel settings, group dialogs, Delete task, About, the skill review), `ui/convo_item.rs` (item view), `ui/convo_sidebar.rs` (the "+" buttons, group menu, drag), `ui/convo_tasks.rs` (task thread bubbles, activity chip, task lines, archived tasks), `convo_tests.rs`.

- **Threads on messages (`95W-0`).** Reply in thread opens the thread panel on the right (one panel at a time). The root's bubbles get the accent ring in the timeline and in the panel. Under a message: chips and "4 replies, Last reply 9:31 AM", which opens the thread. The reply box has attachments (the add menu, paste and drop) and Enter sends.
- **Reactions (`9I9-0`).** The right-click menu of a message has the six quick emoji and More above Copy text, Copy as Markdown, Reply in thread and Quote in reply. A chosen emoji has the ring and a second click removes it. More opens our own grouped picker. A click on a chip toggles your reaction.
- **Search (`A0N-0`, `AFG-0`).** Command K: the scope filter (Tab and Shift+Tab), Go to at once, the hub results under it 220 ms after the last key, skeleton rows while it searches, bold matches, the footer counts, and "Nothing matches" with Ask Tutti to search. Enter or a click opens in place: a message scrolls its channel to it (older pages load first), a reply opens its thread (a task thread for a task card), a task opens its thread (an archived one opens the archived list), a memory note opens its channel (see "Memory views").
- **New task (`ARL-0`).** Title, brief, channel, model and effort (from the channel's task model), Any worker tagged or One worker, repo and isolation, approval mode with its caption, Now or a day and a time; Start task or Schedule task. The New task buttons open it. Delete is the last item of the task menu, with a confirm. Retry shows for Ready for review and Resolved when the session ended.
- **Task item view (`CA5-0`, `CPJ-0`).** A line of the activity chip opens the item: the panel widens to 560 px, output starts at the end (a uniform list, so a large output stays fast), diffs show both line numbers on tints, Copy output or Copy diff, Item N of M, the loading state, and the offline state with Try again.
- **Channel settings (`CYJ-0`).** One 880 px dialog in two columns: General, Assistant (model and effort, channel memory; the 4b agent adds the Proactivity row after Channel memory), Work (worker tags, repos with remove and Add a repo, default isolation, default approval mode, local MCP servers with their workers), Connectors and skills, Archive and Delete at the bottom left, and Save changes, which sends only the changed fields. Rename, Instructions, Model and Worker tags of the channel menu open it; their dialogs are gone.
- **New channel and groups (`EYQ-0`, `FOZ-0`, `FWA-0`).** New channel asks a name and an icon. The Tutti row shows "+" on hover (New channel, New group); a group row shows "..." (New channel, Rename, Delete group) and "+" ("New channel in Learning"). An empty group shows Add a channel. Delete group asks first and Enter does not confirm. Groups move by drag (a line shows where). The New channel row at the end of the sidebar is gone.
- **Task thread in bubbles (`BDH-0`).** The brief is the first bubbles with the placement as a caption; the placement table and the Brief section are gone. Task posts are bubbles, the activity is a chip, approvals and decisions have a task line.
- **About (`DWF-0`).** The mark, the wordmark, version and build, the hub (from Settings > Hub's status), the update section (Version X is ready with Restart to install, or the state with Check now), Get updates on their own (`Updates::set_auto`, `updates.json`), and four links. It opens from the "About Consort" footer row of the settings rail and from the app menu ("About Consort", action `ShowAbout`).
- **Voice input.** See the decision below. The stub mic button is gone; the composer's text field has a tooltip with the dictation shortcut.
- **Smaller items.** The approval card's Folder label lines up with its value. A dialog list (`faded_scroll`) draws its bottom fade before the first scroll. Reconnect of a connection whose service needs a form loads the service's schemes when no catalog page has, then opens the form. A skill with a new version has Review changes: a diff of each changed file (installed against pending) before Update.
- **Screens:** `--mock --screen message-thread`, `reactions`, `search`, `search-loading`, `search-empty`, `new-task`, `item-output`, `item-diff`, `item-loading`, `item-offline`, `channel-settings`, `task-thread`, `about`, `new-channel`, `new-channel-group`, `new-group`, `delete-group`, `sidebar-new`, `sidebar-group`, `archived-tasks`. The old `instructions` and `channel-tags` screens are gone with their dialogs.
- Tests: 17 window tests in `convo_tests.rs`, the reorder test in `convo.rs`, the caption test in `ui/convo_tasks.rs`, the any-word Go to test in `ui/switcher.rs`, and updated menu, picker and New task tests.

### Theme

- `component.convo` in `design/structure.json` (`Convo` in `structure.rs` and `tokens.rs`) with the test `convo_tokens_keep_the_values_of_the_phase_4_designs`. `docs/design-system.md` lists them.

### Screenshots (`docs/screenshots/`)

`phase4-message-thread-dark.png`, `phase4-reactions-dark.png`, `phase4-search-dark.png`, `phase4-search-loading-dark.png`, `phase4-search-empty-dark.png`, `phase4-new-task-dark.png`, `phase4-item-output-dark.png`, `phase4-item-diff-dark.png`, `phase4-item-loading-dark.png`, `phase4-item-offline-dark.png`, `phase4-channel-settings-dark.png`, `phase4-task-thread-dark.png`, `phase4-about-dark.png`, `phase4-new-channel-dark.png`, `phase4-new-channel-group-dark.png`, `phase4-new-group-dark.png`, `phase4-delete-group-dark.png`, `phase4-sidebar-new-dark.png`, `phase4-sidebar-group-dark.png`, `phase4-archived-tasks-dark.png`.

## Voice input: the decision

The task asked to start the system's dictation from the composer's mic button if that is possible without `unsafe` code, and else to remove the button and show the shortcut.

- AppKit starts dictation with the `startDictation:` action. `objc2-app-kit` 0.3.2 has no safe method for it: the only ways are `NSApplication::sendAction_to_from` (an `unsafe fn`, because it takes a raw selector and untyped objects) and `msg_send!` (unsafe). `unsafe_code` is forbidden in every crate. So the button cannot start dictation safely.
- Dictation through the system shortcut already types into the composer: GPUI's macOS window is an `NSTextInputClient`, and gpui-kit's `InputState` implements `EntityInputHandler` (`replace_text_in_range`, `replace_and_mark_text_in_range`), which dictation and other input methods use, marked text included. On Windows, voice typing (Windows key+H) uses the same text input path.
- So the stub button is removed, and the composer's text field has a tooltip: "To dictate, press the dictation shortcut of macOS. Set it in System Settings, Keyboard, Dictation." (Windows: "To dictate, press Windows key+H."; Linux has no built-in dictation, so no tooltip). The placeholder stays "Message Tutti", as designed.

## Memory views (for the orchestrator)

Phase 2 builds the Memory tab of the Channel panel and Settings > Memory in parallel. In this branch neither exists, so a memory note in the search results opens its channel (Tutti for global memory) and nothing more. To wire it after the merge: in `ConsortApp::open_hit` (`ui/switcher.rs`), the `SearchTarget::Memory { channel_id, path }` arm should open the Channel panel on its Memory tab with the note (`channel_id` set) or Settings > Memory with the note (global).

## Decisions

| # | Area | Decision | Why | Easy to change? |
|---|---|---|---|---|
| P4-1 | Proto | `Task.session_ended` tells clients when Retry is the way on for a task in Ready for review or Resolved. Stored in the task's JSON, no migration | Clients had no way to know that the session ended. A session entity in the feed would be much more | Yes |
| P4-2 | Proto | `message.search` takes `kinds`, filtered in the SQL query; memory and task hits carry `updated_at` | The scope filter must not lose memory hits to many messages; the rows show "Changed October 25" | Yes |
| P4-3 | Proto | `task.list_archived` with `offset` and `limit`, newest archive first, texts cut as in a snapshot | Snapshots leave archived tasks out, and the plan asks for a list and restore. Offset paging is enough for a list the user reads | Yes |
| P4-4 | Worker | The worker keeps each command's whole output and each file change's diff in `cache/items/<session>/`, at most 16 MiB per item, and uploads it on `item.detail` | The hub keeps only a tail and no diff (decision 90 left `item.detail` unbuilt). The item view needs the text | Yes |
| P4-5 | Harness | `FileChange.diff` carries the diff from the adapter to the worker, which takes it out before the hub. ACP diffs are made with `similar` from the old and the new text | The adapters have the texts; the hub must not store diffs (data-model) | Yes |
| P4-6 | Client | Reactions show at once and go back when the hub refuses | A click must feel immediate; the feed confirms | Yes |
| P4-7 | Desktop | The message menu has the quick emoji above its items, and it opens at the pointer, as every right-click menu | Rule 2: menus are planned; the design draws the menu by the bubble | Yes |
| P4-8 | Desktop | The emoji picker is our own grouped grid of about 150 emoji, with no search | Approved: a full picker of our own behind More | Yes |
| P4-9 | Desktop | The hub search runs 220 ms after the last key. Local Go to results show at once and match any word of the query | The design shows local results at once and the hub's under them; one word is enough for a channel or a task name | Yes |
| P4-10 | Desktop | Tab and Shift+Tab change the scope through key bindings in the `SearchPalette` context | GPUI matches key bindings before key listeners, and the base layer binds Tab to focus traversal | Yes |
| P4-11 | Desktop | A search result opens in place: a message scrolls its channel to it (older pages load first), a thread reply opens its thread, a task its thread, an archived task the archived list | Approved: results open in place | Yes |
| P4-12 | Desktop | New task starts from the channel's task model, isolation, approval mode and first repo; "At a time" offers the next 14 days and every half hour | The channel's defaults are what the hub would use; a short list fits a select | Yes |
| P4-13 | Desktop | The item view widens the panel to 560 px; output starts at the end in a uniform list; lines are not selectable, Copy output copies all | A large output must stay fast; one selectable text cannot hold 16 MiB in GPUI | Yes |
| P4-14 | Desktop | Channel settings sends only the changed fields with Save changes; a repo URL is made with `repo.create` at save | Approved: an explicit Save. Two clients that change different fields do not undo each other | Yes |
| P4-15 | Desktop | Rename, Instructions, Model and Worker tags in the channel menu open the channel settings | Their dialogs are gone; the menu keeps its planned order | Yes |
| P4-16 | Desktop | Delete group says the channels move "to the channels with no group", not "to the top of the sidebar" | The hub puts them at the end of the ungrouped list, which the sidebar draws after the groups | Yes |
| P4-17 | Desktop | A group row also has Rename (`group.update`) | The hub has it and a group needs a way to fix a name | Yes |
| P4-18 | Desktop | No voice button; the composer's tooltip names the system dictation shortcut | See "Voice input"; no `unsafe` | Yes, if objc2 gets a safe `startDictation` |
| P4-19 | Desktop | About shows build `dev` unless `CONSORT_BUILD` is set at build time, and its links go to the GitHub repo (`update::RELEASES_REPO`) | There is no build number yet, and no domain | Yes |
| P4-20 | Desktop | Archived tasks are a section at the end of the Resolved tab, with Restore | The plan asks for a view; Resolved is where done work lives | Yes |
| P4-21 | Desktop | A skill with a new version has Review changes, a diff of each changed file before Update | Smaller item: a diff view for a new skill version | Yes |

## Where it differs from the design, and why

1. **The message menu opens at the pointer**, not anchored to the bubble (P4-7).
2. **The thread panel's root time** is the time only ("9:12 AM"), not "Today 9:12 AM". `MessageView` has only the time label.
3. **The thread panel's reply box** has the text and the buttons in one row, as designed; the task reply box keeps the phase 1 layout (text above, buttons below).
4. **Search Go to** shows the channels and tasks that match any word. The mock has no task called "Build the Windows engine on bigboi", so that row of the design is not in the screenshot.
5. **Delete group copy** (P4-16).
6. **About** shows protocol 1 (the real one) and build `dev`.
7. **The item view lines** are not selectable (P4-13), and long lines are cut at the panel edge, as in the design.
8. **One worker** in New task opens a menu of the workers; the design shows only the choice.
9. **Add a repo** opens an inline URL field with a Known repos menu; the design shows only the row.
10. **Drawn without a design:** the emoji picker, Rename group, Delete task, the archived tasks section, the skill review dialog, and the dictation tooltip.

## Rows for DECISIONS.md

| Area | Decision | Reason |
|---|---|---|
| Proto | `Task.session_ended`; `message.search` `kinds` and hit `updated_at`; `task.list_archived`; `FileChange.diff` (worker only) | Retry rule in clients, the search scope, the archived view, the item view |
| Hub | `session_ended` is set when the current session of a task ends and cleared by a new session; no migration | Clients show Retry only when it works |
| Worker | `item.detail` uploads the kept output or diff (16 MiB cap per item, `cache/items/`) | Replaces decision 90's `not_found` |
| Client | Phase 4 views and commands in `convo.rs`, `reduce_convo.rs`, `hub/convo.rs`, `mock_convo.rs` | Shared with mobile later |
| Desktop | Threads on messages in the right panel with the accent ring; reactions with the quick emoji and a picker of our own | Approved designs `95W-0`, `9I9-0` |
| Desktop | Search: local Go to at once, hub results after 220 ms, scope by Tab, results open in place | Approved designs `A0N-0`, `AFG-0` |
| Desktop | New task dialog, item view at 560 px, channel settings with Save changes, simple New channel, group "+" and drag | Approved designs `ARL-0`, `CA5-0`, `CPJ-0`, `CYJ-0`, `EYQ-0`, `FOZ-0`, `FWA-0` |
| Desktop | Task thread in bubbles; About from the settings rail and the app menu | Approved designs `BDH-0`, `DWF-0` |
| Desktop | No voice button: the system dictation types through GPUI's text input; the tooltip names the shortcut | No safe AppKit call starts dictation |

## What is left

- **Memory search results** open only the channel until phase 2's views are merged (see "Memory views").
- **The item files** in `cache/items/` are never pruned. A prune with the blob cache prune is in the plan's smaller items.
- **The item view of an old session** names the worker of the task's current session.
- **A message thread whose root is very old** needs up to 20 older pages to find it from a search result; beyond that the timeline does not scroll to it.
- **Reactions by the assistant** come from the `react` hub tool, which existed; nothing changed there.
- **Light mode screenshots** were not taken.
- **A real dictation run** was not tried by hand (it would need the user's microphone); the input path is GPUI's and gpui-kit's.

## Checks

- `scripts/check.sh` (the whole workspace): see the last lines in the final report.
- Each new screen was opened with `--mock --screen` and compared with its artboard (the screenshots above).
