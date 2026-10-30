# Design notes: proactivity

These are the Paper designs for the screens in the "Screens" section of [PROACTIVITY-PLAN.md](../PROACTIVITY-PLAN.md). The Paper file is `01M6342D512B1CKH0D29HMDN8V`, page Desktop. The artboards are in one row at worldY 10320, from worldX -720, with the same 80 px gap as the earlier rows. The plan says worldY 9600, but the row at worldY 9340 (the new channel screens) ends at 10240, so 9600 would overlap it.

The example data is the same in all four artboards: it is Thursday, about 11:50 AM. Tutti is Eager, house-lights is Normal, sayso, miss-lucy-tg and reading are Quiet, homelab is Off. Snooze is on for 2 hours. Quiet hours are 21:00 to 06:00 in America/Chicago.

## Artboards

| Artboard | ID | What it shows |
|---|---|---|
| Settings, Proactivity, dark | `FYE-0` | The settings rail with a new "Proactivity" item (last, after Routines). One 760 px column: Pause all, Quiet hours (from, to, time zone), Snooze (Off, 2 hours, Until tomorrow, A week, with the current state), and "Cancel all follow-ups". Then Each channel (a compact select with Off, Quiet, Normal or Eager, and the one-line meaning of the level), Today (proactive turns, notifications out of 3 with a bar, "What to watch" with "Edit check-in.md", Morning brief with "Open routine"), and Activity (the log with a channel filter and a kind filter). The artboard is 2000 px high, because the page scrolls in the app. |
| Proactive messages and offers, dark | `G2C-0` | The Tutti timeline with four marked messages. An offer card with "Want me to draft a reply to Dana?" and "Do it", "Not now", "Never about this". The morning brief with three groups and five ranked items. A check-in with the hover actions "Useful" and "Too noisy". A follow-up with its reason line. The artboard is 1070 px high, because four messages do not fit in 900. |
| Channel panel, Routines with follow-ups and a check-in, dark | `G8V-0` | The Routines tab of house-lights. The Check-in row (every 6 hours, "Uses check-in.md", "Stayed silent at 7:25", next run) in its hover state, with "Run now", "Run history" and "Open note", and no edit. Two normal routines. A "Follow-ups" section with two follow-ups that the assistant set, "Cancel" on hover, and "Cancel all". |
| Channel settings, Proactivity row, dark | `GHK-0` | The channel settings dialog of sayso with a new "Proactivity" select in the Assistant section (Quiet, "Default for this channel") and its helper text. The dialog grows by 41 px. |

## Decisions the plan did not settle

- The icon of the Proactivity item is `eye`, from the hand-drawn set. The assistant keeps watch, and the same icon marks a check-in in the timeline and in the Routines tab. The bell is taken by Notifications, and the metronome by Routines.
- The icons of the other marks are `sun` (Morning brief), `eye` (Check-in), `clock` (Follow-up) and `compose` (Offer, because an offer is mostly a draft). The icon paths are the set's own paths, thinned to straight segments with round joins. At 14 px the difference does not show.
- Settings > Proactivity has four parts in this order: when the assistant may start things, each channel, today, activity. The switch and the time fields come first, because "Pause all" is the answer to "stop pinging me".
- "Pause all" is a switch in the same style as Notifications. It is off in the design, so the other rows stay in their normal state.
- Quiet hours, snooze and "Cancel all follow-ups" share one bordered group. "Cancel all follow-ups" sits there because the plan puts it in Settings > Proactivity. It is a secondary button, as "Back up now" is.
- Each channel shows the meaning of its own level, not a legend. The user reads the effect of the level that is set. The select is compact (112 px) and has no color.
- The notification count uses `status.working` for the bar, as other progress bars do. A count of 3 of 3 is not an error, so it stays in this color.
- "Today" has no chart. It has two numbers and one bar.
- The log row has four lanes: time, channel and kind, result, finding. The result is in words. `Notified` is in `accent.text`, because it asked for you. `Failed` is in `status.failed`. `Posted` is in `text.primary`. All skipped and silent results are in `text.tertiary`. The finding is one line of `text.secondary`, and a link ("Open message") when the wake posted a message.
- The log groups rows under "Today" and "Yesterday". The filters are two compact selects at the right of the section title.
- A proactive mark is a caption in Label size and `text.tertiary` with an icon, above the first bubble of the group, on the left edge of the bubble. The caption is not a chip and has no color, so the message itself stays the main thing.
- The reason line of a follow-up sits under its mark ("You asked me to check the vendor reply today."), in Label size, so it reads as part of the mark and not as part of the message.
- The morning brief uses the group names "Needs you", "Ready for review" and "Done since yesterday" as small labels. The first two use `accent.text`, because both are your turn. The third uses `text.tertiary`. The numbers run through all three groups, so the rank is clear. The task name is the link.
- The offer card is a card with buttons, not a list of option rows. The three options have no extra text, so rows would be empty. "Do it" is the accent button, "Not now" is the bordered button, and "Never about this" is the quiet text button. This follows the approval card.
- The offer card says what it will do and what it will not do ("I will only write a draft. I will not send it."), because the plan says a draft never gives permission to send.
- The feedback actions "Useful" and "Too noisy" show under the bubble on hover, in Label size, with no border. The design shows the hover state on "Useful".
- The check-in row in the Routines tab shows `check-in.md` in mono, as note names are mono elsewhere. It has no "Edit" action. The hover actions are "Run now", "Run history" and "Open note". "Open note" opens `check-in.md` in Memory.
- A follow-up row shows its reason as the title and the time as the second line ("Today at 15:00, set this morning"). It has no switch, because a follow-up runs once and a switch would mean it can be paused. It can only be cancelled. The Routines tab uses 24-hour times, so follow-ups do too.
- The Routines tab count (5) counts the check-in, the routines and the follow-ups. The toolbar line counts only the check-in and the routines ("3 routines, 3 on").
- The channel settings select shows the level and the default for this channel. The helper text says what the level does in this channel and that only the user can raise it.
- Copy uses "they" for Dana, as the style rules ask.

