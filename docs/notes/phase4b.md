# Phase 4b: the assistant on its own

These notes are for the merge into `docs/DECISIONS.md`. The plan is [PROACTIVITY-PLAN.md](../PROACTIVITY-PLAN.md) with [ADR 0013](../adr/0013-the-assistant-is-proactive-within-limits.md). The designs are in [design-proactivity.md](design-proactivity.md) (`FYE-0`, `G2C-0`, `G8V-0`, `GHK-0`, and its "Answers", which are final) and in [design-phases-3-5.md](design-phases-3-5.md) (`F6N-0`, `FDX-0`, `FJC-0`). Written on 2026-10-30. One build did the proto, the hub, the client, the desktop app, the theme and the docs.

## What was built

### Proto (`crates/consort-proto`)

- **New file `model/proactivity.rs`.** `ProactivityLevel` (off, quiet, normal, eager; ordered, with `default_for` and `allows`), `ProactiveKind` (check_in, brief, follow_up, stale_task, research, channel_created, feedback; with `min_level` and `budget_can_skip`), `ProactiveMark` (`kind`, `notify`, `reason`, `offer`, `rating`), `ProactiveRating`, `Suggestion` with `SuggestionAction` (send, type) and `validate_suggestions` (at most 4, at most 40 characters, no copy), `ProactiveTasks` (drafts, channel_default), `QuietHours` (with `contains_minute`, which crosses midnight), `ProactivitySettings`, `SnoozeChoice`, `ProactiveOutcome` (with the words of the log: "Skipped: over the turn limit", "Skipped: a turn was running"), `ProactiveWake` (the log entry), and the limits as constants.
- **Fields with serde defaults**, so old rows and old peers read: `Channel.proactivity` and `Channel.proactive_tasks`; `Routine.kind`, `Routine.origin` and `Routine.created_at`; `Message.suggestions` and `Message.proactive`.
- **Wake events** (`turn_input.rs`): `check_in`, `brief`, `follow_up`, `stale_task`, `research`, `proactive_feedback`, `channel_created`, each with a hidden `log` entry number that is never rendered. `WakeEvent::proactive_kind` and `log_id`.
- **Hub tools** (`tools/proactivity.rs` and `tools.rs`): `follow_up_later`, `cancel_follow_up`, `set_proactivity`; `post_message.suggestions` and `post_message.notify`; `reply.suggestions`; `no_reply_needed.finding`; `ask_decision.offer` (options may be left out for an offer).
- **Client API** (`client_api/proactivity.rs`): `proactivity.get`, `proactivity.set` (with `time_zone_if_unset`), `proactivity.snooze`, `proactivity.log`, `proactivity.cancel_follow_ups`, `message.rate`. Feed entities `proactivity_settings` and `proactive_wake`. `Snapshot.proactivity` for the `all` scope. `ChannelUpdate.proactivity` (a patch: `null` is the default) and `proactive_tasks`.

### Hub (`crates/consort-hub`)

