# Design notes: voice

These are the Paper designs for the screens in the "Screens" section of [VOICE-PLAN.md](../VOICE-PLAN.md). The Paper file is `01M6342D512B1CKH0D29HMDN8V`, page Desktop. The artboards are in one row at worldY 12552, from worldX -720, with the same 80 px gap as the earlier rows. The proactivity row is at worldY 10320, and "Settings, Proactivity, dark" is 2152 px high, so it ends at 12472.

The example data is the same in all four artboards. The user is Chris. The mail account is chris@example.com and the Slack workspace is Work. Consort learned on Oct 30 from 214 sent mails and 380 Slack messages. Dana is the contact in the approval card. The numbers are example data. The plan sets no real counts.

## Artboards

| Artboard | ID | What it shows |
|---|---|---|
| Settings, Voice, off, dark | `H3V-0` | The settings rail with a new "Voice" item in the Assistant group, after Memory. One 760 px column. The switch "Write in my voice" is off, with the helper text from the plan. Under it, "Where Consort learns from": Gmail (chris@example.com, "Sent mail, last 90 days"), Slack with "Public channels" ticked and "Direct messages" not ticked, "Documents you pick" with Add, and "Your edits to drafts". Then "What the voice never changes" (wording only, approval, where it applies) and the accent button "Turn on and learn". The artboard is 1040 px high. |
| Settings, Voice, on, dark | `H7T-0` | The switch is on. The row "Learned from 214 sent mails and 380 Slack messages on Oct 30" has "Change sources" and "Learn again". The style guide has Core rules, four contexts (Formal email, Close email, Chat, Public posts) with their rules and 2 to 3 examples each, and Hard rules. "Edit as markdown" sits at the right of the section title. Remove shows on one rule and on one example, as the hover state. Then "Try it" (the field "Reply to Dana, say yes", "Write a sample", a sample draft, "Nothing is sent"), Disclosure (three radio options), and "Turn off the voice" and "Reset the voice". The artboard is 2980 px high, because the page scrolls in the app. |
| Voice setup offer and learning, dark | `HL7-0` | The Tutti timeline. First the task card of the voice task "Learn your writing voice" with the status "Ready for review", its worker, three finished steps and "Open thread". Then the offer card "Save this style guide?" with a short summary, "Open the full guide", "Do it", "Not now" and "Never about this". |
| Approval card in your voice, dark | `HTB-0` | A task thread panel (400 px) in three states, side by side. Review: the "Send email" card for Gmail with To, Subject and Body, the line "Written in your voice (Formal email)", two flags, the unticked box "I checked the marked lines", Approve off, Edit, Deny and "Not how I write". Checked: the box is ticked and Approve is on. Edit: Subject and Body are fields, and the buttons are "Save changes" and "Cancel". |

## Decisions the plan did not settle

- The icon of the Voice item is `quote`, from the hand-drawn set. The `voice` icon is a microphone. It already means dictation in the composer, and it would read as "speak" here. The item sits after Memory, because the style guide is a memory note. Routines and Proactivity follow.
- The plan says the marked lines use `status.waiting`. The design system has no such role. The statuses are `working`, `done`, `failed` and `scheduled`. A line that waits for your check is "your turn", so the design uses `accent.text`, the color of Needs input. This is the only difference from the plan.
- A flag has two marks. The phrase in the body is underlined in `accent.text`. Under the body, each flag has a 2 px bar in `accent.text`, the phrase, its kind in `text.tertiary` ("Date", "Agreement"), and the label "Not in your request. Check it." The label repeats on each flag, so a flag can be read alone. Price, time, weekday and hard-rule flags use the same layout with their own kind word.
- Approve is off with `surface.selected` and `text.tertiary`, not with a low-opacity accent. The accent stays for the one button that can answer. The other connector cards say "Allow once". The plan says "Approve" for this card, so the design uses "Approve".
- "Not how I write" is a quiet text in `text.tertiary`, at the right end of the button row. It is not a button, because it does not answer the approval.
- Edit changes the card in place. Subject and Body become fields, and Body has the `accent.text` focus line. To stays text, because the plan lists only subject, body, text and message as editable. The flags and the checkbox leave while the user edits. The hub scans the text again when the user presses "Save changes", and the card returns to the review state with new flags.
- The three card states show side by side in 400 px panels, on `surface.sidebar`. The task thread panel is 400 px wide in the app, so the body wraps as it will in the build.
- The off page shows the sources before the user turns anything on. Public channels are ticked and Direct messages are not, as the plan sets the default. "Turn on and learn" is the one way to start. The switch and the button do the same thing: the switch only turns on and starts learning, and does not turn on with no sources.
- "What the voice never changes" sits on the off page, above the button. The plan says consent is per source. The three rules (wording only, approval, where it applies) are what the user agrees to, so they are visible before the click.
- The plan has "Edits to drafts" as a source, on by default. The page shows it as a fourth row with a checkbox, and no tile color.
- A service tile is a monogram (G, S) on `surface.selected`, as the design system says until a logo loads. "Documents you pick" and "Your edits to drafts" use the `file` and `edit` icons in the same tile, so the four rows share one lane.
- The on page adds "Change sources" next to "Learn again". The plan lists "Learn again" only, and the user needs a way to change the sources after the first run.
- The style guide is one card for each section, with the section name and its source count in the header ("Learned from 96 mails"). The count shows that a context has enough text (10 messages at least). A rule is one row with a small dot. An example is a block on `surface.card`, with the names replaced by placeholders such as {name}, {day} and {link}.
- Remove shows on hover, in a fixed 84 px slot at the right of each rule and example, so the text does not move. The design shows the hover on "No exclamation marks." and on the second Formal email example.
- "(you)" is in `accent.text`, at the end of a rule that the user wrote. Both marked rules are in Hard rules. The third hard rule ("Do not use emoji in mail.") has no marker, so the page shows the difference. The helper line of the section says that a learned change never replaces a "(you)" rule.
- Public posts need a source. The plan names the context `post.public`, but none of its sources holds social posts. The design learns it from 38 Slack messages in public channels, and calls the context "Announcements and social posts". See the questions.
- "Try it" has one field and one button, then the sample draft under them. The caption of the sample says "Written in your voice (Formal email)", the same line as on the approval card, so the user sees what the card will say. "Nothing is sent. Consort does not save this sample." has a `status.done` dot, as the system line in Notifications has.
- Disclosure is three radio options with one line each. The first option is chosen. The helper text says that the line is part of the draft text, so the user sees it on the approval card.
- "Turn off the voice" is a secondary button. "Reset the voice" is a secondary button with its text in `status.failed`, not a red fill. Reset deletes the note, so the helper text says that the old version stays for 30 days.
- The voice task card uses the status "Ready for review", because the task returned a proposal and only the user can resolve it. It shows three steps with check marks in `status.done`. The last line says that the raw text stays in the task. The mark at the top has no activity pill, because the task no longer works.
- The offer card follows the offer card of the proactivity design: the mark "Offer" with the `compose` icon, "Do it" in the accent, "Not now" with a border, "Never about this" as text. The summary says what it learned in two sentences, and "Open the full guide" is a link with the `external-link` icon. The card says "I save nothing until you say Do it", as the plan's memory rule requires.
- Copy uses "they" for people. Dana appears only as a name in the examples.

