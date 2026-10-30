# Phase 5, part b: the saved client cache and the offline start

These notes are for the merge into `docs/DECISIONS.md`. The plan is [FINISH-PLAN.md](../FINISH-PLAN.md), phase 5, item 4, and the decision "The hub is offline: messages wait in a queue; approvals and decisions stay blocked until the hub is back". The design is Paper `ERQ-0` in [design-phases-3-5.md](design-phases-3-5.md). Written on 2026-10-30.

## What was built

### The saved cache (`crates/consort-client/src/persist.rs`)

- One SQLite file, `cache.sqlite3`, in the client's folder next to `client.json`. Two tables: `entity(kind, id, json)` and `meta(key, value)`. `PRAGMA user_version` is the schema version (1). The format is in [data-model.md](../data-model.md#saved-client-cache).
- The API is narrow. `open_or_recover(path, owner)` opens the file at start and never fails. `rows(cache, fresh_at, owner)` makes the rows of a save from the `Cache`. `CacheFile::write(rows)` saves them in one transaction and writes only the rows that changed (it keeps a hash of each row that the file holds) and removes the rows that are gone. `delete(path)` removes the file and its companions.
- The cache is saved only when its first load is whole (`Cache::loaded`), so the file never holds a half load. A half load leaves the last whole file as it was.
- `hub/save.rs` (a task, `run_save`) saves 0.5 s after a change. The wait does not restart on a new change, so a feed that never stops is still saved at this pace. The save runs on a blocking thread. `Shared::publish` and `publish_now` ask for it. A quit loses at most that half second, and the next connect replays it from the feed.
- At start, `Shared::new` reads the file before the first publish. The app has its data before it connects. The saved cache has `loaded = true` and its `seq`, so the existing `initial_sync` sends `sync.subscribe` with `after_seq`. A `resync_required` answer loads a snapshot, as before. The hub did not change: it already answers `resync_required` for a position below its floor or above its head.
- What the file holds: groups, channels, repos, workers, messages, channel files, routines and their runs, memory notes (with the flag for loaded text), tasks, approvals, decisions, read markers, session item summaries, session activity, connections, skills, the paired clients, the preferences, and the sets and flags that say what is loaded. What it leaves out is written in the doc comment of `rows`.
- A guard against a silent loss: the test `assert_same` destructures `Cache` with no `..`. A new field of `Cache` stops the test from compiling, so its author must decide to save it or to leave it out.
- A file that cannot be used is moved to `cache.sqlite3.broken` (and its journal files are removed, because a left journal would damage the new file). The cases: not a database, a failed `quick_check`, a schema version other than 1, and a row that does not parse. A log line says why, and the client starts with no saved data. Data of another client ID or hub URL is removed from the file.
- Unpair: new `Command::Unpair` and `Client::unpair()` remove the identity, the outbox and the cache file, move the messages that wait to the failed sends, end the live link and set `NotPaired`. `Command::Pair` (a new hub) and a `Refused` link also remove the file. `Store::clear` removes it too.

### The offline state (`consort-client/src/offline.rs`, `consort-desktop`)

- `ViewState.saved_data` (`SavedData { from_ms, from_label }`) is set while the link is `Offline`, the cache holds data and the time of the data is known. `Shared.fresh_at` is the time when the data was last current: it moves on each publish while the link is `Online`, and when a link ends. It is saved in the file, so the next start knows it. `mark_saved` makes every worker status unknown (`online = false`, "Status unknown") and clears the activity.
- `ViewState::answers_blocked()` gives the reason ("Waiting for the hub. You can answer when it is back.") for every connection state but `Online` and `Demo`.
- Pill (`ui/timeline.rs`, `render_top_overlay`): "Cannot reach the hub. You see saved data from 9:41 AM." with Try again, under the mark. The mark rests at `opacity.offline_mark`. The line above the composer ("Retrying in N s") stays out while the pill shows. A day name replaces the time alone when the data is older than today.
- Sidebar: the Workers group label says "Status unknown", and the dots are hollow.
- Approvals and decisions (`ui/hub_blocked.rs`): the buttons use `opacity.offline_dim`, and a tooltip says why. A click shows the reason in a notice, and `answer_approval`, `answer_decision`, `continue_connect_card`, `change_connect_card` and `sign_in_from_card` stop with the same notice, so no other path sends an answer.
- A queued message: its bubble uses `opacity.queued` and says "Sends when the hub is back" with the working glyph. The composer line says "Messages wait here until the hub is back" in place of "Runs on <worker>". The outbox already sent the queue when the link came back, so nothing changed there.
- New tokens: `opacity.offline_dim` 0.45, `opacity.offline_mark` 0.7 and `opacity.queued` 0.75 (from the Paper values).
- Screen: `--mock --screen hub-offline` (`consort_client::mock_offline`). Screenshot: `docs/screenshots/phase5-hub-offline-dark.png`. It matches `ERQ-0` in the offline parts: the pill and its text, the dimmed mark, "Status unknown" and the hollow dots, the dimmed answers, the faded queued bubble with its line, and the composer hint. The channels, the task list and the card text come from the existing mock, so they differ from the artboard, which was drawn with fewer channels.

