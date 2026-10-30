# The assistant is proactive within limits that the hub enforces

The assistant starts contact and work on its own: check-ins, a morning brief, follow-ups that it sets, nudges on stale tasks, a guided start for a new channel, and (when the user wants it) a read-only research pass with offers. The hub, not the prompt, decides when a proactive turn may run and what it may do. Silence is the default answer. The full design is [PROACTIVITY-PLAN.md](../PROACTIVITY-PLAN.md).

## Context

- The user asked for proactivity like Claude Duet, or a better combination of Duet, Grok Bot and OpenAI dots. The research is in [research/proactivity.md](../research/proactivity.md).
- All three products wake the agent on a slow clock, stay silent by default, and let the agent read and draft but not act outward without the user. They differ in the controls. None documents a clear off switch with quiet hours, snooze, budgets and a visible log together.
- Reviews of Duet name two problems: urgency that is wrong, and notifications that the user cannot trust. A wrong alert costs more trust than a missed one.
- Consort already has the parts: wake events, one turn at a time per channel, a silent tool (`no_reply_needed`), routines with once and interval schedules, decisions, approval modes and the channel cap on tasks.

## Decision

- A proactivity level per channel: Off, Quiet, Normal, Eager. Tutti starts at Normal, other channels at Quiet.
- Proactive wakes are routines with a kind (`check_in`, `brief`, `follow_up`, `research`) or hub timers (`stale_task`) or hub events (`channel_created`). They become wake events like the others.
- The hub gates every proactive wake before a model runs: the level, quiet hours, snooze, a daily budget, back-off after silent check-ins, and a "nothing changed" check. A skipped wake costs no tokens and is logged.
- In a proactive turn the hub drops assistant text that no posting tool sent, puts tasks into the strictest approval mode, refuses management changes that widen access, and (for the research pass) refuses every write tool.
- A proactive message notifies only when the assistant sets `notify`, at most 3 times a day, never in quiet hours. Every proactive message is marked, can be rated Useful or Too noisy, and appears in the proactivity log.
- The user tunes what to watch in a memory note, `check-in.md`, in chat or in Memory.

## Considered options

- Proactivity only in the prompt ("check in when useful"). Rejected because the model cannot see the clock, the budget or the user's sleep, and a prompt rule is not a safety rule.
- A separate cheap session for check-ins. Rejected for now because it breaks one turn at a time per channel, and the persistent coordinator session already holds the context.
- A hidden pass after each reply that finds commitments to follow up. Rejected because OpenClaw added it and removed it. The assistant sets follow-ups on purpose with a tool.
- Event watches on connectors. Deferred because they need a public webhook URL or paid connector triggers.

## Consequences

- New proto types (levels, routine kinds, proactive marks, suggestions), new wake events, new hub settings and requests, a new settings page, and a migration.
- The coordinator prompt gets a section on working on its own.
- Proactive messages carry `notify`, so phone push (phase 6) can use the same tier.
