# Plan: a proactive assistant

Status: written on 2026-10-30 for the build after phases 2 and 3. The user asked for proactivity "like Duet, or better: look at how Duet, Grok Bot and Dots handle it, and come up with the best combination". The research is in [research/proactivity.md](research/proactivity.md). The decision is [ADR 0013](adr/0013-the-assistant-is-proactive-within-limits.md).

"Proactive" means: the assistant starts contact or work without a new message from the user.

## What we take from each product

All three products share one shape: a slow clock wakes the agent, the default answer is silence, the agent may read and draft alone but asks before it acts outward, and the user tunes it in plain words. Consort takes the best part of each:

| From | Idea | In Consort |
|---|---|---|
| Duet | Check-ins "every few hours", a morning brief, follow-ups on work that went quiet, "drafts first, asks before it sends" | The check-in, the morning brief, stale-task nudges, offers |
| OpenClaw (Clawdbot) | A heartbeat that reads a standing checklist (`HEARTBEAT.md`), a silent "OK" reply, active hours, an empty checklist skips the model | The check-in note, silent findings, quiet hours, the hub gate |
| Grok Bot | "Prepare before you execute", nudge a stalled handoff once, avoid broad listeners, one notification policy | Offers, one nudge per stale task, no broad listeners, tiers |
| OpenAI dots | The model picks its own wake time, idle research that is read-only by code, route updates by type | Follow-ups that the assistant sets, the read-only research pass, tiers |

What Consort adds that none of them document: one clear level per channel with an off switch, quiet hours plus snooze, hard budgets, back-off after silent check-ins, and a visible log of every proactive wake with feedback buttons.

## The rules that hold everything together

1. **Silence is the default.** A proactive wake that finds nothing ends without a visible message. "Nothing to report" is never posted.
2. **The hub decides before the model runs.** Level, quiet hours, snooze, budget and "nothing changed" are hub checks. A skipped wake costs no tokens.
3. **Read and draft alone, ask before acting outward.** The hub enforces this in code, not only in the prompt.
4. **Every proactive message is visible and accountable.** It carries a mark, it can be rated, and the log shows every wake, silent or not.
5. **The user tunes it in plain words.** "Stop pinging me", "tell me about invoices", "check again tomorrow" work in chat. The settings page shows the same state.

## Terms (add to CONTEXT.md)

- **Proactive turn**: a coordinator turn whose wake events are all proactive. A turn that also has a user message is a normal turn.
- **Proactivity level**: Off, Quiet, Normal or Eager, per channel.
- **Check-in**: a routine of kind `check_in` that wakes the coordinator to look for things that need the user.
- **Check-in note**: the memory note `check-in.md` that says what to watch and what to skip. Tutti's note is in global memory. A channel can have its own note in channel memory.
- **Follow-up**: a one-time wake that the assistant sets for itself, with a reason.
- **Offer**: a decision card that the assistant posts on its own initiative, with the options "Do it", "Not now" and "Never about this".
- **Quiet hours**: the hours when the assistant starts nothing and sends no notifications.
- **Proactivity log**: the list of every proactive wake and its outcome.

## Levels

| Level | What the assistant does on its own | Default for |
|---|---|---|
| Off | Nothing. It answers only. Stale tasks still show in the Tasks panel. | |
| Quiet | Follow-ups that it set, stale-task nudges, the guided start of a new channel | Every channel except Tutti |
| Normal | Quiet, plus check-ins and (in Tutti) the morning brief | Tutti |
| Eager | Normal, plus the read-only research pass and offers | |

The user sets a level per channel (Settings > Proactivity, and the channel settings dialog). The coordinator can set a quieter level at once with `set_proactivity` ("stop pinging me"). It cannot set a louder level: the tool refuses, and the assistant tells the user where to change it. This follows the Consort rule that management changes that widen access need the user.

## Mechanisms

### M1. Check-ins (from Duet and OpenClaw)

- A check-in is a routine with `kind: check_in`, action `coordinator_turn`, and an interval schedule.
- The hub creates it when a channel's level becomes Normal or Eager, and pauses it when the level goes below Normal. Defaults: every 3 hours in Tutti, every 6 hours in other channels. The least interval for a check-in is 30 minutes.
- Check-ins run only outside quiet hours.
- The wake event is `check_in` and carries a hub-made summary of what changed in the channel since the last check-in (see M3), and the findings of the last three silent check-ins, so the assistant does not report the same thing twice.
- The routine prompt is fixed by the hub. The user tunes the check-in through the check-in note, not the prompt. The routine row shows "Check-in" with the interval and "Uses check-in.md".

