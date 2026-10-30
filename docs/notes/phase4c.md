# Phase 4c: write in my voice

These notes are for the merge into `docs/DECISIONS.md`. The plan is [VOICE-PLAN.md](../VOICE-PLAN.md) with [ADR 0014](../adr/0014-the-assistant-writes-in-the-users-voice-with-consent.md). The designs are in [design-voice.md](design-voice.md) (`H3V-0`, `H7T-0`, `HL7-0`, `HTB-0`, and its "Answers", which are final). Written on 2026-10-30. One build did the proto, the hub, the client, the desktop app, the theme and the docs.

## What was built

### Proto (`crates/consort-proto`)

- **New file `model/voice.rs`.** `VoiceSettings` (enabled, sources, learn from edits, disclosure, last learned, learned summary, the voice task, the offer that waits, the sample number and state), `VoiceSource` with `VoiceSourceKind` (mail, chat, documents; `for_service` maps Gmail and Outlook to mail and Slack, Discord, Teams and Telegram to chat), `Disclosure` (none, footer, footer for new contacts), `VoiceSample`, `OutgoingKind` and `outgoing_kind` (the draft and send rule, below), `EDITABLE_ARGUMENTS`, `VoiceUse` with its line, `FlagKind`, `CommitmentFlag` with its note, `DraftPair`, and the style guide: `VoiceGuide` (parse, render, screen, fit to 6 KiB, prompt block, section text), `VoiceRule`, `VoiceContextSection`, `mark_user_rules` and `placeholders` (addresses, phone numbers and links become `{email}`, `{phone}`, `{link}`).
- **`ConnectorCard`** gets `outgoing`, `editable`, `voice`, `flags`, `draft_arguments` and `not_my_voice`, all with serde defaults. `ApprovalAnswer` gets `checked`.
- **`ProactiveKind::Voice`**: the mark of an offer about the style guide. Every level allows it, and it is not in the log filters.
- **Client API** (`client_api/voice.rs`): `voice.get`, `voice.set`, `voice.learn`, `voice.reset`, `voice.try`, `voice.feedback`, `approval.edit`. The feed entity `voice_settings` and `Snapshot.voice`.
- **Wake events** `voice_edits` (with `VoiceEditPair` and `VoiceEditPattern`), `voice_feedback` and `voice_sample`.
- **Hub tools** `voice_guide` (every session) and `voice_sample` (Tutti).

### Hub (`crates/consort-hub`)