- **Migration `schema_v9.sql`**: `proactive_wakes` (the log), `proactive_channels` (last check-in time, silent streak, base interval), `task_status_since` (status entry times and nudges), `follow_ups` (key and set time). The settings are a JSON record in `meta`.
- **`store/proactivity.rs`**: settings, the log (insert, save, page, turns today, findings, research offers today, purge), the channel state, the status entries and the follow-up keys.
- **`proactivity/`**:
  - `gate.rs`: the pure gate (level, Pause all, snooze, quiet hours, turn budget), the user's local time with chrono-tz (quiet hours that cross midnight, daylight saving gaps), the end of quiet hours, snooze ends, the start of the user's day, the jitter of a routine (0 to 5 minutes, stable per routine), and the time words of the log ("Quiet hours until 7:00 AM.", "Snoozed until 1:45 PM.").
  - `summary.rs`: the change summary of a check-in (tasks that changed status, approvals and decisions that wait, failed routine runs, workers that went offline or came back in Tutti, new channel files, messages from Tutti), the 2 KiB cut of the note, and `watch: outside`.
  - `routines.rs`: the check-in at Normal (3 hours in Tutti, 6 elsewhere), the research pass at Eager (06:30 daily), the morning brief in Tutti once (weekdays 07:30), Tutti's check-in note with its three starting lines (`watch: outside` when a connection exists), the defaults once per hub at start, the time zone change that moves the hub's cron routines, the checks on a change of a check-in (prompt fixed, at least 30 minutes, a new interval ends back-off), and `begin_proactive_run`, which the scheduler calls for every routine that is not `custom`.
  - `stale.rs`: the stale-task timer every 30 seconds (24 hours in Needs input, 72 hours in Ready for review, once per status entry; quiet hours hold it with no row; snooze and Pause all log one row and hold it; the turn limit skips it for good).
  - `turn.rs`: which turn is proactive, the tool rules (no tool that widens access in a proactive turn; read only in a research turn), the marks, the outcome of each log entry at the end of a turn, back-off and its reset, the deletion of a follow-up after its turn.
  - `tools.rs`: the three tools, the notification rule of a post (outside quiet hours, snooze and Pause all; once per turn; under the daily limit), offers (Eager only, three fixed options, at most 3 a day from research), draft mode for tasks, `memory_write` under `research/` in research, findings, and the task of a turn in its log entries.
  - `mod.rs`: the 30 second loop (stale tasks, and the hourly purge of the log after 30 days), the client requests, the guided start of a new channel, and `message.rate`.
- **Small hooks in shared files**: the scheduler hands proactive routines to the gate and takes a clock (`tick_at`, `run_routine_at`); `coordinator_turn_ended` drops the fallback post of a proactive turn and finishes its log entries; `run_tool` checks the turn rules; `post_message`, `reply`, `ask_decision`, `start_task`, `no_reply_needed`, `memory_write` and `run_connector_tool` follow them; `channel.create` queues the guided start; `channel.update` takes the level and makes or pauses routines; `queue_wake` of a user message resets back-off; `transition` notes the status entry; channel and task deletes clean up; the snapshot holds the settings; the prompt fingerprint includes the level.
- **Coordinator prompt**: the section "Working on your own" in `prompts/coordinator.md`, the proactive events in the events list, and a "Working on your own here" part with the level of the channel.
- **`HubHandle::run_routines_at` and `nudge_stale_tasks_at`** let integration tests run the schedule at a chosen time.

### Client (`crates/consort-client`)

