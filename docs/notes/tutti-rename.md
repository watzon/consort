# Tutti rename and app follow-ups

These notes record the final pass after the redesign and step 3. The rows at the end are for `docs/DECISIONS.md`. Merge them there.

## What changed

### The rename (Part 1)

- The main channel is called Tutti in the code, the wire, the storage, the prompts and the living docs. ADR 0012 gives the reason.
- Rust names: `ChannelKind::Tutti`, `ToolAudience::TuttiCoordinator`, `MemoryActor::TuttiCoordinator`, `WakeEvent::TuttiMessage`, `Channel::is_tutti`, `TUTTI_ONLY`, `conversation::tutti_channel`, `mock::tutti_id`, `SelectionSource::Tutti`, and the fields and tests that used the old word.
- Mock ids changed too (`chn_tutti`, `msg_tutti_*`).
- `--screen everything` and `--screen everything-manages` still work. `canonical_screen` in `consort-desktop/src/main.rs` maps them to `tutti` and `tutti-manages`.

### Old values still read

| Type | New value | Old value that still reads |
|---|---|---|
| `ChannelKind::Tutti` | `tutti` | `everything` (serde alias) |
| `ToolAudience::TuttiCoordinator` | `tutti_coordinator` | `everything_coordinator` (serde alias) |
| `WakeEvent::TuttiMessage` | `tutti_message` | `everything_message` (serde alias) |

The prompt element `<everything_message>` is now `<tutti_message>`. The round-trip tests are in `consort-proto` (`conversation.rs`, `tools.rs`, `turn_input.rs`).

### Where the hub stores the old value

Only the hub stores it. The worker and the client store no channel kind. The client keeps its pairing, outbox and drafts. They hold channel IDs, not kinds. The worker spool holds no channel kind either.

| Place | Old content | Migration 6 does |
|---|---|---|
| `channels.kind` | `everything` | sets `tutti` |
| `channels.data` (JSON) | `"kind":"everything"`, `"name":"Everything"` | sets `tutti` and `Tutti` |
| index `channels_one_everything` | `WHERE kind = 'everything'` | drops it and makes `channels_one_tutti` |
| `feed.payload` of a `channel` entry | the same JSON | the same rewrite |
| `wake_events.event` | `"type":"everything_message"` | sets `tutti_message` |
| `coordinators.turn_events` | an array of wake events | text replace of the `type` value |

`schema_v6.sql` is the migration. The test `store::migrations::tests::version_6_rewrites_everything_rows_to_tutti_and_they_read_as_tutti` opens a database at version 5, fills it with `everything` rows, migrates it, and checks that the rows read as Tutti. A message body that contains the text of an old `type` value stays as it is.

`schema_v1.sql` is not edited. A finished migration never changes.

### Prompts

- `coordinator.md` says once that Tutti is the main channel where the user talks to the assistant about everything.
- The test `prompts::tests::the_prompts_and_the_tool_texts_name_tutti_and_never_everything` checks the Tutti prompt, the channel prompt, the task prompt and every tool text of every audience. None may say "Everything".

### What is left in `crates/`

`grep -rn -i "everything" crates/ --include=*.rs` shows only these:

- The three serde aliases and their round-trip tests (`conversation.rs`, `tools.rs`, `turn_input.rs`).
- `canonical_screen` and its test (`main.rs`).
- The migration test (`store/migrations.rs`), and the comment on the old value in `schema_v6.sql`.
- The prompt test that checks the old word is absent (`prompts.rs`).
- Plain English: "stores everything", "manages everything", "leaves everything as it is", "Everything that ..." at the start of a sentence, and similar.

## App follow-ups (Part 2)

### Add skill and the fake hub