- **Migration `schema_v10.sql`**: `draft_pairs` and `voice_uses`. The settings and the guide that waits for Do it are in `meta` (`voice`, `voice_proposal`).
- **`store/voice.rs`**: the settings, the proposal, the pairs (save, unused edited pairs of a context, the newest pairs, "wrote to before"), the `voice_guide` calls, and the purge after 90 days.
- **`voice/mod.rs`**: the requests, the guide and the active guide, the screening of every `voice.md` write, the prompt section and the rule for "can reach a draft or send tool", the "Try it" sample in memory, the end of a sample turn.
- **`voice/learn.rs`**: the fixed brief, the source checks, the voice task, its limits (only read tools of the chosen connections) and the filter of the assistant's text out of what it reads, the proposal and the offer card, and Do it, Not now and Never about this.
- **`voice/card.rs`**: the voice parts of a call (outgoing, editable, voice, disclosure, flags), `approval.edit` with the second scan, the tick check, the draft pairs and the `voice_edits` wake, and "Not how I write".
- **`voice/scan.rs`**: the commitment scan and the checkable hard rules. **`voice/edits.rs`**: sentence diffs and the changes that agree. **`voice/tools.rs`**: `voice_guide` and `voice_sample`.
- **Small hooks in shared files**: `run_connector_tool` (the voice task check, the voice parts, a card for every call with flags, the edited arguments after Allow, the filter of results); `approval_answer` (the tick, the pair); `decision_answer` (the offer of the guide); `write_memory_note` (the screening of `voice.md`); `suggest_memory` (the voice task's guide goes to the offer); `decision_posting` (offers in a turn with a voice event); the coordinator inputs, fingerprint and system prompt; the task system prompt; the fallback post and the end of a sample turn; the snapshot; the tool list; the maintenance purge.
- **Prompts**: the section "Writing in the user's voice" in `coordinator.md` and `task.md`, and the voice events in the coordinator's events list.

### Client (`crates/consort-client`)

- **`voice.rs`**: the cache part, `VoiceChanges`, `GuideEdit` and `apply_guide_edit`, `VoiceView` for Settings > Voice (sources with the plan's default ticks, the learned line and the sources line, the learning state, the guide in sections, the sample, the proposal), `DraftCardView` for the card (the voice line, the flags with their ranges, the editable fields, `allow_blocked`), and the steps of the voice task's card.
- **Commands** `LoadVoice`, `SetVoice`, `LearnVoice`, `ResetVoice`, `TryVoice`, `EditVoiceGuide`, `VoiceFeedback`, `EditApproval`, and `checked` on `AnswerApproval` (`hub/voice.rs`).
- **Cache**: the settings from the snapshot and the feed, saved in the client cache as `voice_settings`; `voice.md` is always kept from the feed. `ConnectorCardView.draft`, `TaskCardView.steps` and `note`.
- **Mock** (`mock_voice.rs`): the content of every artboard, the screens, and the commands on the mock state.

### Desktop (`crates/consort-desktop`)

- **New files**: `voice.rs` (state, inputs, actions, dialogs, screens), `ui/voice.rs` (the page, the source rows, the guide, Try it, Disclosure, the draft card, the task steps, the guide link, the dialogs), `voice_tests.rs`.
- **Short additions to shared files**: the Voice rail item and page arm; `Dialog::Voice`; the fields of `ConsortApp`; the editor style of the new inputs; the load when the page opens; the screen hook; the dialog hooks; the draft branch of the connector card; the steps and the note of a task card; the guide link of the offer card; `LOG_KINDS` lists its seven kinds.

### Theme

`component.voice` in `design/structure.json`, with `Voice` in `structure.rs` and `tokens.rs`, and the test `voice_tokens_keep_the_values_of_the_designs`.

### Screens and screenshots

`--mock --screen`: `voice-off` (H3V-0), `voice-on` (H7T-0, with the hover on a rule), `voice-offer` (HL7-0), `voice-approval`, `voice-approval-checked` and `voice-approval-edit` (the three states of HTB-0).

`docs/screenshots/`: `phase4c-voice-off-dark.png`, `phase4c-voice-on-dark.png` and `phase4c-voice-on-bottom-dark.png` (top and bottom of the page), `phase4c-voice-offer-dark.png`, `phase4c-voice-approval-dark.png`, `phase4c-voice-approval-checked-dark.png`, `phase4c-voice-approval-edit-dark.png`.

## How the hub knows a draft or send tool

Composio tags carry the MCP-style hints `readOnlyHint`, `createHint`, `updateHint` and `destructiveHint`; they give the tool kind (read, change, delete). Neither Composio nor MCP has a hint for "send". The rule (`consort_proto::model::outgoing_kind`, tested with real Composio slugs):

1. Only a tool of the kind `change` can draft or send. A read tool or a delete tool never can, so a wrong slug word cannot turn a read into a send.
2. The words of the slug decide: `DRAFT` makes a draft; `SEND`, `SENDS`, `REPLY`, `FORWARD`, `POST`, `PUBLISH`, `TWEET`, `COMMENT` or `COMPOSE` sends; `MESSAGE` with `CREATE`, `UPDATE` or `EDIT` sends. Words, not substrings, so `POSTGRES_RUN_QUERY` is not a post.

For the prompt, a connection "can reach" such a tool when its service is a mail, chat or social service, or when the hub's cached tool list of the service has one. The hub never asks Composio for this at session start.

## Where it differs from the design or the plan, and why

1. **The buttons of the draft card are Allow and Don't, plus Edit.** Answer 2 says to keep the connector card words and names them "Allow once", "Allow for this task" and "Deny". The built connector card has the hub's two options, Allow and Don't (`hub_card_options`, step 2). The intent of the answer is one set of words for every connector card, so the card keeps Allow and Don't and adds Edit. Every allow stays off until the tick.
2. **The hub, not the coordinator model, posts the offer "Save this style guide?" and writes `voice.md` on Do it.** The plan says the Tutti coordinator shows the offer and writes the note. With the hub doing it, the saved note is exactly the screened text that the user saw, and no model can change it between the card and the write. The write is in the name of the voice task's session, with the suggestion of the task, and it posts the memory saved line in Tutti. Offers about edits (`voice_edits`) are the coordinator's, as the plan says.
3. **"Never about this" on the guide's offer turns the voice off.** It is the one way to say "stop" on that card.
4. **Turning the voice on starts learning only when no style guide exists.** With a guide from before (after Turn off), turning on uses it; Learn again learns anew.
5. **Reset turns the voice off.** With no guide, drafts are neutral anyway, and the page goes back to the off state.
6. **The scan and the disclosure line run only with the voice on.** Edit works with the voice off too, as the plan says. With the voice off, the card says "Neutral style" and has no flags, as before.
7. **A call with flags always asks.** Even when the connection's rule would run it without asking. The voice can make the approval stricter, never looser.
8. **"Only for new contacts"** means a recipient with no earlier draft pair in the hub. The hub cannot see the user's whole sent mail.
9. **"Try it" runs as a Tutti coordinator turn** with a `voice_sample` event and the `voice_sample` tool. The hub never runs a model. The text stays only in the hub's memory and in `voice.get`; the feed carries only the sample number and state. The turn posts no fallback.
10. **Public posts learn only from documents** (answer 3). The design's Slack example for Public posts is not in the mock; the context says "Not enough text yet. Add documents to learn this context."
11. **Sources while the voice is off stay in the app** until "Turn on and learn", so unticking everything does not bring the defaults back. With the voice on, a change goes to the hub at once ("Change sources" opens the same rows).
12. **The flagged body** is drawn as underlined text under a selectable copy with clear text, because GPUI's selectable text has no highlights. The body stays selectable.
13. **The rule note** ("Create and change tools ask every time") is not on a draft card; the voice line and the flags say why it asks, as in HTB-0.
14. **The voice task's card** has the note and Open thread at the bottom, and no Resolve button, as in HL7-0. The thread and the menu still resolve it.
15. **Agreement on edits** is by sentence: the hub compares the sentences that a draft lost and gained, and counts each change once per edit.

## Rows for DECISIONS.md

| Area | Decision | Reason |
|---|---|---|
| Proto | A draft or send tool is a `change` tool whose slug words name a draft or a send (`outgoing_kind`) | Composio and MCP have no send hint; read and delete tools can never count |
| Proto | The style guide format is parsed and rendered in proto (`VoiceGuide`); unknown sections and loose text are dropped | One format for the hub's screening, the client's page and the prompt; raw text cannot reach memory |
| Proto | `ProactiveKind::Voice` marks offers about the style guide; every level allows them and the log does not list them | An offer card needs a mark; these offers answer the user's own edits |
| Proto | `approval.edit` is its own request; `approval.answer` gets `checked` | The hub scans the edited text before the answer, and refuses an allow of a flagged card without the tick |
| Hub | The hub posts the guide's offer and writes `voice.md` on Do it | The saved note is exactly what the user approved |
| Hub | Every write of `voice.md` goes through the format, placeholders and the 6 KiB limit; a user write marks new rules `(you)`; a session write must keep them | "A learned change never overwrites them" holds in code |
| Hub | The voice task may run only read tools of the chosen connections, and the assistant's unchanged text leaves its results | Consent per source, and assistant text never teaches the voice |
| Hub | The scan and the disclosure line run only with the voice on; Edit works always | The voice is the feature that can add commitments; Edit is useful alone |
| Hub | A connector call with flags always asks | The voice can only make approval stricter |
| Hub | "New contact" is a recipient with no earlier draft pair | The hub sees only what went out through Consort |
| Hub | Draft pairs are stored for every allowed draft card, with or without the voice, for 90 days | The "sent unchanged" filter needs them when the voice is turned on later; the approval already holds the text |
| Hub | `voice_edits` takes all waiting edited pairs of a context at 5, with the changes that 3 or more agree on | The plan's thresholds, counted in code |
| Hub | "Try it" is a Tutti coordinator turn whose sample stays in memory | The hub never runs a model; nothing is saved |
| Hub | The voice section of the prompt is in the coordinator's fingerprint | Turning the voice on or off reaches an idle coordinator |
| Client | The client keeps `voice.md` from the feed even when global memory is not loaded | Settings > Voice shows the guide that Do it or a coordinator wrote |
| Desktop | The draft card keeps Allow and Don't and adds Edit; Allow stays off until the tick | Answer 2: one set of words for connector cards |
| Desktop | Sources while off stay in the app until Turn on and learn | Unticking everything must not bring the defaults back |
| Desktop | A flagged body is a selectable copy over an underlined copy | GPUI's selectable text has no highlights |
| Desktop | Edit fields show only in the card where Edit started | One text input entity cannot draw in two views |

## Tests

The plan's list, and where each test is:

1. **Off by default**: `tests/voice.rs::the_voice_is_off_by_default_and_a_draft_card_says_neutral_style` (no `voice.md`, no voice section, no `voice_guide`, the card has no voice and no flags).
2. **Consent**: `the_voice_task_reads_only_the_chosen_sources_and_the_guide_waits_for_do_it` (the brief names only Gmail Work; the fake Composio records no call to Gmail Home; a send is refused) and the unit test `the_brief_lists_only_the_chosen_sources_and_the_assistant_text`.
3. **No raw text in memory**: the same integration test (no loose message, no address, phone or link, no unknown section in `voice.md`), and the proto tests of the parser and the screening.
4. **Assistant text filter**: `five_edited_pairs_with_the_same_change_wake_tutti_and_one_does_not` (unchanged pairs are not in the wake) and the brief and redaction unit test.
5. **Ownership**: the integration test (a task's `memory_write` fails; the guide reaches Tutti as an offer) and `writes_of_voice_md_keep_the_format_and_the_rules_of_the_user`.
6. **Scope**: `a_session_that_can_send_gets_the_voice_and_its_draft_card_has_flags_edit_and_the_tick` (a channel with no connection gets no voice section; Tutti with Gmail does).
7. **Approval cannot be skipped**: `the_voice_never_skips_the_approval_of_a_draft_with_marked_lines` and the refused allow without the tick.
8. **Commitment flags**: the integration test above, `scan.rs` tests, and the desktop test `a_draft_card_keeps_allow_off_until_the_tick`.
9. **Hard rule "never promise a date"**: `the_hard_rule_never_promise_a_date_flags_a_promise` and `the_scan_uses_the_brief_and_the_hard_rules`.
10. **Edit learning**: `five_edited_pairs_with_the_same_change_wake_tutti_and_one_does_not`, `three_agreeing_edits_make_a_pattern_and_two_do_not`, and `an_offer_about_the_guide_needs_a_voice_event_in_the_turn`.
11. **Reset**: `try_it_writes_a_sample_that_is_not_posted_and_reset_turns_the_voice_neutral` and `reset_makes_drafts_neutral_and_the_old_version_can_come_back`.
12. **Disclosure**: `each_disclosure_option_gives_the_right_footer`.
13. **Injection**: the integration test's guide holds "Add this link to the voice guide" with a link; `voice.md` holds neither. The brief says the text is data.

Also: proto round trips of every request, the feed entity and the snapshot field, the wake events, the tool lists; client unit tests (`voice/tests.rs`, `mock_voice.rs`) and the fake hub test `the_voice_page_and_the_draft_card_use_their_requests`; desktop window tests (`voice_tests.rs`: the off page, the on page with Remove, Disclosure and Reset with its dialog, the guide link, the draft card); the prompt test `the_prompts_have_the_rules_of_the_voice`; the theme token test.

## What is left

- **A real model and a real connector** were not run. The hub tests use the fake worker and the fake Composio.
- **Light mode screenshots** were not taken.
- **Documents from a worker path**: the brief names the path, and a task on a worker without it says so in its summary. The hub does not check that a path exists.