- **`proactivity.rs`**: the cache part (settings, log, load state), `ProactivityView` for Settings > Proactivity (levels and their meanings, follow-ups, today's counts, the brief row, the log grouped by day), the marks and the quick replies on the timelines (chips only under the newest assistant message, gone at any reply), the line "You made the channel X." at the start of a standard channel whose history is loaded, `is_writing` (the writing bubble), `quiet_now`, the brief parser, `check_in_line`, the level words and the helper text.
- **`notify.rs`**: the switch `attention` ("When the assistant needs your attention", on by default). A proactive message notifies only with its `notify` mark and that switch, never through the switch for messages. Nothing notifies in quiet hours or while snoozed (`AttentionView.quiet`).
- **Commands**: `LoadProactivityLog`, `SetProactivity`, `SnoozeProactivity`, `CancelAllFollowUps`, `SetChannelProactivity`, `RateMessage` (`hub/proactivity.rs`). At the first load the client sends this computer's time zone with `time_zone_if_unset`, and loads the newest 200 log entries, before it subscribes.
- **A brief is one bubble** (`chat.rs`): its groups and numbered items need it.
- **Mock** (`mock_proactivity.rs`): the content of every artboard, and the commands on the mock state.

### Desktop (`crates/consort-desktop`)

- **New files**: `proactivity.rs` (state, actions, select picks, screens), `ui/proactivity.rs` (the page, the level select, the marks, the feedback actions, the chips, the writing bubble, the offer card, the brief, the Follow-ups section), `proactivity_tests.rs`, and the dab glyphs `assets/glyphs/dab-{gold,verdigris,lapis}-{0,1,2}.svg` (also in `design/glyphs/`).
- **Short additions to shared files**: the settings section and rail item; the select pick hook; the screen hook; the field in `ConsortApp`; the log load when the page opens; the writing bubble under the last row with its list key; the caption, the feedback actions, the chips and the hover region of a message in `render_bubble`; the brief in a bubble; the offer card arm in `render_card_row`; no time after the "You made the channel" line; the check-in row and the Follow-ups section in the Routines tab; no Edit in a check-in menu; the switch in Settings > Notifications.

### Theme

`component.proactivity` in `design/structure.json`, with `Proactivity` in `structure.rs` and `tokens.rs`, and the test `proactivity_tokens_keep_the_values_of_the_designs`.

### Screens and screenshots

`--mock --screen`: `proactivity` (FYE-0), `proactive-messages` (G2C-0), `routines-follow-ups` (G8V-0), `channel-greeting` (F6N-0), `channel-writing` (FDX-0), `channel-question` (FJC-0). The names `new-channel` and the like were taken by the New channel dialog screen.

`docs/screenshots/`: `phase4b-proactivity-dark.png` and `phase4b-proactivity-log-dark.png` (FYE-0, top and bottom of the page), `phase4b-proactive-messages-dark.png` and `phase4b-offer-and-brief-dark.png` (G2C-0, scrolled to the end and to the offer), `phase4b-routines-follow-ups-dark.png` (G8V-0), `phase4b-channel-greeting-dark.png` (F6N-0), `phase4b-channel-writing-dark.png` (FDX-0), `phase4b-channel-question-dark.png` (FJC-0).

## How to add the level select to the channel settings dialog (GHK-0)

The dialog comes with phase 4 and is not in this branch. After the merge, in the Assistant section of the dialog, between "Model and effort" and "Channel memory":

1. Add a field label "Proactivity".
2. Add the select: `crate::ui::proactivity::level_select(crate::proactivity::CHANNEL_SETTINGS_LEVEL_SELECT, &channel_id, form_level, kind, <the field width>, self, t, cx)`. `form_level` is an `Option<ProactivityLevel>` in the dialog's form, which starts as the channel's own level: `state.proactivity.channels` has `level` for each channel (`None` is "Default for this channel"). `kind` is `consort_client::proactivity::LevelKind::Tutti` or `Standard`. The select shows the level, and its menu has the four levels and "Default for this channel (Quiet)". To show "Default for this channel" at the right of the box as in GHK-0, put a `text("Default for this channel", detail(t), c.text_tertiary)` beside the label when `form_level` is `None`.
3. Under it: `crate::ui::proactivity::level_helper_text(form_level, kind, t)`. It says what the level does and "Only you can raise the level."
4. In `pick_select`, before `pick_proactivity_select`, take the pick of `CHANNEL_SETTINGS_LEVEL_SELECT` into the form: `crate::proactivity::parse_level_option(option)` gives `(channel_id, level)`. Do not send it at once: the dialog has Save changes.
5. On Save changes, send it with the other fields: `ChannelUpdate.proactivity = Some(form_level)` (the client command `Command::SetChannelProactivity` does the same alone). The hub makes or pauses the check-in and the research pass.
6. Remove the two `#[allow(dead_code)]` lines (on `CHANNEL_SETTINGS_LEVEL_SELECT` and on `level_helper_text`).

The dialog grows by 41 px, as the design notes say. `ChannelUpdate.proactive_tasks` ("Tasks the assistant starts on its own": Draft mode or Use the channel default) has no row in GHK-0; a select with those two words fits under the helper text if the user wants it in the dialog.

## Where it differs from the design or the plan, and why

1. **The level select of a channel row** has the four levels and "Default for this channel (Quiet)" as a fifth choice, so a channel can go back to its default. The design draws only the closed select.
2. **The time zone select** offers this computer's zone, the zone that is set and eleven common zones, not every IANA zone. The phase 2 zone popover with a search lives in the routine dialog's state; a second copy for one field was not worth it. A zone outside the list can still be set by a client with `proactivity.set`.
3. **The log** loads the newest 200 entries at the first load and when the page opens, and the feed keeps it current. Older entries are on the hub (`has_more`). The filters work on the entries that the client has.
4. **"Open message"** opens the channel, and the task thread when the message is a reply in a task thread (answer 6). It does not scroll to the message: the timeline has no jump to a message yet.
5. **The writing bubble** shows while the user's message is the newest one and reached the hub, or while a new channel with no message has a working assistant. A check-in shows no bubble, because it may end in silence.
6. **"You made the channel X."** is a line that the client draws from `channel.created_at` at the start of a standard channel whose whole history is loaded. The hub posts no notice, so no channel gets a message that the user did not see come.
7. **The dashed border** of "I will type it" draws, but at 1 px on a pill it reads almost solid in the screenshot. GPUI draws the dashes; no token sets their length.
8. **Times in the log** use the 12-hour clock, as the design does. The Routines tab keeps the 24-hour clock of phase 2 ("Stayed silent at 7:25.").
9. **The brief** is drawn from its Markdown: lines that name a group ("Needs you", "Ready for review", "Done since yesterday", also in bold or as a heading) start a group, and `N. [Name](task:...) rest` lines are items. A brief in another shape shows as plain Markdown.
10. **A wake that the level stops** gets no row (answer 3). A routine run of it is still recorded as `skipped`, with the reason, so its run history is honest.
11. **The research pass** is a routine in any channel at Eager, not only in Tutti, because the level is per channel.
12. **Approval cards that never expire.** The hub never expires them. A harness that times out a request on its own side can still end one; that is outside the hub.

## Decisions

| # | Area | Decision | Why | Easy to change? |
|---|---|---|---|---|
| P4b-1 | Proto | The proactive mark lives on the message (`Message.proactive`), with `offer` and `rating` in it. Decisions got no new field. | One record per message; an offer is a decision card with a mark, so `Decision` stays as it is | Yes |
| P4b-2 | Proto | Proactive wake events carry the number of their log entry, which the turn text never shows. | The hub finishes the right log entries at the end of the turn, also after a requeue | Yes |
| P4b-3 | Proto | The brief has its own wake event `brief`. | A turn must know that it is proactive; the `routine` event stays for custom routines, as the plan says | Yes |
| P4b-4 | Hub | The gate is pure code that takes `now`; the scheduler and the stale timer pass the clock. | Tests run any day and time, and the hub decides with no model | Yes |
| P4b-5 | Hub | A check-in compares against hub state since the last check-in of its channel; the time moves on at each check-in that ran or found nothing, not at a skip for quiet hours, snooze or Pause all. | Changes in the night show in the first morning check-in | Yes |
| P4b-6 | Hub | Status entry times live in `task_status_since`, written by `transition`. A task from before has its last change as its entry time. | `Task` has no status time, and a field would touch every client | Yes |
| P4b-7 | Hub | Busy: a check-in whose last one still waits for its turn is `skipped_busy`. | That is the case of "waited more than one interval" | Yes |
| P4b-8 | Hub | The notification limit counts notifying messages of today in the user's time zone; one notification per proactive turn. | "The brief may notify once"; one rule for every kind | Yes |
| P4b-9 | Hub | In a proactive turn the hub refuses the tools that widen access, add or remove connectors and skills, change channels, models or routines. `set_proactivity`, memory, tasks (in draft mode), decisions and follow-ups stay. | "A proactive turn cannot widen access, add a connector, or turn on scripts"; settings changes need the user | Yes |
| P4b-10 | Hub | Offers need the level Eager and a proactive turn. | The level table puts offers at Eager | Yes |
| P4b-11 | Hub | A follow-up keeps its key and set time in `follow_ups`; `Routine.created_at` is new for every routine. | The Follow-ups row says "set this morning" | Yes |
| P4b-12 | Hub | The defaults (check-in, brief, check-in note) come once per hub at start (`proactivity.defaults_v1`); the brief once ever (`proactivity.brief_made`). A deleted check-in comes back only on a level change. | The user's delete stays | Yes |
| P4b-13 | Hub | The hub writes Tutti's check-in note as the user (`MemoryAuthor::User`). | `MemoryAuthor` has no hub value; the note is the user's to edit | Yes |
| P4b-14 | Hub | The prompt fingerprint includes the level, and the system prompt names the level. | A level change reaches the coordinator in its next turn | Yes |
| P4b-15 | Hub | Manual Run now of a proactive routine passes the gate. | The user asked for it | Yes |
| P4b-16 | Hub | A new channel's guided start is gated like other wakes: no greeting at Off, a logged skip while paused. | Answer 4: Pause all stops everything that starts on its own | Yes |
| P4b-17 | Client | The client sends this computer's time zone with `time_zone_if_unset` at the first load, and loads the newest 200 log entries, before it subscribes. | A handshake call drops feed events that come while it waits | Yes |
| P4b-18 | Client | Nothing notifies in quiet hours or while snoozed, for every kind, not only proactive messages. | CONTEXT: quiet hours are the hours with no notifications | Yes |
| P4b-19 | Client | The writing bubble is computed from the view (`is_writing`), not stored. | No new state to keep in step | Yes |
| P4b-20 | Desktop | The level selects use 32 fixed select IDs by row; the option names the channel. | A select needs a static ID for its menu anchor; the option keeps a pick on its channel | Yes |
| P4b-21 | Desktop | The quiet hours are two time boxes with times every 30 minutes, and a short time zone list. | The design draws boxes; see "Where it differs" | Yes |
| P4b-22 | Desktop | The dabs of the writing bubble are SVG glyphs in three versions each, on the shared clock. | GPUI has no SVG noise; the icons shimmer the same way | Yes |

## Tests

- **Proto**: the levels and kinds, quiet hours across midnight, suggestions, settings and wakes, marks, old routines, the wake events and their hidden log number, the tool schemas, the request round trips, the feed entities and the snapshot field, the channel update patch.
- **Hub unit tests** (`proactivity/tests.rs`, 27, and `gate.rs`, `summary.rs`, `turn.rs`): the check-in made and paused with the level; the defaults once and the brief not again after a delete; the time zone move; the check-in prompt and interval rules; the no-change skip with no wake event; a note with `watch: outside`; the change summary content in a channel and in Tutti; quiet hours, snooze, Pause all and the turn limit before the model, and the budget back the next day; skipped as busy; back-off to 12 hours and the reset by a user message; the brief and follow-ups in quiet hours and snooze, and Pause all for the brief; follow-up limits, replace by key, cancel, the reason on the post, the delete after the turn, Cancel all, and none at Off; a follow-up skipped while paused; stale tasks once per status entry, 24 and 72 hours, held by quiet hours and snooze, none at Off; the research pass read only with offers and their limit; offers need Eager; draft mode and `channel_default`; no widening tools; the notification limit, once per turn, and no mark in a normal turn; nothing notifies while snoozed; suggestions on the message and their limits; `set_proactivity` quieter only; Useful and Too noisy; the guided start and its pause; a failed turn; the log page, filters and purge. Gate unit tests: quiet hours in America/Chicago across daylight saving time, the gap in spring, snooze ends, the order of the checks, jitter.
- **Hub integration tests** (`tests/proactivity.rs`, 6): a check-in with nothing new sends nothing to the worker; a proactive turn drops its text, marks its post and notifies; the guided start with quick replies, then a normal turn with a `type` chip; Too noisy wakes the coordinator; the settings, snooze, snapshot, log and Cancel all over the client API; a stale task wakes its coordinator once.
- **Prompt test**: the "Working on your own" section has every rule of the plan.
- **Client** (`proactivity/tests.rs`, 11, and `mock_proactivity.rs`, 2): chips only under the newest assistant message and gone at a reply, the made line, the marks and one caption per group, the notification rule for `notify` messages and not for other proactive messages, quiet hours and snooze, the writing bubble, the cache, the page numbers and the log, level words, the brief parser, the quiet hours request, follow-up words. The fake hub tests expect the new `proactivity.log` call.
- **Desktop** (`proactivity_tests.rs`, 6, and `proactivity.rs`, 2): the page (Pause all, snooze, a level, the filters, Cancel all follow-ups), a send chip and the chips gone at the reply, the type chip that focuses the composer and sends nothing, the writing bubble, the marks with Useful and Too noisy and the offer card, the check-in row with Open note and no Edit, and Cancel of one follow-up.
- `scripts/check.sh` with no arguments passed: 1724 tests.

## What is left

- **A real model** was not run with a proactive turn. The hub tests use the fake worker; the prompt is checked by its test.
- **Light mode screenshots** were not taken.
- **Jump to a message** for "Open message" (see above).
- **The channel settings dialog row** waits for the phase 4 merge (see the steps above).

## Rows for DECISIONS.md

| Area | Decision | Reason |
|---|---|---|
| Proto | The proactive mark is on the message (`Message.proactive`: kind, notify, reason, offer, rating); decisions get no field | One record per message; an offer is a marked decision card |
| Proto | Proactive wake events carry their log entry number, never rendered | The hub finishes the right log entries at the turn's end, also after a requeue |
| Proto | The brief has its own wake event `brief` | A turn must know that it is proactive; `routine` stays for custom routines |
| Hub | The gate is pure code that takes `now`; the scheduler and stale timer pass the clock | Tests at any time; the hub decides before a model runs |
| Hub | A check-in's change summary starts at the last check-in that ran or found nothing; quiet hours, snooze and pause do not move it | Night changes show in the first morning check-in |
| Hub | Status entry times live in `task_status_since`, written by `transition` | `Task` has no status time |
| Hub | A check-in whose last one still waits for its turn is `skipped_busy` | "Waited more than one interval" |
| Hub | The notification limit counts notifying messages today in the user's zone; one per proactive turn | "The brief may notify once"; one rule for all kinds |
| Hub | A proactive turn cannot use tools that widen access or change channels, models, connectors, skills or routines | Settings changes need the user |
| Hub | Offers need the level Eager and a proactive turn; three fixed options | The level table |
| Hub | Follow-ups keep key and set time in `follow_ups`; `Routine.created_at` is new | "Set this morning" in the Follow-ups row |
| Hub | Defaults come once per hub; the brief once ever; a deleted check-in returns only on a level change | The user's delete stays |
| Hub | Tutti's check-in note is written as the user | `MemoryAuthor` has no hub value |
| Hub | The prompt fingerprint and the system prompt include the level | A level change reaches the coordinator |
| Hub | Run now of a proactive routine passes the gate | The user asked for it |
| Hub | The guided start is gated like other wakes (Off: none; paused: logged skip) | Pause all stops everything that starts on its own |
| Client | The client sets the hub's time zone with `time_zone_if_unset` and loads 200 log entries before it subscribes | Handshake calls drop feed events |
| Client | Nothing notifies in quiet hours or while snoozed, for every kind | CONTEXT defines quiet hours as hours with no notifications |
| Client | A proactive message notifies only with `notify` and the "needs your attention" switch, never as a channel message | The plan's tier 2 never notifies |
| Client | "You made the channel X." is drawn from `created_at`, not posted by the hub | No message the user did not see come |
| Desktop | Level selects use fixed IDs by row; the option names the channel | Static menu anchors; picks stay on their channel |
| Desktop | Quiet hours use 30-minute time boxes and a short zone list | The design's boxes; one zone picker with search lives in the routine dialog |
| Desktop | The writing bubble's dabs are SVG glyphs in three versions on the shared clock | GPUI has no SVG noise |
