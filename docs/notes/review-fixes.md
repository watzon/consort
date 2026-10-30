# Review fixes: proactivity, backup and restore, client cache

These notes are for the merge into `docs/DECISIONS.md`. The review is `consort-wt/review-report.md` (15 findings, at `2057d66`). The orchestrator decided the fixes. Written on 2026-10-30. Every finding was true in the code, so none was skipped. Each fix has a test that fails without it.

## Schema

The hub schema goes to 11. `schema_v11.sql` adds `proactive_tasks` and `follow_ups.set_in_proactive_turn`. Version 10 is kept for the voice build: this branch has a `schema_v10.sql` that changes nothing (`SELECT 1;`). When the voice branch merges, its `schema_v10.sql` takes the place of this file, and the list in `store/migrations.rs` stays as it is. The hub epoch lives in the `meta` table and needs no migration.

## One line per finding

1. **Fixed.** `message_channel` is refused in a proactive turn with the decided text (`proactivity/turn.rs`). `proactive_tasks` records each task that a proactive turn starts, and `transition` puts the turn's kind on its `task_update` (`WakeEvent::TaskUpdate.proactive`, hidden from the model), so the turn that the update wakes is proactive. User messages, decision answers, card answers (allowed or denied), and ratings are user-caused (`WakeEvent::user_caused`) and make a normal turn. A turn of memory suggestions only is a new `TurnKind::Suggestions`: normal (no gate, no marks), but only memory tools, `suggest_memory`, posting, `react` and `no_reply_needed`. Before, it was a full normal turn. A task that a normal turn starts after "Do it" on an offer runs in draft mode, as the offer's text says. Tests: `proactivity/tests/review.rs`, `turn.rs`, `turn_input.rs`.
2. **Fixed.** `run_connector_tool` uses Ask every time for a tool that is not read-only in a proactive turn and in a draft task (`Hub::connector_calls_ask_each_time`). Earlier grants are not read, and the answer saves no grant. A three-line change in `connectors/run.rs`; the tests are in a new file, `connectors/proactive_tests.rs`.
3. **Fixed.** `message_task` and `retry_task` in a proactive turn reach only a task that a proactive turn of the same channel started in draft mode (`Hub::check_steer_task`).
4. **Fixed in the protocol.** `session.turn` had no per-turn mode. New: `SessionTurn.approval_mode` and `SessionStart.turn_approval_mode`, `HarnessSession::set_approval_mode` (default `Unsupported`). The worker runs a turn at the lower of the turn's mode and the session's mode and goes back for the next turn. Claude (ACP) changes the mode with its start call and reads it back; Codex sends `approvalPolicy` and `sandboxPolicy` with `turn/start` when the mode changed. Other ACP agents and Pi refuse, so the worker refuses a stricter turn with `policy_denied` (fail closed). The hub sends Ask for proactive and research turns. Tests: worker `core.rs`, ACP and Codex adapter tests, hub `coordinator.rs` and `tests/proactivity.rs`.
5. **Fixed.** `classify` follows "the stricter rule wins": a user-caused event makes a normal turn; else any proactive event makes the turn proactive; research with anything is research. `turn_batch` gives the research pass a turn of its own.
6. **Fixed.** `set_proactivity` refuses `snooze: off` and keeps the later of the current and the new end. The tool has no `paused` argument, so it cannot end Pause all; a test checks that too.
7. **Fixed.** Follow-ups and the brief already counted in `turns_since`. New: a follow-up set in a proactive turn (`follow_ups.set_in_proactive_turn`) is `skipped_budget` when the budget is used; at most 10 follow-ups run in a channel in a day. The brief and the research pass must have 12 hours between runs: `schedule::shortest_gap_ms` walks the next runs (400 days, at most 1,000 runs) and `check_routine_change` refuses a shorter gap, for tools and clients alike. A follow-up's `routine_update` also keeps the follow-up limits (the review asked for it; small and in the same place).
8. **Fixed.** `Shared.generation` goes up under the cache lock in Pair and Unpair (`Shared::new_owner`), which also drops the live link sender. A link, a call (`Shared::call`) and a recover carry the generation of their start and write the cache only through `with_cache_of`, which checks it under the lock. A link whose owner changed ends with "The app paired with another hub, so this link ended."
9. **Fixed.** A recover raises `Shared.recovering`, and `save_cache` writes nothing while it is above zero. The live position still moves, so the feed goes on; the file keeps the position before the gap until the data is in. A failed recover also ends the hold (it sets `loaded = false` and restarts the link, as before).
10. **Fixed.** Archive names use UTC. Prune sorts by the manifest's `created_at`, then by the name time read as UTC when the manifest cannot be read.
11. **Fixed.** `follow_up_time` uses checked math and gives "out of range" or "at most 30 days" errors. The writer thread now catches a panic of one job (`catch_unwind`), so one bug fails one write and the hub goes on. The same pattern elsewhere: the blob signature time check (`net/blobs.rs`, a header can hold any number) now uses `saturating_sub` and `unsigned_abs`; the late check in the scheduler and the back-off steps use saturating math too.
12. **Fixed.** `begin_proactive_run` applies the "more than 1 hour late" rule to scheduled runs of every proactive kind, with the same run text as other routines; a late follow-up is deleted. The temp database copy is created with `create_new_private_file` (0600) before SQLite opens it.
13. **Fixed** (findings 14 and 15 of the report). A research-pass write under `research/` posts no "memory saved" line. The hub has a random `hub_epoch` in `meta`, made when it creates its database (or opens one without it) and again at restore; `hello` sends it. The client keeps it in `Cache.hub_epoch` and in the saved file (`meta.hub_epoch`), and loads a snapshot instead of a resume when it differs.