### Tests

- `persist.rs` (10): every kind that the cache keeps comes back (all 18 kinds, the sets and the flags), a save writes only the rows that changed, a time that moves alone is saved with no row, a file that is not a database, a file cut in half, a newer schema (kept as `.broken`), a row that cannot be read, data of another hub or client, `delete`, and the mode 0600.
- `offline.rs` (5) and `mock_offline.rs` (2): the time label, the pill text, when saved data shows, unknown worker status, the blocked answers.
- `tests/fake_hub.rs` (7 new, against the fake hub; it got a `down` switch that refuses the WebSocket): a restart opens with the saved data, sends `sync.subscribe` with the saved position and no snapshot, and takes a change from the feed; a position that the hub does not have loads a snapshot; a damaged file is moved aside and a snapshot loads; unpair removes the file, the identity and the link, and a late save does not bring the file back; pairing with another hub removes the old data; the app opens offline with saved data, queues a message, and sends it when the hub is back; a hub that goes down while the app runs shows the time of the last update; a refused client loses the file.
- `consort-desktop/src/offline_tests.rs` (5, headless window): the pill, the hint and no "Runs on"; no pill online; an answer does not go out and shows the reason; answers work when the hub is back; the pending text.

### Checked by hand

- The mock screen was run and captured (see the screenshot). The tooltip on a dimmed button was not checked with a real pointer: it needs a hover, and moving the system pointer was not allowed here. It uses GPUI's own `.tooltip`.
- No real hub was started for this part.

## Rows for DECISIONS.md

| Area | Decision | Reason |
|---|---|---|
| Client cache | The saved cache is one SQLite file, `cache.sqlite3`, with `entity(kind, id, json)` and `meta(key, value)`, and `user_version` 1. It is not one table for each kind. | A new kind needs no schema change. The JSON is the wire form, so a proto change that keeps old JSON readable needs no code. The hub uses the same driver (`rusqlite`, `bundled`). |
| Client cache | The client saves 0.5 s after a change, in one transaction, and writes only the rows that changed. The wait does not restart on a new change. It saves only when the first load is whole. | A burst of feed entries makes one save. A busy feed is still saved. A half load must not replace a whole one. |
| Client cache | At start, the client reads the file before it connects and resumes with `sync.subscribe` and the saved `seq`. The hub answers `resync_required` for a position it does not have, and the client loads a snapshot. No hub change. | The resume path existed for reconnects in one run. The hub already checks the floor and the head of the feed. |
| Client cache | A file that cannot be used (not a database, failed integrity check, other schema version, a row that does not parse) is moved to `cache.sqlite3.broken`, and the client starts with no saved data. There is no migration: a new schema version moves the old file aside. Data of another client ID or hub URL is removed. | The file is a copy of hub data, so losing it costs one snapshot. A newer file stays for the newer app. A left journal would damage a new file, so it goes too. |
| Client cache | The file never holds the outbox, sign-ins, the ACP registry, the connector catalog, logos, the hub status, or blob paths. A test destructures `Cache` with no `..`, so a new field forces a decision. | The outbox has its own file. The rest is asked again when a page needs it. A new field must not be lost by accident. |
| Client | `Command::Unpair` and `Client::unpair()` remove the identity, the outbox and the cache file, end the link, and set `NotPaired`. Pairing with another hub and a refused link remove the file too. Pairing again ends a live link to the old hub. | The saved data is the old hub's private data. A paired app that stays on its old hub after a new pairing was a gap that the offline work showed. |
| Client | `ViewState.saved_data` is set when the link is `Offline`, the cache holds data and its time is known. The time is the last moment that the link was up, saved in the file. A worker has an unknown status, and the activity is empty. | The user must see how old the data is. The client cannot know a worker's state without the hub. |
| Client | `ViewState::answers_blocked()` is true in every connection state but `Online` and `Demo`. | An approval or decision needs the hub. A click gives a reason and sends nothing. |
| Desktop | The offline pill replaces the "Retrying in N s" line above the composer while saved data shows. Try again calls `reconnect`. The composer says "Messages wait here until the hub is back" in place of "Runs on". | Paper `ERQ-0` has one place for the notice. Two lines would say the same thing. |
| Desktop | Dimmed answers use a tooltip and a notice on click. The guards are in `answer_approval`, `answer_decision` and the connect and sign-in card actions, not only in the buttons. | A dimmed button is still a click target. Every path that sends an answer must stop. |
| Theme | New opacity tokens: `offline_dim` 0.45, `offline_mark` 0.7, `queued` 0.75. | Values from `ERQ-0`. |

## What is left

- **A hub that was restored from a backup** keeps its URL, and its head can be above the saved position. The client then resumes from a position that means something else. The hub has no ID that the client could compare. A hub ID in `hello` would close this gap.
- **An unpair button** has no screen in the designs. `Client::unpair()` is ready for it (for example in Settings > Hub).
- **The tooltip** on a dimmed button needs a look with a real pointer.
- **A session item list** per session is saved as summaries only, as the cache holds them.