### M2. The check-in note (from OpenClaw `HEARTBEAT.md`, Duet "tell it what you want to hear about")

- Path `check-in.md`. Tutti's note is global memory. A channel note is optional; when it is missing, the channel check-in uses only the hub summary.
- Tutti's note starts with three lines when the hub creates Tutti's check-in: what to watch (tasks that need the user, mail and calendar if a connector exists), what to skip (newsletters, receipts, automated notices unless a payment failed), and "Ask before anything outward".
- The user edits it in chat ("skip receipts") or in Memory. The coordinator writes it like any note.
- Max 2 KiB. The hub cuts a longer note in the wake event and says so.
- Never secrets in it (the prompt already says this for all notes).

### M3. The hub gate (from OpenClaw "no tasks due" skip and Grok "a scheduled routine wakes even when there is nothing to do")

Before a check-in, the hub builds the change summary from its own state since the last check-in of that channel:
- tasks that changed status, approvals and decisions that wait, failed routine runs, workers that went offline or came back, new files, messages from other channels addressed to this one.

The hub skips the check-in with no model call (outcome `skipped_no_change`) when BOTH are true: the summary is empty, AND the check-in note (Tutti's note for Tutti, the channel note for a channel) does not ask to watch an outside source. A note asks to watch an outside source when the assistant marked it so: the check-in note may start with a front matter line `watch: outside` that the coordinator sets when the note lists mail, calendar or any connector. Tutti's starting note has it when a connector exists.

### M4. Silent findings (from OpenClaw `HEARTBEAT_OK` and `notify:false`)

- `no_reply_needed` gets an optional `finding`: one short line about what the assistant saw and chose not to report. It goes into the proactivity log and into the next check-in's wake event.
- In a proactive turn, assistant text without a posting call is dropped. It is NOT posted as the fallback reply (the fallback stays for normal turns).

### M5. Tiers: what may interrupt (from Grok, dots and the "interrupt only for a click or a break" rule)

Every message that a proactive turn posts is marked `proactive` with its kind. `post_message` gets `notify: bool`:
- **Notify** (tier 1): the user must act or a deadline is close: an approval or decision waits, a task failed, something is due inside 24 hours. The client shows an OS notification (switch "When the assistant needs your attention", on by default). At most 3 notifying proactive messages a day across the hub; after that the hub posts them without `notify` and says so in the log.
- **Post** (tier 2, the default): a normal message in the timeline. No OS notification, even for a channel that is not open. It shows in the unread count.
- **Silent** (tier 3): `no_reply_needed` with a finding.

During quiet hours and snooze, nothing notifies.

### M6. The morning brief (from Duet, with the empty-day rule)

- A cron routine in Tutti with `kind: brief`, weekdays at 07:30 in the user's time zone. The hub creates it once, when Tutti first has the level Normal or Eager. If the user deletes it, the hub does not create it again.
- The prompt: read every channel (`list_channels`, `read_channel`), the tasks, and the connected mail and calendar; post one ranked list of at most 5 items in three groups (Needs you, Ready for review, Done since yesterday); link each item to its task, message or source; prepare drafts as tasks in draft mode (M10) where they help. If nothing needs the user, post nothing (`no_reply_needed` with a finding).
- The brief may notify once (it is the one message the user expects).

### M7. Stale-task nudges (from Duet "follows up on threads that have gone quiet" and Grok "nudge a stalled handoff once")

- A hub timer, no model poll. A task in Needs input for 24 hours, or in Ready for review for 72 hours, adds a `stale_task` wake event to its channel, once per status entry.
- The coordinator decides: one short nudge, or silence. One nudge per task per status entry, never more.
- Level Quiet or higher. Deferred to the end of quiet hours.

### M8. Follow-ups the assistant sets (from dots "decide when to wake up" and OpenClaw one-shot `--at`)

- Tool `follow_up_later { after_minutes | at, reason, key? }` creates a routine with `kind: follow_up`, `origin: assistant`, a once schedule, and the reason as the prompt. The same `key` in a channel replaces the old follow-up.
- Limits: at least 15 minutes, at most 30 days ahead, at most 20 waiting per channel.
- Tool `cancel_follow_up { routine_id | key }`. The user cancels one in the Routines tab ("Follow-ups" section) or all of them in Settings > Proactivity.
- The wake event `follow_up` carries the reason and when it was set. A follow-up deletes itself after it runs.
- A follow-up that falls inside quiet hours runs at its time (the assistant chose it for a reason) but cannot notify.
- No hidden pass that guesses commitments (OpenClaw removed that for good reason).

### M9. The research pass (from dots "proactive research is read-only")

- Level Eager only. A routine with `kind: research`, daily at 06:30 in the user's time zone (before the brief, so the brief can use what it found).
- The hub enforces read-only in code for a turn whose only event is `research`: it allows the read tools (`list_*`, `read_*`, `search`, `fetch_*`, `memory_read`, `files_list`, connector tools whose hints say read-only), `memory_write` only under `research/`, `ask_decision` for offers (M10), and `no_reply_needed`. It refuses every other tool with the error "The research pass is read-only. Make an offer with ask_decision instead."
- At most 3 offers a day from research.

### M10. Offers, then act (from Duet "drafts first", Grok "prepare before you execute", dots "drafting does not give permission to send")

- An offer is `ask_decision` with the options "Do it", "Not now" and "Never about this", marked as an offer. "Never about this" makes the coordinator add a skip line to the check-in note.
- A task that a proactive turn starts runs in draft mode: the hub sets its approval mode to the strictest mode (every outward action waits for the user), unless the channel setting "Tasks the assistant starts on its own" is "Use the channel default". Drafts go to a draft place (a mail draft, a file). The approval cards stay until the user answers. They never expire and never run on their own.
- A proactive turn cannot resolve a task, widen access, add a connector, or turn on scripts. The management tools refuse these in a proactive turn.

### M11. Quiet hours, snooze and the off switch (the gap in all three products)

- Quiet hours: one global range, default 21:00 to 08:00, in the user's time zone. The desktop app sets the time zone from the system when the hub has none.
- Snooze: for 2 hours, until tomorrow 08:00, or for a week. While snoozed, no proactive wake runs except follow-ups (which cannot notify). The coordinator can snooze with `set_proactivity` ("not today").
- Off per channel, or "Pause all" in Settings > Proactivity.

### M12. The proactivity log and feedback (from OpenClaw's audit transcript and Grok's run history)

- The hub records every proactive wake: channel, kind, time, outcome, finding, the messages it posted and the task it started. Outcomes: `posted`, `notified`, `silent`, `skipped_no_change`, `skipped_quiet_hours`, `skipped_snoozed`, `skipped_level`, `skipped_budget`, `skipped_busy`, `failed`. The hub keeps 30 days.
- Settings > Proactivity shows the log with filters, and "Proactive turns today".
- Every proactive message shows two quiet actions on hover: "Useful" and "Too noisy". "Too noisy" wakes the coordinator with a `proactive_feedback` event; it adds a skip line to the check-in note and confirms in one short reply in the thread. "Useful" is only recorded.

### M13. Budgets and back-off (from OpenClaw's cost list and Duet's slow poll)

- A daily budget of proactive turns per channel: 8 in Tutti, 3 in other channels. A wake over the budget is skipped (`skipped_budget`). Follow-ups and the brief count but are never skipped for the budget.
- Back-off: after 6 silent check-ins in a row in a channel, the hub doubles that channel's check-in interval, up to 12 hours. A user message in the channel resets it.
- A proactive wake while the channel's turn runs waits (`skipped_busy` only if it waited more than one interval).
- Check-ins and the research pass jitter their start by up to 5 minutes, so routines do not all start at the same minute.
- Proactive turns use the channel's coordinator session, which already holds the context. The wake event stays small: the change summary and the note, not the history.

### M14. The guided start of a new channel (FINISH-PLAN phase 4 item 7, moved here because it is proactive)

- `channel.create` (not Tutti) adds a `channel_created` wake event. The coordinator greets in two short bubbles (who it is and what the channel is, then the offer to set it up) with quick-reply suggestions.
- Setup goes one question at a time: purpose, connectors, repos, model. Each question has suggestions (the names of the user's connectors, the repos it knows, the models). "I will type it" is a suggestion that only moves the focus to the composer.
- The coordinator applies the answers with its tools (channel update of its own channel: purpose, instructions, model; connector and repo changes as the existing tools allow, with the same risk rules).
- Level Quiet or higher (the default for new channels).

### M15. Quick replies and the writing bubble (FINISH-PLAN phase 4 item 7)

- `Suggestion { label, action: send | type }`. `post_message` and `reply` take at most 4 suggestions, each at most 40 characters. The client shows them as chips only under the newest assistant message of the timeline or thread. A send chip sends its label as the user's message. A type chip ("I will type it", dashed border) focuses the composer and sends nothing. Any user reply hides the chips.
- The writing bubble: three dabs of the mark at 10 px in the assistant lane while a coordinator turn runs in the channel and has not posted yet. It shimmers on the shared mark clock and stands still with Reduce motion.

## Out of scope, with reasons

- **Event watches on connectors** (Grok event routines, OpenClaw webhooks). They need a public webhook URL on the hub or connector triggers that Composio bills per event. The check-in with `watch: outside` covers the common need. Revisit with hub TLS (phase 5) and phone push (phase 6).
- **Phone push.** Phase 6. Tier 1 uses desktop notifications now. The hub records `notify` on the message, so push can use it later.
- **A cheaper model for check-ins in a separate session.** It breaks "one turn at a time per channel" and the persistent session already caches the context. Revisit when costs are measured.

## Contract (proto)

- `ProactivityLevel { off, quiet, normal, eager }`. `Channel.proactivity: Option<ProactivityLevel>` (none means the default for the channel kind). `channel.update` takes `proactivity`.
- `RoutineKind { custom, check_in, brief, follow_up, research }` and `RoutineOrigin { user, assistant, hub }` on `Routine` (serde defaults `custom` and `user`, so old rows read).
- `Message.suggestions: Vec<Suggestion>` (default empty, skipped when empty). `Message.proactive: Option<ProactiveMark { kind: ProactiveKind, notify: bool }>`. `Message.feedback` or a reaction-like field for Useful and Too noisy, whichever fits the store best.
- `ProactiveKind { check_in, brief, follow_up, stale_task, research, channel_created, feedback }`.
- Wake events: `check_in { routine, summary, note, recent_findings }`, `follow_up { routine, reason, set_at }`, `stale_task { task, title, status, since }`, `research { routine }`, `proactive_feedback { message, rating }`, `channel_created { name, kind }`. The existing `routine` event stays for custom routines.
- Hub settings: `ProactivitySettings { quiet_hours: { start, end }, time_zone, snoozed_until, paused, notify_cap_per_day }` with `proactivity.get`, `proactivity.set`, `proactivity.snooze`, and `proactivity.log { channel?, kind?, before?, limit }`. Feed entries so clients stay current.
- Tools: `follow_up_later`, `cancel_follow_up`, `set_proactivity` (quieter only, plus snooze), `post_message.notify`, `post_message.suggestions`, `reply.suggestions`, `no_reply_needed.finding`, `ask_decision.offer`.

## Screens (Paper first, from worldY 9600 on the Desktop page)

1. Settings > Proactivity: Pause all, quiet hours and time zone, snooze, the level of each channel, "Proactive turns today", and the log with filters.
2. A proactive message in the timeline with its mark ("Check-in", "Follow-up", "Morning brief") and the hover actions Useful and Too noisy.
3. An offer card (decision with Do it, Not now, Never about this).
4. The Routines tab with the "Follow-ups" section and a check-in row.
5. The guided start and the writing bubble: already designed (`F6N-0`, `FDX-0`, `FJC-0`).

## Tests that prove it

- Hub: level gates, quiet hours (including a range that crosses midnight and a time zone with DST), snooze, the budget, back-off and reset, the no-change skip with no model wake, the change summary content, stale-task timers (once per status entry), follow-up limits and replace by key, research read-only refusals, draft mode for proactive tasks, dropped fallback text in proactive turns, the notify cap, the log entries for each outcome, the morning brief created once and not again after delete, the check-in created and paused with the level.
- Client: suggestions show only under the newest assistant message and hide on reply, the notification rule for `notify` messages and not for other proactive messages, the writing bubble state.
- Desktop: the settings page, the chips (send and type), the writing bubble, the feedback actions, the Follow-ups section.
- Prompts: the coordinator prompt has a "Working on your own" section with the rules above, and the prompt tests check it.
