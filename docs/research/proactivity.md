# Proactivity research: Claude Duet, Grok Bot (and OpenClaw), OpenAI dots

Date: 2026-10-30. Purpose: design proactivity for Consort.

"Proactive" here means: the assistant starts contact or work without a new user message.

## Summary

All three products use the same shape. Only the numbers differ.

1. A cheap, slow clock wakes the agent. Duet checks "every few hours". OpenClaw checks every 30 minutes by default. Dots pick their own wake time.
2. The default answer to a wake is silence. OpenClaw has `HEARTBEAT_OK` and `NO_REPLY`. Duet "messages you only when something needs your attention". Grok Bot "only come[s] back when something needs your approval".
3. The agent may read and draft alone. It must ask before it sends, pays, deletes or publishes. Dots make this a code rule: idle research uses read-only tools.
4. The user tunes proactivity in plain words. A short standing note holds the rules (OpenClaw `HEARTBEAT.md`, Duet "tell it what you want to hear about and how often", dots "which changes deserve your attention").
5. Cost is the main limit. Duet polls slowly because "checking nonstop would use up your plan's limits much faster". OpenClaw documents how to cut heartbeat cost from about 100K tokens to 2K to 5K per run.
6. Nobody documents quiet hours except OpenClaw (`activeHours`). Duet and dots document none.
7. Two ideas failed or look weak. OpenClaw removed "inferred commitments" (hidden extraction of follow-ups). Reviewers found Duet's morning brief and priorities unremarkable, and two reviewers could not tell if notifications worked.

"Grok Bot" is most likely xAI's (now SpaceXAI) Grok Bot, launched 2026-08-11. OpenClaw (earlier Clawdbot) is the second candidate. I researched both. See section 2.

Source quality rules for this report:

- FACT means a quote from a page I read. The URL follows the quote.
- INFERENCE means my reading. It has no quote.
- "Excerpt" means the page text came from a summarizing fetch tool, not the full page. Check those quotes before you cite them in a public document. The safety page for Duet is in this class.
- Pages from Anthropic, OpenAI, xAI and OpenClaw are primary. Blogs, videos and review sites are secondary. I label them.

---

## 1. Claude Duet (Anthropic, research preview, 2026-10-21)

Primary pages read:

- Launch post: https://claude.com/blog/claude-duet (same text at https://claude.com/resources/articles/claude-duet)
- Help center: https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- Safety post: https://claude.com/resources/articles/claude-duet-safety-privacy-and-security (excerpt)
- Product page: https://claude.com/product/claude-duet (excerpt)
- Release notes: https://support.claude.com/en/articles/12138966-release-notes (only the 2026-10-21 launch entry, no later Duet entry found)

### 1.1 Triggers

Time-based, fixed morning brief:

