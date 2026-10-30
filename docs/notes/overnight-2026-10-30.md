# Overnight build, 2026-10-30

You asked for phases 2 to 5 of [FINISH-PLAN.md](../FINISH-PLAN.md), complete as specced, plus a proactive assistant and "Speak in your voice". This page is the summary for the morning. Every phase has its own notes file with the details, the differences from the design and the decisions. The decisions are also rows in [DECISIONS.md](../DECISIONS.md).

## What is built

| Phase | What | Notes |
|---|---|---|
| 2 | Routines (Channel panel tab, run history, the routine dialog with six schedule presets, Settings > Routines, skipped runs) and memory (Memory tab, note editor with the stale-write check, Settings > Memory with version history, restore and Recently deleted, the "memory saved" line) | [phase2.md](phase2.md) |
| 3 | Settings > Hub (status, storage, backups with a schedule, `consort hub restore`), Settings > Clients (Revoke, Pair a new app with a QR code), desktop notifications with a Dock badge and Settings > Notifications | [phase3.md](phase3.md) |
| 4 | Threads, reactions, Cmd+K search of the hub, New task, task delete and retry, the full output or diff of a task item, one channel settings dialog, the simple New channel dialog, groups, the task thread in bubbles, About, archived tasks. Dictation uses the system shortcut | [phase4.md](phase4.md) |
| 4b | The proactive assistant (see below) with the guided start of a new channel, quick-reply chips and the writing bubble | [phase4b.md](phase4b.md), [PROACTIVITY-PLAN.md](../PROACTIVITY-PLAN.md) |
| 4c | Write in my voice (see below) | [phase4c.md](phase4c.md), [VOICE-PLAN.md](../VOICE-PLAN.md) |
| 5 | `consort worker install` (launchd, systemd, Task Scheduler), install scripts, hub deployment (systemd, Docker, Caddy for TLS), a release workflow, a macOS `.app` bundle, app updates, the three-step Add worker dialog, a client cache on disk, and the app's state when the hub is offline | [phase5-install.md](phase5-install.md), [phase5-cache.md](phase5-cache.md) |
| Smaller items | Composio status, a logo cache, private and ssh skill repos, archive and npm plugin sources, the skill cache prune, expired Claude logins, doc fixes, time limit tests | [smaller-items.md](smaller-items.md) |

Also: the top tint is much darker at the mark and the pill, as you asked ([screenshot](../screenshots/top-tint-darker-dark.png)), the checks are faster (`scripts/check.sh`, nextest), and a flaky heartbeat test is fixed.

## The proactive assistant

Research: [research/proactivity.md](../research/proactivity.md) (Duet, Grok Bot, OpenClaw, OpenAI dots). Decision: [ADR 0013](../adr/0013-the-assistant-is-proactive-within-limits.md).

- A level per channel: Off, Quiet (default), Normal (Tutti's default), Eager.
- Check-ins (Tutti every 3 hours) read the memory note `check-in.md`. The hub skips the model when nothing changed. Silence is the default.
- The morning brief (weekdays 07:30), stale-task nudges (once), follow-ups that the assistant sets, a read-only research pass with offers (Eager only).
- Quiet hours 21:00 to 07:00, snooze, Pause all, at most 3 notifications a day, a daily turn budget, back-off after silent check-ins, and a log with Useful and Too noisy.
- The hub, not the prompt, enforces the limits. A review found ways around them; all 15 findings are fixed ([review-fixes.md](review-fixes.md)).

## Write in my voice

Research: [research/speak-in-your-voice.md](../research/speak-in-your-voice.md). No public source documents Duet's toggle, so the design combines Gmail, Superhuman, Grammarly, Claude styles, dots and OpenClaw. Decision: [ADR 0014](../adr/0014-the-assistant-writes-in-the-users-voice-with-consent.md).

- Off by default. You choose the sources. A task learns a style guide, `voice.md`, and you approve it before it is saved.
- Only text that goes out in your name uses it. The approval card says which voice it used, flags dates, amounts and agreements that you did not ask for, and keeps Allow off until you check them. Edit on the card teaches the voice.
- While the voice is on, every send asks you, whatever the connection's rule. Only your own words count as the request. The hub owns `voice.md` and your rules.
- A review found 16 problems; all are fixed ([voice-fixes.md](voice-fixes.md)). Left open there: Edit of Slack `blocks`, a "sent mail only" check for Gmail sources, and a stricter harness mode for voice turns.

## Checks

- macOS: the full `scripts/check.sh` passes on main.
- Windows (bigboi): three runs. `cargo test --workspace` passes (1926 tests in the third run). Fixes from those runs are merged ([windows-checks.md](windows-checks.md)).
- Screenshots of every new screen are in `docs/screenshots/` (`phase2-*`, `phase3-*`, `phase4-*`, `phase4b-*`, `phase4c-*`, `phase5-*`, `windows-*`).

## What needs you

1. **Live runs with real models and accounts.** No real model ran tonight; every test uses the fake worker and fake connectors. Try: a check-in and the morning brief in Tutti, a new channel's guided start, "Write in my voice" with your sent mail, and a real draft card.
2. **Apple Developer ID** for signing and notarizing the macOS app (the secret names are in `docs/install.md`).
3. **The final domain.** The bundle IDs are `dev.consort.worker` and `dev.consort.desktop`, and the repo slug for releases is `watzon/consort`. Both are hard to change after users install.
4. **A real `consort worker install`** on each system (tonight only `--dry-run` ran; nothing was installed on any machine).
5. **The earlier live checks** in FINISH-PLAN.md: the Composio run, real sign-ins, skills at the model level, the extras switches.
6. **Light mode.** Tonight's screenshots are dark only.

## Housekeeping

The agents worked in git worktrees under `~/Projects/personal/consort-wt/`. Every branch is merged into main. The worktrees hold large build folders; remove them with `git worktree remove <path>` when you no longer need them. Nothing was pushed: the repo has no remote.
