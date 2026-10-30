# Phase 2: routines and memory

These notes are for the merge into `docs/DECISIONS.md`. The plan is [FINISH-PLAN.md](../FINISH-PLAN.md), phase 2, with the approved decisions of 2026-10-29 and the design notes in [design-phases-1-2.md](design-phases-1-2.md). Written on 2026-10-30. One build did the hub, the proto, the client, the desktop app and the theme.

## What was built

### Proto (`crates/consort-proto`)

- **Routine runs.** `RoutineRunStatus` has four values: `ok`, `error`, `skipped` and `running`. `RoutineRun` has a run number (`id`), `channel_id`, `ended_at`, `status` and `manual`. The new fields have serde defaults, so a run from an older hub still reads.
- **A run is a feed entity.** `EntityKind::RoutineRun` and `EntityPayload::RoutineRun`, with the run number as the `entity_id` and the routine's channel as the feed channel.
- **Memory notes.** `MemoryNote` and `MemoryNoteSummary` have `suggested_by` (the task or channel of the memory suggestion that a version keeps) and `deleted_at` (set while the note is in Recently deleted). Both are left out of the JSON when empty. `MEMORY_DELETED_KEEP_DAYS` is 30.
- **Requests.** `memory.list` takes `deleted: true` for Recently deleted. New `memory.restore` and `memory.move`. `routine.update` takes `channel_id` to move a routine.
- **Hub tool.** `memory_write` takes `from_suggestion`. `SuggestionSource` has `FromStr` and a JSON schema.

### Hub (`crates/consort-hub`)

- **Migration `schema_v8.sql`.** `routine_runs` gets `channel_id`, `status`, `ended_at` and `manual`. Old runs become `ok` or `error` and end when they started. `memory_notes` gets `suggested_by` and `deleted_at`; `memory_note_versions` gets `suggested_by`.
- **The scheduler** (`scheduler.rs`):
  - A run is recorded as `running` when it starts. A `start_task` run is recorded before its task is made, so Run now and the schedule cannot start two at the same time.
  - A scheduled run while the last run of the same routine still runs is recorded as `skipped` ("The last run was still going."). The run that still runs stays the routine's last run. Run now then fails with `conflict` and records nothing.
  - A run that is 1 hour late or more (the hub was down) is recorded as `skipped` with the time when it was due, and the routine moves to its next run. A run less than 1 hour late runs once.
  - An interval routine keeps the rhythm of its save: the next run is the first multiple of the interval after now, counted from the run that was due (`next_interval_run`).
  - A run whose work is gone ends when the scheduler looks at it again: a task that was deleted or ended, a prompt that left the queue with no turn, or a `start_task` run whose task never came (5 minutes).
- **Run endings** (`store/routines.rs`): the coordinator turn that got the prompt ends the run (`end_turn_runs`): `ok` with the first timeline message of the session in that turn, or `error` when the turn failed, was stopped, or the session was lost after it acted. A task status change ends the run of the task (`end_task_run`, called from `ops::flow::transition`): Ready for review and Resolved are `ok`, Failed is `error` with the reason. Deleting the task ends the run as `error`. The routine's `last_run_status` follows the run while it is the last one.
- **Memory**:
  - A delete moves the note to Recently deleted (the row stays with `deleted_at`, the search index drops it, the feed gets the note with `deleted_at`). Reads, lists, indexes and sessions skip it.
  - `memory.restore` brings it back as it was. A write with `not_exists` to its path starts it again with the next version number.
  - `memory.move` renames a note with its versions. A live note at the new path is a `conflict`. A deleted note there is removed for good first.
  - The hourly maintenance removes notes deleted more than 30 days ago, with their versions, and sends a feed `delete`.
  - Deleting a channel also sends a `delete` for its notes in Recently deleted.
- **The memory saved line.** A coordinator's `memory_write` posts a `memory_saved` notice in its channel's timeline, in the same transaction as the write. The body has the note, the version, `created`, and the suggestion's ID and name. When the newest timeline message is the line for the same note from the same session, the hub updates it. A user's write posts no line.
- **Suggestion attribution.** `memory_write` keeps `from_suggestion` when the model sets it. Without it, when the coordinator's open turn holds memory suggestions from one source only, the write keeps that source.
- **Routine moves.** `routine.update` with `channel_id` moves a routine to an open channel. A session cannot move one.