- FACT. "Each morning, it sends you a short brief on the most important items across your connected apps." (https://claude.com/blog/claude-duet)
- FACT. "Each morning, it looks across your email and calendar, picks what most needs your attention, and can start the first steps, like drafting replies for you to review." (support article)

Time-based, polling check-ins:

- FACT. "Claude Duet checks your connected accounts every few hours, because checking nonstop would use up your plan's limits much faster." (launch post)
- FACT. "It checks your accounts at set intervals, so something urgent can arrive between checks." (support article, Limitations)
- FACT (secondary, one tester). Peter Yang "traces its proactive behavior to a default recurring check-in and prefers fewer interruptions than its initial schedule." (https://intelligentartifact.com/posts/claude-duet-is-here-everything-you-need-to-know/)
- INFERENCE. The check-in is an ordinary scheduled routine that the user can see and edit. Yang's text says "a visible routine". No Anthropic page says this.

Event-like triggers (work finished, approval needed):

- FACT. "You'll receive a notification when the task is complete or needs your review." (launch post)
- FACT. "Your main thread stays quieter: Claude Duet posts there when a task is done or needs a decision from you." (support article)

Follow-ups on stale work:

- FACT. "Through the day, it sends updates on the work it's doing and follows up on threads that have gone quiet." (support article)
- FACT. "If you can't approve right away, Claude Duet finishes the rest of the task and brings the approval back to you later." (support article, "Privacy, safety, and control")

Noticing things unasked (examples from testers, in the launch post):

- FACT. "One day, without being asked, it flagged a property showing that was on her calendar for the wrong day."
- FACT. "It also noticed the call fell right before her flight and offered to move it."

Idle work:

- INFERENCE. Duet has no named "idle research" mode. The check-in does the equivalent: it reads accounts and starts "the first steps, like drafting replies".

### 1.2 Decision to speak

- FACT. "We designed it around Claude Opus 5.5 and built it to be resourceful and to decide what needs you, what can wait, and when to check with you first." (launch post)
- FACT. "Throughout the day, it keeps tabs on those apps and messages you only when something needs your attention." (launch post)
- FACT. "We're continuing to test its judgment about when to message you, and we'll keep tuning it based on what testers tell us." (launch post)
- FACT. No published threshold, score or rule defines "needs your attention". The model decides.
- FACT. "It still might misread a thread or miscalibrate what's urgent." (launch post)
- FACT (secondary). Claire Vo found that "an email and Slack review surfaced some useful items, but its priorities and context did not seem right", and that morning briefings "did not feel compelling". (https://soundcast.cc/shows/how-i-ai/claude-duet-is-beautiful-i-still-forgot-to-use-it, summary text)
- FACT (secondary). The reviewer "botsinproduction" set a similar rule for their own bots: "a bot may interrupt a human for exactly two reasons: it needs a click, or it broke. Everything else goes into a daily note at 18:00." Their test: "if it pings me ten times a day I'll mute it in a week; if it pings me twice and is right both times I'll never turn it off." (https://botsinproduction.com/blog/claude-duet-review-from-someone-who-built-one-by-hand/)

### 1.3 Delivery

- FACT. One ongoing thread plus channels: "You can also start separate channels for different topics, and they share the same memory." (support article)
- FACT. Tasks tab and quiet main thread (quote in 1.1).
- FACT. "It may suggest that a task looks finished, but only you can mark a task as completed." (support article)
- FACT. Mobile notification for done or needs review (quote in 1.1).
- FACT. No page describes batching, digest rules or quiet hours.
- FACT (secondary, one tester). A recap routine "set for 9 a.m. to 5 p.m." in an "artifacts channel" reminds the user to return to unfinished work. (https://usefollowed.com/episodes/riley-brown-how-i-m-actually-using-claude-duet-after-10-hours-of-testing, summary)
- FACT (secondary). Claire Vo "is unsure whether notifications were enabled" and forgot slow threads (soundcast link above).

### 1.4 User controls

- FACT. "You can also tell it what you want to hear about and how often." (launch post)
- FACT. "You can disconnect any app at any time." Memory of past information is kept. (support article)
- FACT. No page documents an off switch for the brief or check-ins, a cadence setting or quiet hours. The Anthropic pages say nothing on these.
- FACT (secondary, unofficial site). "There is no settings screen full of toggles for the brief. You shape the Claude Duet morning brief by talking to it in the thread, and it carries that into tomorrow's ranking through your Claude memory." (https://claudeduets.com/claude-duet-morning-brief). Treat as unverified.
- INFERENCE. Tuning is by conversation plus memory, not by settings.

### 1.5 Safety (what it may do unasked, what it must ask)

- FACT. "Claude Duet reads, drafts, and prepares work on its own. But before it sends a message, makes a payment, signs a document, or deletes anything, it shows you exactly what it's about to do and waits for your approval." (launch post)
- FACT. "A few things are off-limits even with your approval, like trading securities or moving money between accounts." (launch post)
- FACT. "Claude Duet is built to treat what it reads as information and to act on what you ask, but no defense is perfect, which is one reason consequential actions need your approval." (support article)
- FACT (excerpt). The check "runs outside the model doing the work so instructions hidden in a page cannot affect the outcome." And: "The routines that run while you are away have the same safeguards." (safety post)
- FACT (excerpt). "Your own request counts as consent, so 'send a note to my dentist about my tooth pain' generally won't need additional approval." Some steps "aren't covered by a 'don't ask me again' rule ... like paying or sending money, deleting an order, or creating an account." (safety post)
- FACT. "It doesn't make phone calls or send text messages to other people." (support article)

### 1.6 Cost controls

- FACT. "Work it does in the background, like the morning brief and check-ins, also counts, so you may reach your limits sooner than with chat alone. Claude Duet checks your accounts at set intervals to keep that background work from using up your limits." (support article)
- FACT. "Its background checks count toward those limits, and larger tasks ... require more model turns and background work." (launch post)
- FACT. "You can't choose a model. Claude Duet runs on Claude Opus 5.5." (support article)
- INFERENCE. The polling interval is the cost control. No per-day cap or budget view is documented.

### 1.7 Gaps in Duet

- No documented quiet hours, snooze or cadence setting.
- No documented noise metric.
- "Something urgent can arrive between checks." Duet offers no event push for urgent mail.

---

## 2. "Grok Bot"

### 2.1 Which product did the user mean?

Candidates:

| Candidate | Fit | Evidence |
|---|---|---|
| xAI / SpaceXAI **Grok Bot** (launched 2026-08-11) | Best. Exact name. Same product class as Duet and dots: always-on agent with its own computer, routines, approvals. | https://x.ai/news/introducing-grok-bot |
| **OpenClaw** (earlier Clawdbot, then Moltbot) | Second. Sounds like "Claw bot". Famous for heartbeat proactivity. Open source and self-hosted, like Consort. | https://docs.openclaw.ai/gateway/heartbeat |
| Grok chat features (Automations, Tasks, Companions) | Weak. Automations are scheduled jobs only. Companions ended on 2026-09-01 (secondary source). | https://x.ai/news/grok-automations |

INFERENCE. The user listed Duet and dots in the same request. Grok Bot belongs in that group. OpenClaw is also a good model for a self-hosted design. So this report covers both, with equal depth. The user's memory file says dictation software mishears product names, so this guess can be wrong.

Note on names: x.ai pages now say "SpaceXAI" or "SpaceXSI". I write "xAI" or "Grok Bot" for short.

### 2.2 Grok Bot

Primary pages:

- Launch: https://x.ai/news/introducing-grok-bot
- Skills and routines docs: https://docs.x.ai/grok-bot/skills-routines-and-automations
- Settings and notifications docs: https://docs.x.ai/grok-bot/settings-and-notifications
- Bots docs: https://docs.x.ai/grok-bot/bots
- Changelog: https://x.ai/changelog/bot
- Grok chat Automations: https://x.ai/news/grok-automations

Secondary (detailed, dated 2026-10-29): https://flaviocopes.com/grok-bot/ . Also https://www.grokbotwiki.com/guides/permission-boundaries and https://framewright.cloud/blog/grok-bot-first-two-months .

#### Triggers

Scheduled routines:

- FACT. "A routine tells one Bot when to run a workflow: on a schedule, or when something happens, such as a Slack message, a GitHub, Linear, Sentry, or PagerDuty event, an email, or a webhook call." (docs.x.ai skills page)
- FACT. "A new routine waits for its next scheduled time; it doesn't run when it's created." (same page)
- FACT (secondary, quoting changelog). "Routines can run up to 300 times an hour in total, and up to 200 times each." Also 50 routines per Bot and the 20 most recent runs kept. (flaviocopes)

Event routines:

- FACT. "Only new activity counts: events from before the trigger existed don't start a run." (docs.x.ai)
- FACT. "Avoid broad listeners such as 'every new message.' They create noise, consume usage, and increase the chance of acting on irrelevant input." (docs.x.ai)
- FACT (secondary). "New mail doesn't wake a Bot. To act on it, set up a routine that watches the inbox." (flaviocopes, citing the apps docs)
- FACT (secondary). "Eric Zakariasson recommends using an event trigger when one exists. ... A scheduled routine wakes up even when there is nothing to do." (flaviocopes)

Follow-ups and nudges:

- FACT. "They can follow up on threads you dropped, nudge a stalled handoff, and pick work back up from previous conversations." (launch post)
- FACT. "Over time they become more proactive, picking up work before you need to ask and knowing when something needs your attention." (launch post)
- FACT. Primary Bot: "Your primary Bot is your main point of contact. It checks in with you on its own and coordinates work with your other Bots." (docs.x.ai/grok-bot/bots)
- FACT (secondary, citing changelog). "Since October 2, the Primary Bot also spots work it can take off your plate and offers to handle it." (flaviocopes)
- FACT (secondary). "The word that matters is offer. It proposes, and you decide." (framewright)
- FACT (secondary). A "workforce-checker Bot" idea: it "periodically checks the other Bots, nudges a stalled Bot once, stays quiet while work is moving, and contacts you only when a person must intervene." (flaviocopes, a user tip, not a product feature)

Idle work:

- INFERENCE. No named idle-research mode. The Primary Bot "offer" behavior is the nearest thing.

#### Decision to speak

- FACT. "They finish jobs end to end, and only come back when something needs your approval." (launch post)
- FACT. "After a few tasks, they pick up your voice, your edge cases, and know when to ping versus keep going." (launch post)
- FACT (docs). The routine template asks the user to define "The approval boundary" and "What should happen when a source is missing". (docs.x.ai)
- FACT (secondary). "The schedule and the notification policy are separate decisions. A watcher can run every hour and save every check while contacting you only when a price changes, a deployment fails, or another defined threshold is crossed. A quiet run is still a successful run." (flaviocopes)
- FACT (secondary). "Tell every Bot to report as soon as it cannot make progress. A blocked Bot should not burn through the remaining plan by retrying the same step." (flaviocopes)
- FACT (secondary). An inbox Bot "scans only messages that probably need a reply. It skips newsletters, receipts, calendar responses, automated notifications, and internal noise. ... It does not send anything, and it stays quiet when nothing needs attention." (flaviocopes)

#### Delivery

- FACT. "Turn on Notifications for a Bot to get a system or mobile notification when that Bot finishes or is waiting for you." (docs.x.ai settings page)
- FACT. "While you use the desktop app on macOS or Windows, notifications from Bots you own go to the desktop instead of your phone. ... Finished replies skip your phone. When a Bot needs your input, your phone is notified about 3 minutes later unless you open that Bot, turn off its Notifications, or hide it from your sidebar first." (same page)
- FACT. Routine sends wait as drafts: "Sends from a routine still wait as drafts until you press Send." (flaviocopes)
- FACT. Grok chat Automations: "You choose how each automation reports back: email, app notification, both, or neither if you'd rather check in yourself." (https://x.ai/news/grok-automations)
- FACT. No quiet hours are documented. The settings page lists a Timezone used by routine schedules.

#### User controls

- FACT. Per-Bot notification switch (above). Routine menu: "Right-clicking a routine in the details pane gives you Pause, Resume, Test, Edit, and Delete." (flaviocopes, citing changelog)
- FACT. "Test" runs real work: "A test run performs real work. It can navigate websites, change files, and call connected tools." (docs.x.ai)
- FACT. Schedule in plain words: "Ask for a schedule in plain words, like 'every weekday at 8:00 AM' or 'every 2 hours.' Then open the routine and check When to run, which shows the saved schedule in words." (docs.x.ai)
- FACT. A Primary Bot is optional to replace, not to switch off. No global "proactivity off" switch is documented.

#### Safety

- FACT. "Design routines for trust: Automate preparation before execution. Have the Bot draft, reconcile, or recommend first. Require approval for sending, purchasing, deleting, publishing, or changing production systems. Include a no-data and stale-data policy. Make retries idempotent where possible." (docs.x.ai, condensed list)
- FACT (secondary, citing docs). "A card raised by a routine or other background work expires after 30 minutes. A card raised by a chat message waits for your answer." (flaviocopes)
- FACT (secondary). "Approvals raised by routines, triggers or other Bots expire after about 30 minutes ... and the action does not run." (grokbotwiki)
- FACT (secondary). "Instructions inside an email never count as your permission to act." (flaviocopes)
- FACT. Auto Review: "A review model that checks tool calls and computer actions before they run." Custom Rules: "Ask first wins when both match." (grokbotwiki; changelog)

#### Cost controls

- FACT. "Grok Bot comes with its own usage, separate from your Grok and Cursor plans." (launch post)
- FACT (secondary). "Every run consumes usage, even when the Bot wakes up and finds nothing. ... a routine scheduled every 15 minutes runs 96 times a day. An hourly run or a few checks per day is often enough." (flaviocopes)
- FACT (secondary). "Delete a one-shot watch or monitoring routine after the event has passed. Otherwise it can keep waking up, finding nothing, and consuming usage." (flaviocopes)
- FACT (secondary). On group chats, "the group host still wakes each member in turn behind the scenes. Every Bot may spend time considering a message even when only one produces the useful answer." (flaviocopes)
- FACT. "Grok Bot manages model selection, so there is no model picker." (docs.x.ai)

### 2.3 OpenClaw (earlier Clawdbot / Moltbot)

Docs read (raw files, pinned to commit b8ed2c32, plus current pages):

- Heartbeat doc at commit b8ed2c32: https://github.com/openclaw/openclaw/blob/b8ed2c32/docs/gateway/heartbeat.md
- Automation overview at the same commit: https://github.com/openclaw/openclaw/blob/b8ed2c32/docs/automation/index.md
- Inferred commitments at the same commit: https://github.com/openclaw/openclaw/blob/b8ed2c32/docs/concepts/commitments.md
- Standing orders: https://github.com/openclaw/openclaw/blob/b8ed2c32/docs/automation/standing-orders.md
- Newer heartbeat doc (search copy): https://docs2.openclaw.ai/gateway/heartbeat
- Heartbeat migration page (current): https://docs.openclaw.ai/gateway/heartbeat
- Automation page (current): https://docs.openclaw.ai/automation/cron-vs-heartbeat
- Prompt source: https://github.com/openclaw/openclaw/blob/main/src/auto-reply/heartbeat.ts

Important: the docs changed in 2026. The older doc describes a `heartbeat` config block. The current doc says heartbeats became ordinary "Automations" jobs. Both are useful. I describe the heartbeat design first, then the change.

#### Triggers

Heartbeat (interval):

- FACT. "Heartbeat runs **periodic agent turns** in the main session so the model can surface anything that needs attention without spamming you." (heartbeat.md)
- FACT. "Interval: `30m` (or `1h` when Anthropic OAuth/token auth is the detected auth mode ...). ... use `0m` to disable." (heartbeat.md)
- FACT. Prompt body: "Read HEARTBEAT.md if it exists (workspace context). Follow it strictly. Do not infer or repeat old tasks from prior chats. If nothing needs attention, reply HEARTBEAT_OK." (heartbeat.md)
- FACT. "The heartbeat prompt is sent **verbatim** as the user message." (heartbeat.md)
- FACT. The prompt purposes: "**Background tasks**: 'Consider outstanding tasks' nudges the agent to review follow-ups (inbox, calendar, reminders, queued work) and surface anything urgent." and "**Human check-in**: 'Checkup sometimes on your human during day time' nudges an occasional lightweight 'anything you need?' message, but avoids night-time spam by using your configured local timezone". (heartbeat.md)

Due-only task blocks inside the heartbeat file:

- FACT. "`HEARTBEAT.md` also supports a small structured `tasks:` block for interval-based checks inside heartbeat itself." Example: `inbox-triage` every `30m`, `calendar-scan` every `2h`. (heartbeat.md)
- FACT. "Only **due** tasks are included in the heartbeat prompt for that tick. If no tasks are due, the heartbeat is skipped entirely (`reason=no-tasks-due`) to avoid a wasted model call." (heartbeat.md)

Cron (exact time) versus heartbeat (approximate), older doc:

- FACT. Table: "Timing: Exact (cron expressions, one-shot) | Approximate (default every 30 min)". "Use Scheduled Tasks (Cron) when you need precise timing or isolated execution. Use Heartbeat when the work benefits from full session context and approximate timing is fine." (automation/index.md)
- FACT. Guide rows: "Send daily report at 9 AM sharp: Cron. Remind me in 20 minutes: Cron (one-shot `--at`). Check inbox every 30 min: Heartbeat. Monitor calendar for upcoming events: Heartbeat." (automation/index.md)

On-demand wake:

- FACT. "You can enqueue a system event and trigger an immediate heartbeat with: `openclaw system event --text "Check for urgent follow-ups" --mode now`. ... Use `--mode next-heartbeat` to wait for the next scheduled tick." (heartbeat.md)
- FACT. "Detached background tasks can enqueue a system event and wake heartbeat when the main session should notice something quickly." (heartbeat.md)

Self-scheduled follow-ups (inferred commitments), later removed:

- FACT. "When enabled, OpenClaw can notice that a conversation created a future check-in opportunity and remember to bring it back later." Off by default. "`commitments.maxPerDay` limits how many inferred follow-ups can be delivered per agent session in a rolling day. The default is `3`." (commitments.md)
- FACT. "OpenClaw never delivers an inferred commitment immediately after writing it. The due time is clamped to at least one heartbeat interval after the commitment is created." (commitments.md)
- FACT. "Commitment extraction uses an LLM pass, so enabling it adds background model usage after eligible turns." (commitments.md)
- FACT. "The inferred commitments experiment was removed in v2026.8.1: OpenClaw no longer extracts follow-ups from conversations or delivers them through heartbeat. ... For reminders or scheduled work, create an explicit automation." (https://docs.openclaw.ai/automation/cron-vs-heartbeat)
- INFERENCE. The page does not say why. The likely reasons are cost (an extra LLM pass after every reply) and surprise (the user did not ask for the reminder). Not confirmed.

Current design (2026 migration):

- FACT. "Heartbeat monitoring now uses ordinary automation jobs." (https://docs.openclaw.ai/gateway/heartbeat)
- FACT. "Explicit setup provisions one enabled Proactive check job for the ambient owner agent. It runs every 30 minutes unless the provider supplies a different initial cadence, uses the agent's main conversation, waits for idle execution, and delivers to a resolved owner DM." (same page)
- FACT. "A background process completing, a task becoming blocked, or a restart finishing uses ordinary session execution. It does not require an enabled recurring monitor." (same page)

#### Decision to speak

- FACT. "If nothing needs attention, reply with **`HEARTBEAT_OK`**." (heartbeat.md)
- FACT. "During heartbeat runs, OpenClaw treats `HEARTBEAT_OK` as an ack when it appears at the **start or end** of the reply. The token is stripped and the reply is dropped if the remaining content is **<= `ackMaxChars`** (default: 300)." (heartbeat.md; the source uses the symbol for "less than or equal")
- FACT. "For alerts, **do not** include `HEARTBEAT_OK`; return only the alert text." (heartbeat.md)
- FACT. Structured outcome: "Tool-capable heartbeat runs may instead call `heartbeat_respond` with `notify: false` for no visible update, or `notify: true` plus `notificationText` for an alert." (heartbeat.md)
- FACT. "A meaningful `heartbeat_respond` result with `notify: false` remains silent but is remembered as bounded internal context for the next user turn in that session." (https://docs2.openclaw.ai/gateway/heartbeat)
- FACT. Tool text in source: "Set notify=false when nothing needs the user's attention. Set notify=true with notificationText only when the user should be interrupted." (heartbeat.ts)
- FACT. Empty-file skip: "If `HEARTBEAT.md` exists but is effectively empty ..., OpenClaw skips the heartbeat run to save API calls." (heartbeat.md)
- FACT. Current prompt in source: "Follow the heartbeat monitor scratch context when provided. Recurring tasks are automations ... Do not infer or repeat old tasks from prior chats. If nothing needs attention, reply HEARTBEAT_OK." (docs2 page and heartbeat.ts)
- FACT. Example `HEARTBEAT.md` text: "Quick scan: anything urgent in inboxes? If it's daytime, do a lightweight check-in if nothing else is pending. If a task is blocked, write down _what is missing_ and ask Peter next time." (heartbeat.md)

#### Delivery

- FACT. "`target`: `last` ... deliver to the last used external channel. explicit channel ... `none` (default): run the heartbeat but **do not deliver** externally." (heartbeat.md). Current default: "Delivery target: `owner`. ... and never sends this route to a group." (docs2 page)
- FACT. "`directPolicy` ... `block`: suppress direct/DM delivery." (heartbeat.md)
- FACT. Visibility flags: "`showOk` ... `showAlerts` ... `useIndicator`". Defaults: OK hidden, alerts shown. "If **all three** are false, OpenClaw skips the heartbeat run entirely (no model call)." (heartbeat.md)
- FACT. "Control UI and WebChat history hide heartbeat prompts and OK-only acknowledgments. The underlying session transcript can still contain those turns for audit/replay." (heartbeat.md)
- FACT. Batching: "Heartbeat handles routine monitoring (inbox, calendar, notifications) in one batched turn every 30 minutes." (automation/index.md)
- FACT. A migration warning: "Heartbeat-specific duplicate suppression and no-route skipping were removed: repeated updates can be delivered." (https://docs.openclaw.ai/gateway/heartbeat)
- FACT. "Heartbeat-only replies do **not** keep the session alive." (heartbeat.md)

#### Quiet hours and busy guards

- FACT. "Active hours (`heartbeat.activeHours`) are checked in the configured timezone. Outside the window, heartbeats are skipped until the next tick inside the window." (heartbeat.md)
- FACT. `activeHours` has `start`, `end`, `timezone`. Example 09:00 to 22:00 `America/New_York`. "Do not set the same `start` and `end` time ... That is treated as a zero-width window, so heartbeats are always skipped." (heartbeat.md)
- FACT. "Heartbeats automatically defer while cron work is active or queued." and `skipWhenBusy`. (heartbeat.md)
- FACT. Current job policies: "`activeHours` restricts execution to a timezone-aware window. `idleOnly` gives foreground work priority ..." (docs.openclaw.ai/gateway/heartbeat)

#### User controls

- FACT. Standing note: the `HEARTBEAT.md` file. The doc says the agent can edit the file "if you ask it to". It warns: "Don't put secrets (API keys, phone numbers, private tokens) into `HEARTBEAT.md`" because the file becomes part of the prompt context. (heartbeat.md)
- FACT. Off switch: `every: "0m"`. Per-channel visibility. Per-agent heartbeats: "If any `agents.list[]` entry includes a `heartbeat` block, **only those agents** run heartbeats." (heartbeat.md)
- FACT. "Standing orders grant your agent **permanent operating authority** for defined programs." Each program has Scope, Triggers, Approval gates, Escalation rules. (standing-orders.md)

#### Safety

- FACT. Standing order example: "**Approval gate:** None for standard reports. Flag anomalies for human review." and a "What NOT to do" list ("Do not send reports to external parties"). (standing-orders.md)
- FACT. Commitment check-in turns: "due commitment heartbeat turns run without OpenClaw tools." (commitments.md)
- INFERENCE. OpenClaw leaves action limits to the user's config and standing orders. It has no built-in read-only mode for heartbeat turns in the pages I read.

#### Cost controls

- FACT. "Heartbeats run full agent turns. Shorter intervals burn more tokens. To reduce cost: Use `isolatedSession: true` to avoid sending full conversation history (~100K tokens down to ~2-5K per run). Use `lightContext: true` to limit bootstrap files to just `HEARTBEAT.md`. Set a cheaper `model` ... Keep `HEARTBEAT.md` small. Use `target: "none"` if you only want internal state updates." (heartbeat.md)
- FACT. "`timeoutSeconds` ... Maximum seconds allowed for a heartbeat agent turn before it is aborted" with default "the heartbeat cadence capped at 600 seconds". (heartbeat.md)
- FACT. "Recurring top-of-hour expressions are automatically staggered by up to 5 minutes to reduce load spikes." (cron-jobs.md at commit b8ed2c32)
- FACT. Retry: "Recurring retry: exponential backoff (30s to 60m) between retries. Backoff resets after the next successful run." (cron-jobs.md)

---

## 3. OpenAI dots (ChatGPT, 2026-09-29)

Primary pages:

- Launch: https://openai.com/index/introducing-dots/ (direct fetch gave 403; I read it through a second fetch tool)
- Overview: https://learn.chatgpt.com/docs/dots
- Tasks and memory: https://learn.chatgpt.com/docs/dots/tasks-and-memory
- Controls: https://learn.chatgpt.com/docs/dots/controls.md (search excerpt only)
- Help center FAQ: https://help.openai.com/en/articles/20001529-dots-privacy-security-and-safety-faqs (search excerpt)
- I did not read the system card or the dots safety blog.

### 3.1 Triggers

Self-scheduled wake and pause:

- FACT. "Your dot can decide when to pause and wake up to continue work; you don't need a fixed schedule for every follow-up." (https://learn.chatgpt.com/docs/dots)
- FACT. "Your dot works between conversations. It tracks progress, works out what needs to happen next, and follows through as things change." (same page)
- FACT. "You can also specify a time or a supported event that should trigger work. Tell it which results deserve an update and which decisions need your input." (tasks-and-memory)
- FACT (secondary, summary of the system card). The card describes "a time-budget setting combined with reasoning effort, tested with simulated budgets from four minutes to one year using a clock tool that can wait." (https://velofy.co/research/papers/dots-openai/). Check against the card before you cite it.
- INFERENCE. A dot has a "wait until" tool. The model picks the wake time. The user sees follow-ups under Activity and Scheduled.

Fixed schedules ("Scheduled"):

- FACT. "Work that repeats at fixed times needs a saved schedule." The user gives what to check, when (time zone and end date), "Which changes deserve a notification" and "Where to deliver results". (tasks-and-memory)
- FACT. Example prompt: "Check the connected planning channel each weekday at 9 AM Pacific for the next four weeks. Update the project checklist when a deadline changes. Message me in ChatGPT only if a deadline is at risk or you need a decision. Confirm the schedule." (tasks-and-memory)
- FACT. "Ask your dot to confirm what it saved. Use Scheduled to review recurring work, or ask your dot to list, change, or cancel its scheduled tasks." (tasks-and-memory)
- FACT. "Stopping active work and canceling a schedule are separate actions." (tasks-and-memory)

Event monitoring:

- FACT. "When a connected service supports event monitoring, you can instead ask your dot to respond to a specific event. ... Connecting Slack or another source alone doesn't create a monitoring task." (tasks-and-memory)
- FACT. "Tell it which channels and events to watch for. Adding it to a channel alone doesn't start monitoring." (learn.chatgpt.com/docs/dots)

Idle work ("proactive research"):

- FACT. "When you aren't actively working with it, your dot looks for ways to help in the background. We call this 'proactive research'. It does this by using the apps you've already connected with tools that are restricted to be read-only, which means that they can't send messages, change app content, or control your browser or computer." (https://openai.com/index/introducing-dots/)
- FACT. "The background agents report their findings to your dot, which can bring you a suggestion or question. Any follow-up action is subject to the permissions and approvals that apply to that work." (tasks-and-memory)
- FACT. Example in the docs: "your dot might flag that a release decision conflicts with a draft you shared last week." (tasks-and-memory)
- FACT. "Your dot ... keep[s] private notes to identify useful next steps." (learn.chatgpt.com/docs/dots)

Noticing things unasked:

- FACT. "an early tester's dot noticed he'd forgotten to invoice a publication, prepared the invoice, and sent it after his approval." (launch post)

### 3.2 Decision to speak

- FACT. "[Your dot] reaches out with results or decisions that need you." (learn.chatgpt.com/docs/dots)
- FACT. "Explain which changes need your attention, such as a deadline at risk." (same page)
- FACT. "Tell your dot what to keep current and which changes deserve your attention. It can decide when to follow up." (same page)
- INFERENCE. The threshold is user-defined text plus model judgment. OpenAI publishes no numeric rule.
- FACT. The first case: "it can update the headcount as people confirm and ask before sending a request for revised pricing." (same page). Update silently, ask before outward action.

### 3.3 Delivery

- FACT. "Tell your dot where to send different kinds of updates. For example, ask it to keep routine progress in ChatGPT and bring decisions to you in Slack." (learn.chatgpt.com/docs/dots)
- FACT. "They can also message you with progress, questions, or decisions that need you." Channels: ChatGPT, Slack, Teams, and texting (beta). (launch post)
- FACT. "Messages stay in their respective channels, while your dot can use relevant context across them. Before sharing information from a private conversation with other people, your dot checks that you've allowed it." (learn.chatgpt.com/docs/dots)
- FACT. No quiet hours or batching are documented.

### 3.4 User controls

- FACT. Pause, Activity, Take over, Return control (learn.chatgpt.com/docs/dots, controls section as summarized by the fetch tool). "Pause stops the current main task."
- FACT. "Custom Rules let you allow specific actions, require approval, or block them." (launch post)
- FACT (secondary). Rule levels in the mobile app: "Take action without asking. Take action if pre-approved. Ask before taking action. Hand off to you." (https://dotsbot.co/guides/dots-privacy-security-and-admin-controls)
- FACT. "Reset" and "Pause the dot" exist (secondary: https://www.agenttipsheet.com/agents/dots/safety/).
- FACT. No documented per-topic mute or frequency dial. Control is by instruction.

### 3.5 Safety

- FACT. "Dots start with built-in rules for when to act independently and when to ask for approval." (launch post)
- FACT. "Dots use auto-review to check actions that could affect your accounts or share information against your instructions, Custom Rules, and safety requirements." (launch post)
- FACT. "Certain sensitive tasks, such as changing a password, always stay with you." (launch post)
- FACT. "Asking your dot to draft replies doesn't give it permission to send them." (controls page, excerpt)
- FACT. "Approving one message does not give your dot ongoing permission to contact people on your behalf. Any advance approval remains limited to what you authorized." (help center, excerpt)
- FACT. "Custom Rules also cannot change the separate safety review system, Auto-review, or the restrictions on proactive research." (help center, excerpt)
- FACT. "If our monitoring system detects a safety concern, it can pause or stop the dot's work." (launch post)
- FACT (secondary). Auto-review and monitoring sit "in a separate control plane", and "a dot cannot use its sandbox access to change or disable those checks." (velofy teardown)

### 3.6 Cost controls

- FACT. "Conversations with your dot don't count toward your ChatGPT usage limits. When you ask your dot to start or manage tasks in Codex or ChatGPT Work, those tasks count toward your usage limits as usual." (launch post)
- FACT. "Your plan also includes an allowance for deeper work, with extended limits for the first month after launch." (launch post)
- INFERENCE. Idle research and wake timing are paid by OpenAI inside the plan. No per-user cadence control exists.

---

## 4. Side by side

| Topic | Claude Duet | Grok Bot | OpenClaw | OpenAI dots |
|---|---|---|---|---|
| Clock wake | Morning brief plus check-ins "every few hours" | Routines (user-made). Primary Bot "checks in on its own" | Heartbeat every 30 m (1 h on some auth). Cron for exact times | Model-chosen wake. Saved "Scheduled" tasks |
| Event wake | Task done or needs decision | Routine triggers: Slack, GitHub, Linear, Sentry, PagerDuty, email, webhook | `system event` wake. Background task completion wakes the session | Supported service events, user-defined |
| Self-scheduled follow-up | "follows up on threads that have gone quiet" | "nudge a stalled handoff" | Inferred commitments (removed). Explicit one-shot `--at` | "decide when to pause and wake up" |
| Idle work | Check-in drafts first steps | Primary Bot offers to take work | Not a named feature | Proactive research, read-only |
| Silence rule | Model judgment, "messages you only when something needs your attention" | "only come back when something needs your approval" | `HEARTBEAT_OK`, `NO_REPLY`, `notify:false` | Model judgment plus user text on what matters |
| Quiet hours | None documented | None documented | `activeHours` | None documented |
| Frequency control | "tell it ... how often" | Routine schedule | `every`, `0m` off | Instructions only |
| Delivery | One thread, Tasks tab, mobile push | Per-Bot notification, desktop first, phone after 3 m | Owner DM by default, `target`, per-channel flags | Route by update type: ChatGPT, Slack, Teams |
| Unattended side effects | Draft only, ask first | Routine sends wait as drafts, cards expire in 30 m | Config and standing orders | Idle research read-only. Auto-review on actions |
| Cost lever | Slow poll interval | Event over schedule, fewer runs | Isolated session, light context, cheaper model, skip empty | Plan allowance |

---

## 5. Best combination for Consort

### 5.1 Fit with the Consort model

Facts about Consort (from `docs/architecture.md`, `docs/protocol.md`, `docs/data-model.md` and `CONTEXT.md`):

- The hub wakes a coordinator only on a wake event. Events: user message, task change to `review`, `needs_input` or `failed`, decision answer, routine run, Tutti message, memory suggestion, card answer.
- One coordinator turn runs at a time in each channel. Other events wait.
- A coordinator speaks only with `post_message`, `reply` or `no_reply_needed`.
- A routine is "A prompt that the hub runs on a schedule in one channel". Its action is `coordinator_turn` or `start_task`. Schedules: `cron`, `interval` (at least 60 seconds), `once`. Coordinators can call `routine_create`, `routine_update`, `routine_delete`.
- A task that a coordinator starts gets "at most what its channel allows".
- Only the user moves a task to Resolved.
- Tutti sees every channel and owns global memory. Channel coordinators write only their own channel memory.

INFERENCE. Consort already has the hard parts: a wake queue, a silent tool, routines with `once`, and a channel permission cap on tasks. The missing parts are gates (cheap checks before a wake), limits (budgets and caps), a read-only idle mode, and a tuning surface. Most of the proposal below adds hub code and settings, not new agent skills.

Use Consort words: assistant, routine, coordinator, wake event. Do not call a routine a cron job or automation (see `CONTEXT.md`).

All numbers below are proposed defaults. They are my choices, not facts from the sources. Tune them with real use.

### 5.2 The 14 mechanisms

#### M1. Check-in routine (the heartbeat)

- What: A routine with action `coordinator_turn` that wakes the coordinator to look for things that need attention and stay silent otherwise.
- Trigger: `interval` schedule. Tutti has one by default. Other channels have none until the user adds one.
- Defaults: Tutti every 3 hours (Duet "every few hours"). Floor of 30 minutes for any check-in routine (OpenClaw default is 30 m, and Grok says 15 m is 96 runs a day). Active hours 08:00 to 21:00 in the user's time zone. Weekends: on. A new field `kind: check_in` marks it.
- Prompt: short and fixed, like OpenClaw's: read the check-in note, do not repeat old tasks from earlier chats, reply with `no_reply_needed` if nothing needs attention.
- Controls: change the interval, change active hours, pause, delete. Show it in the Routines list with a lock icon on the prompt (the user edits the note, not the prompt).
- From: Duet (cadence and the "visible routine" shape), OpenClaw (heartbeat, `activeHours`).

#### M2. Check-in note (standing checklist in memory)

- What: One global memory note, `Check-in`, that lists what to watch and what to skip. The check-in routine reads it. The user can edit it by chat or in the app.
- Trigger: read at every check-in turn.
- Defaults: starts with 3 lines. "Look at email and calendar. Skip newsletters, receipts and automated notices unless a payment failed. Ask before anything outward."
- Controls: user edits by chat ("skip receipts") and the Tutti coordinator writes the note (Tutti owns global memory, which fits the Consort rule). A blank note means skip the check-in, with no model call (OpenClaw empty-file skip). Max 2 KiB to avoid prompt bloat.
- From: OpenClaw (`HEARTBEAT.md`, empty skip), Duet and dots ("tell it what you want to hear about").

#### M3. Delta gate before the model wakes

- What: The hub does a cheap check first. It enqueues the `routine` wake event only when something changed since the last check. Otherwise it records a skipped run and spends no model tokens.
- Trigger: the schedule fires. The gate then asks each connector for changes since the last cursor (new mail count, calendar events changed, tasks in `review`, failed tasks).
- Defaults: on for check-ins. Off for `cron` routines that the user wrote (those always run, like a morning brief).
- Controls: per-routine switch `only_if_changed`. The routine run row shows `skipped: no_change`.
- From: OpenClaw (`no-tasks-due`, empty-file skip), Grok ("use an event trigger when one exists ... A scheduled routine wakes up even when there is nothing to do"), Duet's cost reasoning.
- INFERENCE. This is the largest cost win, because most check-ins find nothing.

#### M4. Silent by default, with a silent finding

- What: Extend `no_reply_needed` with an optional `finding` text. A finding stays silent and enters the next turn's context, like OpenClaw `heartbeat_respond notify:false`. A speak decision needs a reason and a tier (M5).
- Trigger: any proactive wake (check-in, stale-task, follow-up, idle research).
- Defaults: a turn that ends with no tool call and only text, on a proactive wake, is dropped, not posted. (This differs from the current rule that posts the last assistant text. Keep the old rule for user-message wakes.)
- Controls: the user can open "Proactivity log" (M13) to see silent turns.
- From: OpenClaw (`HEARTBEAT_OK`, `ackMaxChars`, `notify:false` with bounded context), Duet (silence).

#### M5. Three tiers for what may speak

- What: Every proactive message has a tier chosen by the coordinator and checked by the hub.
  - Tier 1, interrupt: needs a decision or approval, a task `failed`, a deadline inside 24 hours, or a worker needed by a routine is offline for more than a set time. Sends a push.
  - Tier 2, digest: useful but can wait. Goes into the next brief or the 18:00 digest. No push.
  - Tier 3, silent: logged only (M4).
- Trigger: coordinator calls `post_message` with `tier`. Without a tier, on a proactive wake, the hub treats it as tier 2.
- Defaults: at most 3 tier-1 pushes per day across Consort, except approvals the user is waiting on. At most 1 digest per day plus the morning brief. (The 3 comes from OpenClaw `commitments.maxPerDay`.)
- Controls: user sets caps and digest time. "Interrupt me only for X" goes into the Check-in note.
- From: botsinproduction rule (interrupt only for "a click" or "broke"), dots (route updates by type, "deadline at risk"), OpenClaw (cap per day), Grok (separate schedule and notification policy).

#### M6. Morning brief routine in Tutti

- What: A `cron` routine in Tutti. It reads every channel, calendar and mail, then posts one ranked list. Each item has a status, a source link and a prepared draft or a Yes / Not now card.
- Trigger: `cron` weekdays 07:30 local.
- Defaults: top 5 items. Groups: "Needs your decision", "Ready for review", "Done". Link every claim to its source. If nothing needs attention, send nothing. (Duet always sends one. Reviewers found a thin brief dull, so skip empty days by default and offer `send_when_empty`.)
- Controls: time, days, count, on/off. Edit the "skip" rules in the Check-in note.
- From: Duet (brief, ranked items, drafts), the third-party brief guide (status split), Grok ("Automate preparation before execution").
- Note: Consort already lists the morning brief as a Tutti routine. This item adds the empty-day rule, the cap and the source links.

#### M7. Stale-task follow-up (hub timer, not a model poll)

- What: The hub watches tasks. When a task sits in `needs_input` or `review` too long, it adds a `stale_task` wake event to the owning channel's queue. The coordinator decides whether to nudge.
- Trigger: hub clock check. No model wake until a task crosses the line.
- Defaults: `needs_input` after 24 hours, `review` after 72 hours. One nudge per task. After that, the task goes into the digest (tier 2) and gets no more pushes. A task never moves to Resolved on its own (Consort already follows Duet here).
- Controls: per-channel thresholds. "Snooze this task" button.
- From: Duet ("follows up on threads that have gone quiet"), Grok ("nudge a stalled handoff", "nudges a stalled Bot once, stays quiet while work is moving").

#### M8. Self-scheduled follow-up tool

- What: The coordinator can ask to be woken later: `wake_me(after | at, reason, key)`. It creates a `once` routine marked `origin: assistant`, `kind: follow_up`. Same key replaces the old one.
- Trigger: the coordinator calls the tool during a turn. Example: "Check back tomorrow after the vendor replies."
- Defaults: minimum delay 15 minutes. Maximum 30 days. At most 20 pending per channel. A follow-up that fires and has nothing to do ends with `no_reply_needed` and deletes itself. The wake text includes the reason the assistant wrote.
- Controls: follow-ups show in the Routines list under "Follow-ups". User can cancel one or all.
- From: dots ("decide when to pause and wake up"), OpenClaw (explicit `--at` one-shots). Use an explicit tool. Do not add a hidden pass that infers commitments (OpenClaw removed that).
- INFERENCE. `routine_create` with `once` already exists. This is a safer, bounded wrapper.

#### M9. Idle research pass (read-only)

- What: A low-frequency coordinator turn with a hub-enforced read-only tool set. It reads connectors, channels and memory. It writes private notes and may produce suggestion cards. It cannot send messages, create tasks with side effects, or change connectors.
- Trigger: `cron` once a day in Tutti, outside active use (for example 03:00 local), plus an optional run after a new connector is added.
- Defaults: off at install. When on: 1 run a day, at most 3 suggestions a day. The hub gives the session no write tools, so a prompt cannot change it. Suggestions use a card with `Do it`, `Not now`, `Never about this`.
- Controls: on/off, time, max suggestions. "Never about this" writes a skip rule into the Check-in note.
- From: dots (proactive research: read-only, notes, then "a suggestion or question"), Grok (Primary Bot "offers", it does not act).
- INFERENCE. Enforce read-only in the hub, not in the prompt. Dots do it "in code".

#### M10. Offer, then act (draft first)

- What: A proactive turn may start a task only when the task is read-only or draft-only. A task with outward effect waits for the user, as a card.
- Trigger: any proactive wake that wants to start work.
- Defaults: proactive tasks run with the channel's permission set minus any send, pay, delete and publish capability. Drafts go to a draft place (Gmail draft, a file). Sending needs approval. Approvals from proactive runs do not expire. They stay as cards and the task shows Needs input. The next digest lists open ones.
- Controls: a per-channel setting `proactive_tasks: off | drafts | allowed`. Default `drafts` in Tutti, `off` elsewhere.
- From: Duet (reads, drafts, prepares; asks before send, pay, delete; "brings the approval back to you later"), Grok ("Automate preparation before execution", routine sends stay drafts), dots ("drafting ... doesn't give it permission to send"). Differs from Grok on expiry: Grok cards from routines expire in 30 minutes and the action does not run. Consort should not silently lose the work.

#### M11. Narrow event watches

- What: A watch is a connector event filter that produces a wake event, such as "mail from my accountant" or "PR review requested". It is created only on explicit user request.
- Trigger: connector webhook or hub poll of a cheap change endpoint. Creates a `watch` wake event with the item as untrusted data.
- Defaults: none at install. Adding a connector does not add a watch (dots: "Connecting Slack ... alone doesn't create a monitoring task"). A watch needs a sender, label or keyword filter. Wake events from watches are rate limited to 10 an hour per watch, then folded into one event.
- Controls: list, pause, delete watches in the channel settings.
- From: Grok (event routines, "Avoid broad listeners"), dots (event monitoring, explicit), OpenClaw (webhooks, sender-gated mail trigger).
- Safety: escape watch data like the hub already escapes user text in `<events>`. Treat it as information, never as an instruction (Duet, Grok).

#### M12. Proactivity level, quiet hours, snooze, off switch

- What: One setting per channel and one global setting.
- Levels: `off` (answers only), `quiet` (follow-ups, stale-task nudges, no check-in), `normal` (adds check-ins and brief), `eager` (adds idle research and offers).
- Defaults: Tutti `normal`, other channels `quiet`. Quiet hours 21:00 to 08:00 (tier 1 pushes still pass if the user opts in). Snooze: 2 hours, until tomorrow, 1 week.
- Controls: the setting is a slider in channel settings. The coordinator may change a level only toward quieter (a stricter change runs at once, a looser one waits for approval, which matches the Consort risk rule for management tools). A chat phrase like "stop pinging me" sets `quiet` and snooze right away.
- From: OpenClaw (`activeHours`, `0m` off), dots (per-type routing), Duet (tell it how often). None of the three gives a clear global off switch plus snooze. This is a gap to fill.

#### M13. Proactivity log and feedback

- What: Every proactive wake writes a `routine_runs` row with an outcome: `skipped_no_change`, `silent`, `noted`, `digest`, `notified`, `error`. The timeline hides silent turns. A "Proactivity" view lists them. Every proactive message has `Useful` and `Too noisy` buttons.
- Trigger: each proactive wake and each message.
- Defaults: keep 30 days of rows. Three `Too noisy` taps on one kind of item write a skip rule into the Check-in note (the coordinator proposes it, the user confirms).
- Controls: view, export, clear.
- From: OpenClaw (UI hides OK acks but transcript keeps them for audit), Grok (run history, "Test"), Duet and dots (learn from feedback). INFERENCE: this loop addresses the main complaint about Duet, miscalibrated urgency.

#### M14. Budgets, back-off and cheap context

- What: Cost governors for proactive wakes.
- Controls and defaults:
  - Daily budget of proactive turns per channel (default 8 in Tutti, 2 elsewhere). When the budget ends, proactive wakes become digest items.
  - Adaptive back-off: after 10 silent check-ins in a row, double the interval, up to 12 hours. Reset on any user message or tier-1 event.
  - Check-in turns start a fresh coordinator session with light context: the Check-in note, global memory, a hub-made summary of changes. They do not load the channel's long history. (OpenClaw: about 100K tokens down to 2K to 5K.)
  - A separate model selection for check-ins, default a cheaper model. Consort already has a per-channel coordinator model selection, so add one for `kind: check_in`.
  - Defer a proactive wake while a turn runs (Consort already runs one turn at a time) and while the needed worker is offline. Do not retry in a loop. Use the existing coordinator start back-off (10 s up to 10 min).
  - Show "proactive turns today" and an estimated token count in settings.
  - Stagger routines that share the same minute by up to 5 minutes (OpenClaw).
  - For Tutti, one proactive pass reads other channels through `read_channel`. Do not wake every channel coordinator for each event. Wake only the channel that owns the event (Grok group chats wake every member and waste turns).
- From: OpenClaw (cost list, stagger, back-off), Duet (poll slowly), Grok (96 runs a day, one-shot cleanup, group fan-out cost).

### 5.3 How the mechanisms fit together

1. A schedule fires (M1, M6, M9) or the hub clock fires (M7) or a watch fires (M11) or a follow-up fires (M8).
2. The hub gate decides if a model wake is worth the cost (M3, M12, M14).
3. The coordinator reads the Check-in note (M2) and replies in one of three tiers (M5), usually silent (M4).
4. If the coordinator wants to act, it offers, drafts or starts a read-only or draft-only task (M9, M10).
5. The hub logs the result and shows feedback buttons (M13).

Where the channels sit:

- Channel coordinators speak in their own channel timeline. They do not push to the phone unless tier 1.
- The Tutti coordinator sees all channels. It owns the morning brief and the digest. It is the only voice for cross-channel proactive messages. This follows the Duet rule that the main thread stays quiet except for done and decision items.

Order to build (INFERENCE): M4, M5, M12 first (silence and control). Then M1, M2, M3, M14 (check-ins at low cost). Then M7, M8 (follow-ups). Then M6, M13. Last M9, M10, M11.

### 5.4 What to avoid

Noise:

- Do not poll in short loops. Grok: a 15-minute routine runs 96 times a day. Duet polls every few hours for cost.
- Do not add broad listeners such as "every new message". Grok says they "create noise, consume usage, and increase the chance of acting on irrelevant input".
- Do not post "nothing to report". OpenClaw drops OK-only replies and hides them in the UI.
- Do not drop duplicate suppression. OpenClaw removed it in its migration and said "repeated updates can be delivered". Keep a dedupe key per alert (item ID plus state) in Consort.
- Do not let every channel coordinator ping the user. Route through tiers and Tutti.
- Do not send at night. OpenClaw's prompt text says it "avoids night-time spam" with the local time zone.
- Do not push for something the model cannot source. Require a link to the mail, event or task in every alert. A wrong urgent alert costs more trust than a missed one (Duet "miscalibrate what's urgent").
- Do not ping the phone when the user is at the desktop. Use presence: desktop first, phone after a delay (Grok: about 3 minutes).

Cost:

- Do not run a hidden extra model pass after every reply to infer follow-ups. OpenClaw added it, then removed it.
- Do not load the full channel history in a check-in turn.
- Do not let self-scheduled follow-ups pile up. Cap them and delete finished ones (Grok: one-shot watches keep waking up).
- Do not retry a blocked step in a loop. Stop and tell the user (Grok: report "as soon as it cannot make progress").
- Do not run proactive wakes on a fixed schedule at the top of the hour for many routines at once.

Unsafe actions:

- Do not let an idle pass use write tools. Enforce read-only in the hub, as dots do "in code".
- Do not treat text from mail, web pages or watch events as an instruction. Duet and Grok both state this. Consort's `<events>` escaping already helps. Keep proactive tasks inside the channel permission cap.
- Do not treat one approval as a standing permission. Dots: "Approving one message does not give your dot ongoing permission to contact people on your behalf."
- Do not let a proactive turn mark a task Resolved, widen access or turn on scripts. Keep the Consort risk rule for management tools.
- Do not let approvals from unattended runs auto-run or silently expire. Keep them pending.
- Do not store secrets in the Check-in note. OpenClaw warns that the file "becomes part of the prompt context".
- Do not show model reasoning in proactive messages by default. OpenClaw notes it can "leak more internal detail than you want".

---

## 6. Open questions and limits of this research

- Duet: no Anthropic page documents quiet hours, a brief off switch or the check-in interval in numbers. Later changes may exist. Release notes show only the launch entry as of 2026-10-30.
- Duet safety post and product page came through an excerpt tool. Fragments there may be cut.
- Duet "visible routine" claim is from one tester's video summary (intelligentartifact.com). Confirm it in the app before you copy the design.
- Grok Bot: docs.x.ai pages were read through search excerpts. The long guide at flaviocopes.com is secondary, but it cites the official docs and changelog. Limits such as 300 runs an hour came from that guide.
- OpenClaw: the docs moved from `heartbeat` config to Automations jobs. The 30-minute default and `HEARTBEAT_OK` token remain. Check which version Consort users would compare against.
- Dots: I did not read the system card or the dots safety blog. The "time budget from four minutes to one year" claim is from a secondary teardown.
- Reviews are few and early. Two reviewers (Vo, Brown) point to latency and unclear notifications as the main reasons they stopped using Duet. INFERENCE: delivery reliability matters as much as judgment. Test push delivery early in Consort.