- The fake hub in `consort-client/tests/fake_hub.rs` now answers `skill_directory.search`, `.sources`, `.add_source`, `.remove_source`, `skill.preview`, `skill.install` and `connection.calls`. A flag, `skills_unsupported`, makes it answer `unsupported` for the skill requests.
- Tests cover: search and load more, preview of an entry and of a git URL, install, the install card closing on install, sources, connection calls with their size and day, the generic "This hub cannot ... yet. Update the hub, then try again." text for a hub that answers `unsupported`, and no such text for other errors.
- Uploads are not tested against the fake hub. The client refuses them before it sends a request.

### Skill sources

- Settings > Skills has a Sources section under the skill rows. It lists `skill_directory.sources`. It loads when the page opens.
- Add source opens a dialog with one URL field (a git repo or a `marketplace.json` URL). It calls `skill_directory.add_source`. An empty field says: "Paste the URL of a git repo or a marketplace.json file."
- A source that the user added has a menu (right-click, or the "..." button) with Remove. It calls `skill_directory.remove_source`. Built-in sources have no menu.
- skills.sh is a link, with the hint to paste `owner/repo` into the Git URL field of Add skill.
- The mock has one added source (`acme/skills`) so the screenshot shows the menu button. See `docs/screenshots/skills-sources-dark.png`.

### Connection activity

- The connection menu is now Reconnect, Activity, Copy handle, Disconnect.
- Activity opens a dialog with the columns Tool, Decision, Outcome, Time and Size. The first page loads when the dialog opens. Show more loads the next page with `before`.
- A failed load says what happened and has Try again.
- The client view `ConnectorCallView` has two new fields: `day_label` and `size_label` (`format::size_label`).

### Routine icon

- The Scheduled status uses the `routine` icon (the metronome) in `status.scheduled`. So does the Routines item of Settings. `clock` stays for plain times.

### Top fade

- The tokens `layout.fade_top_solid` (92) and `layout.fade_top_blend` (24) replace `layout.fade_top` (96).
- The solid part reaches below the mark and the pill. A theme test checks that it is at least `mark_top + mark + space.xxs + pill + space.xs`, and not far above it.
- The zone is the same with or without the pill. The first row and the "unloaded" notice start below the solid part and the blend.
- Screenshot: `docs/screenshots/top-fade-dark.png`, from `--mock --screen mark-working --dark`. It shows the mark and the pill ("Running a command in sayso") on clean canvas. The bubbles go behind the solid part. The first line under the pill is only a faint trace, in the blend.

## Rows for DECISIONS.md

| # | Area | Decision | Why | Easy to change? |
|---|---|---|---|---|
| T1 | Rename | The main channel is Tutti in the code, the wire and the storage. The old values stay readable by serde aliases (`everything`, `everything_coordinator`, `everything_message`). | Consort should not copy the Duet name, and one channel should not have two names | Yes |
| T2 | Rename | Hub migration 6 rewrites the stored values, and the channel name "Everything" becomes "Tutti". | Old data then shows the right name without help from the alias | Yes |
| T3 | Rename | A new hub writes `tutti`, so an older client cannot read it. | Hub and clients ship together before the first release. Adding a protocol version for this has no use yet | Yes |
| T4 | Rename | The wake event element in prompts is `<tutti_message>`. Stored events with the old name still read, and migration 6 renames them. | The model should never see the old name | Yes |
| T5 | App | `--screen everything` and `--screen everything-manages` stay as aliases. | Old review scripts keep working | Yes |
| T6 | App | A user-added skill source has Remove in its row menu, with no confirm step. | Installed skills stay when a source goes, and the user can add the source again | Yes |
| T7 | App | skills.sh is a link under Sources and not a source. | Its API is closed to the hub (see step 3 notes) | Yes |
| T8 | App | The Activity dialog shows the day and the time, and the size of the result. It does not show the arguments or the result. | The hub keeps only a hash and a size | Yes |
| T9 | App | The Scheduled status and the Routines item use `routine`. `clock` stays for plain times. | The approved design | Yes |
| T10 | App | `layout.fade_top` is split into `fade_top_solid` (92) and `fade_top_blend` (24). The solid part is fixed and does not depend on whether the pill shows. | No bubble should show under the status line, and the edge should not jump | Yes |