### Client (`crates/consort-client`)

- **Cache** (`cache.rs`). The cache keeps every routine from the snapshots and the feed, the runs of the routines whose runs it loaded (`routine.runs`), and the memory notes of the loaded scopes, also the ones in Recently deleted. It reduces upserts and deletes, keeps the newer feed change over an older answer, keeps a note's text while its hash does not change, and drops the routines and the memory of a deleted channel. Before, `cache.rs` dropped these entries.
- **`routines.rs`.** The views (`RoutineView`, `RoutineRunView`), the routine dialog form (`RoutineForm`) and the schedule picker (`ScheduleForm`): presets, the cron each preset makes, the preset of a stored cron, the words ("Mondays and Thursdays at 8:00", "Every 2 hours, Monday to Friday"), the checks with an error for each field, the next runs, the preview, the calendar of the date field, and the words of the rows and the run history. The time zone starts as this computer's (`iana-time-zone`). The cron checks use `croner` as the hub does.
- **`memory.rs`.** `MemoryNoteView`, the tree by path with open and closed folders and "Just now", who saved a version, the changed lines of a version preview (a longest common subsequence of the lines), and the check of a note name.
- **Commands.** `SaveRoutine`, `SetRoutineEnabled`, `DeleteRoutine`, `RunRoutineNow`, `LoadRoutineRuns`, `LoadMemory`, `ReadMemoryNote`, `WriteMemoryNote`, `DeleteMemoryNote`, `RestoreMemoryNote`, `MoveMemoryNote`, `LoadMemoryHistory` (`hub/knowledge.rs`). A stale write comes back as the hub code `stale_write`.
- **Timeline.** A `memory_saved` notice has `NoticeView::memory` with the note and the name of the suggestion.
- **Mock** (`mock_routines.rs`). The routines of house-lights, Tutti and homelab, the run history of Sunset check, the memory of house-lights, global memory and sayso, a note in Recently deleted, the versions of `people/kevin.md`, the memory saved line, and every routine and memory command.

### Desktop (`crates/consort-desktop`)