## Conflicts in the plan that the design shows

1. **The morning brief and the research pass are inside the default quiet hours.** The plan sets quiet hours to 21:00 to 08:00, the brief to 07:30, and the research pass to 06:30. With the default hours, both would always be skipped or silent. The design uses 21:00 to 06:00 for the user's setting, so both run. The default needs a decision.
2. **A snooze can swallow the next brief.** "Until tomorrow" ends at 08:00 in the plan, and the brief is at 07:30. The plan says the brief may notify once. It does not say whether the brief runs while snoozed.
3. **Budget per channel and "Skipped: over the daily limit".** The plan has a budget per channel (8 in Tutti, 3 elsewhere) and a notification cap across the hub (3). The log shows the first as "Skipped: over the daily limit" and the page shows the second as "2 of 3 a day". The two names are close. They need two names in the product, for example "Over the turn limit" and "Over the notification limit".
4. **The plan has outcomes that the user brief does not list:** `skipped_level` and `skipped_busy`. The log design has no row for them. A wake that the level stops does not need a row, but `skipped_busy` may.

## Questions for the user

1. Should the default quiet hours change to 22:00 to 06:00, or should the brief and the research pass move to 08:00 and 07:00?
2. Does the brief run while a snooze is on?
3. Should the log show a row for a wake that the channel level stops (`skipped_level`)? It is noise for a channel that is Off.
4. Should "Pause all" also stop follow-ups, or only new wakes? The design says it stops everything that starts on its own, and the snooze text says follow-ups still run.
5. Should "Too noisy" ask for a reason, or only record a rating and let the assistant answer in one line?
6. Should the Activity log open a message in place, or open the channel at that message? The design says "Open message" and leaves it open.
7. Should a channel with level Off still show its meaning line, or show "The assistant only answers here"?

## Answers (by the orchestrator on 2026-10-30, while the user was away)

The plan was changed to settle the conflicts. These answers are final for the build. The user can change them later.

1. Default quiet hours are 21:00 to 07:00. The brief stays at 07:30, outside quiet hours. The research pass stays at 06:30. It runs in quiet hours because it never posts or notifies.
2. The brief runs while a snooze is on, but it cannot notify. Follow-ups do the same.
3. A wake that the channel level stops gets no log row.
4. "Pause all" stops everything that starts on its own: check-ins, nudges, the research pass, the brief and follow-ups. Follow-ups that come due while paused are skipped and logged. Snooze is the softer control.
5. "Too noisy" asks for no reason. The assistant answers in one line in the thread.
6. "Open message" opens the channel at that message, or the thread when the message is in a thread.
7. A channel at Off shows "The assistant only answers here."
8. The two limits get two names: "Skipped: over the turn limit" (the daily budget of proactive turns per channel) and "Over the notification limit" (the notify cap across the hub). `skipped_busy` gets a row: "Skipped: a turn was running".