## Decisions I made where the orchestrator left a choice open

- **Gate for proactive-origin task updates.** "All its limits" means the turn limits: the tool rules, draft mode, marks, the dropped fallback text, the notify rules and the harness mode. The start gate (level, quiet hours, budget) does not apply to a task update, because dropping it would hide the end of a task from the coordinator.
- **A rating is user-caused.** The orchestrator listed it. So a `proactive_feedback` turn is now a normal turn: its reply has no mark, and unposted text uses the normal fallback.
- **An expired or canceled card is not user-caused.** Only an allowed or denied card is the user's answer.
- **"Do it" tasks are draft tasks** with the offer's kind, so their updates stay proactive. The turn of the answer itself is normal, as decided.
- **A late follow-up is skipped and deleted**, like a late custom once routine, with no log row. The routine run says why. No new outcome was added to the proto.
- **Per-turn mode only goes down.** A turn's mode is capped by the session's mode, so a turn can never raise access.
- **Codex goes back to the plain policy of its mode.** Codex keeps turn overrides, so the adapter sends the mode again when it goes back. The thread's config extras for the sandbox (if the user set them) are not sent again.
- **Harnesses without a mode change fail closed.** A coordinator on OpenCode, Gemini CLI or Pi in a channel above Ask cannot run proactive turns; the worker refuses them with a clear error.
- **Saves wait during a recover**, instead of a separate saved cursor. Simpler, and a quit loses at most the changes since the last save, which the next start replays.
- **The epoch is a 64-hex random token.**

## Docs changed

`protocol.md` (hello `hub_epoch`, `session.turn.approval_mode`, `session.start.turn_approval_mode`, `set_proactivity`), `data-model.md` (schema 10 and 11, `hub_epoch`, cache meta, the hold and the generation, archive names and prune), `harness-adapters.md` ("A mode for one turn", the trait), `PROACTIVITY-PLAN.md` (the proactive turn term, M8, M9, M10, M11, M13), `CONTEXT.md` (Proactive turn), and the coordinator prompt (small additions to "Working on your own", with the prompt test).

## What is left

- No real harness was run with a per-turn mode change. The ACP and Codex adapters are tested against their fake agents, and the Codex request shape comes from the vendored v2 schema.
- The approval card does not show that a proactive turn asked "every time" on purpose. The voice build changes the card now, so I left it.
- Old `.consort-backup-*` files that a stopped backup left are not removed at start.

## Rows for DECISIONS.md

| Area | Decision | Reason |
|---|---|---|
| Hub | A batch is a normal turn only with a user-caused event (message, decision or card answer, rating); else any proactive event makes it proactive; research gets a turn of its own | The stricter rule wins, so a merge never drops the limits |
| Hub | `task_update` carries the kind of the proactive turn that started the task (`proactive_tasks`, schema 11) | Proactive limits travel with the work they cause |
| Hub | A turn of memory suggestions only may use memory tools and post, nothing more | A suggestion is text from a task, not an instruction from the user |
| Hub | `message_channel` is refused in a proactive turn; `message_task` and `retry_task` reach only the turn's own draft tasks | A proactive turn must not start ungated work elsewhere |
| Hub | Connector calls that write or send ask each time in a proactive turn and in a draft task, ignoring earlier allows | "Ask before acting outward" in code |
| Hub | A task started after "Do it" on an offer is a draft task | The offer says the work runs in draft mode |
| Protocol | `session.turn.approval_mode` and `session.start.turn_approval_mode`: a mode for one turn, at most the session's | Proactive and research turns run the coordinator harness in Ask |
| Worker | A harness that cannot change its mode in a session refuses a stricter turn (`policy_denied`) | Fail closed |
| Hub | `set_proactivity` only starts or extends a snooze; never ends one or Pause all | The tool only makes things quieter |
| Hub | At most 10 follow-ups run per channel per day; a follow-up set in a proactive turn is skipped over the budget | No endless chains |
| Hub | The brief and the research pass need 12 hours between runs; the hub walks the next runs of the schedule | A brief every minute is not a brief |
| Hub | Proactive routines follow the "more than 1 hour late" rule; a late follow-up is deleted | Same rule as every routine |
| Hub | The writer thread catches a panic of one job | One bug must not stop every write |
| Backup | Archive names in UTC; prune by manifest time | A local-time name can go back |
| Backup | The temp database copy is 0600 from creation | The backup folder may be shared |
| Hub | Research notes under `research/` post no memory line | The research pass never posts |
| Protocol | `hello` has `hub_epoch`, new at database creation and at restore; the client loads a snapshot when it differs | A restored hub's feed numbers name other changes |
| Client | A generation per pairing; old links and calls never write the cache | Old hub data must not land under a new owner |
| Client | No save while a recover waits for skipped data | The saved cursor never passes data that is not in the file |
| Hub schema | Version 11; version 10 is a no-op kept for the voice build | Two branches add migrations at the same time |