- **Channel panel.** The Routines and Memory tabs are on, with their counts and their hints.
- **Routines tab** (`ui/routines.rs`). The summary, New routine, rows with three lines and the small switch, the hover actions Run now, Run history and the menu, and the empty state.
- **A routine in the panel.** Back, the name, the switch, the menu, the summary (schedule, action, prompt, next runs), Run now, Edit, and the run history of the last 30 days with succeeded, failed, skipped and running runs, durations, "Run now", errors as selectable text, and Message or Task links.
- **Routine dialog** (`ui/routines.rs`, `ui/schedule.rs`). Name, Channel, "Ask the assistant" or "Start a task" (title and brief), the prompt, the schedule with the six presets, the time, the time zone popover with a search, the Weekly days, the interval and its unit, the date field with its calendar, the cron, the next runs preview with the cron, the model and effort ("Same as house-lights", or the task's own model through the model picker), errors under their fields, Save off while a field has an error, and Delete routine in Edit.
- **Settings > Routines.** Every routine grouped by channel (the open channel first), the text filter, the status filter with counts (All, On, Off, Last run failed) and the channel filter.
- **Memory tab** (`ui/memory.rs`). The tree, New note, a note opened in place with its rendered Markdown, Edit, the menu, the editor with the stale-write message (Load the new version, Save my version), and the empty state.
- **Settings > Memory.** Global memory and each channel's memory, grouped, Recently deleted with Restore, the note page with Edit and the menu (Version history, Copy path, Rename, Delete), and the version history with the changed lines and Restore this version.
- **Dialogs.** New note, Rename note, Delete note, Delete routine.
- **The memory saved line** in the timeline, with the path as a link that opens the note in the Channel panel.
- **Theme.** New block `component.knowledge` in `design/structure.json`.
- **AGENTS.md** rule 2 lists the routine and memory menus.

### Screens and screenshots

`--mock --screen <name>`: `routines`, `routine-runs`, `routine-new`, `routine-edit`, `schedule-picker`, `schedule-interval`, `schedule-once`, `schedule-once-past`, `schedule-cron`, `settings-routines`, `memory`, `memory-conflict`, `settings-memory`, `memory-history`, `memory-deleted`, `routines-empty`, `memory-empty`.

`docs/screenshots/`: `phase2-routines-dark.png` (AFH), `phase2-routine-run-history-dark.png` (AZ7), `phase2-routine-dialog-new-dark.png` (BLH), `phase2-routine-dialog-edit-errors-dark.png` (BYU), `phase2-schedule-weekly-dark.png`, `phase2-schedule-interval-dark.png`, `phase2-schedule-once-dark.png`, `phase2-schedule-once-past-dark.png`, `phase2-schedule-cron-dark.png` (the states of CK3), `phase2-settings-routines-dark.png` (CRP), `phase2-channel-memory-saved-line-dark.png` (D8N), `phase2-memory-note-conflict-dark.png` (DNN), `phase2-settings-memory-dark.png` (E0W), `phase2-settings-memory-history-dark.png` (EAE), `phase2-settings-memory-recently-deleted-dark.png`, `phase2-routines-empty-dark.png`, `phase2-memory-empty-dark.png` (9FL).

## Where it differs from the design, and why

1. **Settings > Memory lists every scope.** E0W shows global memory only. The approved decision adds each channel's memory, so the tree has a group per scope with its icon and count, and Recently deleted at the end.
2. **Times of a cron routine show in its own time zone,** as its words do ("Every day at 17:30" with Europe/Berlin shows "Next run today at 17:30"). A Once or an interval routine shows times in this computer's time zone. The panel hint says "Times use each routine's time zone".
3. **The date field is a text field with a calendar.** It reads "Tue, Nov 3, 2026", "Nov 3, 2026" and "2026-11-03". The calendar glyph opens a month calendar under the field.
4. **The time zone select** opens a popover with a search over every IANA zone, this computer's zone first. The design draws only the closed select.
5. **The CK3 states are separate screenshots**, one per preset, in the real dialog. The artboard draws them side by side.
6. **The memory tree sorts by name.** D8N lists `roofline.md` before `porch-switches.md`. Name order is stable while notes change.
7. **The Run history label** says "last 30 days". The client asks for the newest 100 runs and shows the ones of the last 30 days.
8. **The run history of a skipped run** names the run time ("The last run was still going at 17:30."). The hub writes the reason without the time, because the hub does not know the time zone of a run that is not a cron run, and the client adds it.
9. **A memory saved line has its time separator** when it is the first item after a gap, as every timeline item does.
10. **Drawn without a design:** the empty states use 9FL; the Recently deleted page, the New note and Rename note dialogs, the delete confirmations, the calendar, and the time zone search.

## Decisions

| # | Area | Decision | Why | Easy to change? |
|---|---|---|---|---|
| P2-1 | Proto | A routine run is a feed entity (`routine_run`) with a run number, a status (`running`, `ok`, `error`, `skipped`), `ended_at` and `manual`. | The run history updates while a run runs and ends. The client cache can keep runs as it keeps other entities | Yes |
| P2-2 | Hub | A run is `running` until its work ends: the coordinator turn that got the prompt, or the task until Ready for review, Resolved or Failed. | "A run that would start while the last run still runs is skipped" needs a run that knows when it ends | Yes |
| P2-3 | Hub | A coordinator run links to the first timeline message that the session posted in the run's turn. | The design links each run to its message. The first answer is the one that the prompt asked for | Yes |
| P2-4 | Hub | A scheduled run while the last run still runs is recorded as `skipped` and does not change `last_run_status`. Run now then fails with `conflict` and records nothing. | The row keeps saying "Running now". A manual click gets an answer at once instead of a skipped row | Yes |
| P2-5 | Hub | A run 1 hour late or more is recorded as `skipped` with its due time. A run less late runs once. No catch-up. | Approved on 2026-10-29. The record shows the user what the hub missed | Yes |
| P2-6 | Hub | An interval routine counts from the run that was due, not from the time of the run. | "Interval runs count from save": a late tick or a short outage must not move the rhythm | Yes |
| P2-7 | Hub | A running run whose work is gone ends the next time the scheduler looks at the routine (a deleted or ended task, a prompt that left the queue, a task that never came after 5 minutes). | A lost run must not skip the routine for ever | Yes |
| P2-8 | Hub | A deleted memory note keeps its row with `deleted_at` for 30 days; the hourly maintenance purges it with its versions. Restore brings it back as it was, with the same version. | Approved on 2026-10-29. No version is lost while the note waits | Yes |
| P2-9 | Hub | `memory.move` renames a note with its versions. A deleted note at the new path is purged first; a live one is a `conflict`. | The menu has Rename, and a rename must keep the history | Yes |
| P2-10 | Hub | Each coordinator write posts a `memory_saved` notice in its channel. A second write of the same note right after updates that line. A user's write posts none. | "This is how the user sees memory suggestions." One line per note per turn keeps the timeline quiet | Yes |
| P2-11 | Hub | `memory_write` has `from_suggestion`. Without it, a write in a turn with suggestions from one source only keeps that source. | The model can say which suggestion it keeps, and the common case of one suggestion works without it | Yes |
| P2-12 | Proto | `routine.update` takes `channel_id`, for clients only. | The Edit dialog has a Channel select | Yes |
| P2-13 | Client | The schedule picker, its words, its checks and its preview live in `consort-client` (`routines.rs`), with `croner` and `chrono-tz` as the hub uses them. | The mobile client gets the same words and checks, and the preview matches the hub | Yes |
| P2-14 | Client | The time zone starts as this computer's, read with `iana-time-zone`. | The design notes ask for it | Yes |
| P2-15 | Desktop | Routine and memory times use a 24-hour clock ("17:30"). | The phase 2 designs use it, and cron times read that way | Yes |
| P2-16 | Desktop | The run history shows the last 30 days of the newest 100 runs. | The design says "last 30 days" | Yes |
| P2-17 | Desktop | A note that the assistant changed in the last 10 minutes shows "Just now" until the user opens it. | The design shows the dot on a note that the assistant just changed | Yes |
| P2-18 | Desktop | Version history opens in Settings > Memory, also from the Channel panel. | The preview needs the width of the settings page | Yes |
| P2-19 | Desktop | A new note asks for its name in a small dialog, then opens the editor. The note is made when the user saves. | An empty note in memory would be noise for the assistant | Yes |

## Tests

- **Proto:** run and note shapes, the new status values, `memory_write` with `from_suggestion`, entity IDs and channels, and the new requests in the round trips.
- **Hub:** 26 scheduler tests (running runs, skips while the last run still runs, late runs skipped and on time, interval rhythm, task runs that end with their task, a deleted task, a lost prompt), the routine handlers, the migration to version 8, the purge, `memory_saved` lines and suggestion attribution (`tools/chat.rs`), and the integration tests in `tests/routines_and_memory.rs` (Recently deleted and restore, a write over a deleted note, rename with versions, a routine move) and `tests/channels_and_routines.rs` (a run that ends with its message).
- **Client:** `routines::tests` (13), `memory::tests` (6), the cache tests for routines, runs and memory, the mock tests, and two fake hub tests: `routines_save_run_turn_off_and_delete_with_their_runs_in_the_view` and `memory_loads_writes_with_the_stale_check_and_comes_back_from_recently_deleted`.
- **Desktop:** `knowledge_tests.rs` (11): the Routines tab and the switch, a routine with its run history and Run now, the menus' names and order, Save off while a field has an error, the Weekly days and the calendar, the Settings > Routines filters, a stale save that keeps the text, Load the new version, the version history and Restore, Recently deleted, and the memory saved line link. `files_tests.rs` now clicks the Routines tab.
- `scripts/check.sh` with no arguments passed.

## What is left

- **Light mode screenshots** were not taken.
- **A real coordinator** was not run with a routine or a memory suggestion. The hub tests use the fake worker.
- **The model of "Start a task"** uses the model picker. A routine that keeps a task model of an account that is gone shows the stored model name.
- **Run now from a row** does not wait for the hub's answer before the next click; a second click gets the `conflict` message.