## Conflicts in the plan that the design shows

1. **`status.waiting` does not exist.** See the decisions. The build needs either a new role or `accent.text`.
2. **"Approve" and "Allow once".** The connector approval card in the thread says "Allow once", "Allow for this task" and "Deny". The voice plan says "Approve" and "Edit". A send or draft tool card with the voice on needs one set of words. The design uses "Approve", "Edit", "Deny" for this card only.
3. **The plan lists the contexts, but only two sources feed them.** Mail gives Formal email and Close email. Slack gives Chat. Nothing gives Public posts. With no social source, the context stays empty, and the guide says so.
4. **The switch has two meanings.** The plan says "turning the switch on (or Learn again) starts a voice task". If the user turns the switch on with no source ticked, nothing can learn. The design makes "Turn on and learn" the one start, and keeps the button off when no source is ticked. This is not drawn.
5. **A second scan after Edit.** The plan says the hub scans the draft. It does not say whether it scans the edited text. The design scans again on "Save changes".

## Questions for the user

1. Should the marked lines use `accent.text`, or do you want a new `status.waiting` role in the themes?
2. Should the voice card say "Approve" and "Edit", or keep "Allow once" for every connector card and add only "Edit"?
3. Where should Public posts come from? Options: Slack public channels (as drawn), a list of social post links that the user adds, or the context stays empty until the user adds documents.
4. Should the page also show a "learning" state (the task runs, with the sources and a progress line), or is the task card in Tutti enough?
5. Should "Reset the voice" ask first in a small confirm dialog, as Delete does in the Channel panel? The design shows no dialog.
6. Should the user be able to turn a single context off (for example, no voice in chat)? The plan has only one switch.
7. Should "Not how I write" ask which part is wrong in the same card, or only in the thread, as the plan says the coordinator asks?
8. After "Save changes" in Edit, should the card keep the ticked box from before? The design clears it, because the text changed.

## Answers (by the orchestrator on 2026-10-30, while the user was away)

These answers are final for the build. The user can change them later.

1. The marked lines use `accent.text`. No new theme role.
2. Keep the connector card words: "Allow once", "Allow for this task" and "Deny", and add "Edit". One set of words for every connector card is clearer than a special card. A draft with marked lines keeps every allow button off until the box is ticked.
3. Public posts has no source in the first build. The context stays empty until the user adds documents, and the guide says so. Slack public channels feed Chat only.
4. The page shows a short learning state: "Learning from your sources" with the task link. It is drawn from the task's status; no new screen is needed.
5. Reset asks first in a small confirm dialog, the same as other deletes.
6. One switch for now. A context with no rules is not used. The user can remove the rules of a context.
7. "Not how I write" only records the feedback on the card. The coordinator asks what is wrong in the thread.
8. After "Save changes" in Edit, the hub scans the new text again and the box is cleared.
