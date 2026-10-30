# Designs for phases 3, 4 and 5

These are the Paper designs for the missing screens of phases 3, 4 and 5 in `docs/FINISH-PLAN.md`. The Paper file is `01M6342D512B1CKH0D29HMDN8V`, page Desktop. The artboards start at worldX -720, worldY 6400. Each artboard is 1440 by 900.

All screens use the larger sidebar scale: 288 px wide, rows with 7/10 padding, 15 px text, 18 px icons, 13 px group labels and a 52 px title bar. New icons come from the hand-drawn set in `crates/consort-desktop/assets/icons/`. The layer names mark selectable text with "(selectable)" and scroll areas with "(scrolls)".

## Artboards

### Phase 3: hub, clients and notifications

| Artboard | ID | What it shows |
|---|---|---|
| Settings, Hub, dark | `7JP-0` | Address with copy, hub and protocol versions, the data folder, a storage bar (database, blobs, skills), and Backup: Back up now, the last backup and its file, the schedule (Off, Daily, Weekly) and time, the folder on the hub, how many to keep, and the restore command with Copy command. |
| Settings, Clients, dark | `86L-0` | The paired apps with name, kind, who paired them and when, and last seen. One row shows the inline Revoke confirm step. |
| Pair a new app dialog, dark | `8L0-0` | The setup code `KQ7M-4XPD`, a QR code, a 10 minute countdown bar, "Works once", the hub address with copy, "Waiting for the new app", New code and Cancel. |
| Settings, Notifications, dark | `8XN-0` | Five switches (approvals and decisions, Ready for review, Failed, a message in a channel that is not open, routine results), Show message text (off), the Dock badge with a preview, the macOS permission line, an example banner, and Test notification. |

### Phase 4: conversation and tasks

| Artboard | ID | What it shows |
|---|---|---|
| Thread panel for a message, dark | `95W-0` | The right panel with the root message, "4 replies", the replies as bubbles and a reply box. In the timeline, the root message has an accent ring and the line "4 replies, Last reply 9:31 AM". |
| Reactions and message menu, dark | `9I9-0` | Reaction chips under bubbles (yours in the accent, the assistant's neutral, a count only when both reacted) and the message menu with six quick emoji, More, and the four menu items. |
| Search results, dark | `A0N-0` | The Cmd+K palette as search: the scope filter (All, Messages, Tasks, Memory), "Go to" (channels and tasks), then Messages, Memory and Tasks from the hub, each with its channel, a snippet with the match in bold, and its time. |
| Search, loading and no results, dark | `AFG-0` | The loading state (local results at once, skeleton rows for the hub) and the empty state with "Ask Tutti to search". |
| New task dialog, dark | `ARL-0` | Title, brief, channel, model and effort, where it runs (any worker with the channel's tags, or one worker), repo and isolation, approval mode, and Start now or at a date and time. |
| Task item view, command output, dark | `CA5-0` | The task panel widened to 560 px for one command: the whole output in mono with line numbers, exit code and time, Copy output, and the item number. |
| Task item view, diff, loading and offline, dark | `CPJ-0` | A file diff with added and removed lines, and two states: "Getting the output from bigboi" and "truenas is offline" with Try again. |
| Channel settings dialog, dark | `CYJ-0` | One dialog in two columns. General (icon, name, purpose, instructions), Assistant (model and effort, channel memory), Work (worker tags, repos with add and remove, default isolation, default approval mode, allowed local MCP servers), Connectors and skills (a link to the existing dialog). Archive and Delete are at the bottom left. |
| Task thread with bubbles, dark | `BDH-0` | The task thread restyled with bubbles: the brief as bubbles with the placement under it, a task update with the activity chip, an approval card and a decision card with task lines, and the model line under the reply box. |
| About, dark | `DWF-0` | The Three voices mark, the wordmark, the version and build, the hub it connects to, an update section ("Version 0.7.1 is ready", Restart to install, Get updates on their own), and links. |
| New channel dialog, simple, dark | `EYQ-0` | The new small New channel dialog: an icon picker with seven choices and More, the name, Cancel and Create channel. It replaces "New channel dialog with tags, dark" (`3T0-0`). |
| New channel, first greeting, dark | `F6N-0` | A new channel right after creation: a time separator, the notice "You made the channel reading.", the greeting in two bubbles, and three quick-reply chips. |
| New channel, assistant writing, dark | `FDX-0` | The user tapped Yes. The chips are gone, the activity pill says "Writing in reading", and a writing bubble with three painted dabs sits in the assistant lane. |
| New channel, setup question, dark | `FJC-0` | The first setup question ("What is reading for?") with four chips, one of them "I will type it". |
| Sidebar, new channel or group, dark | `FOZ-0` | Left: the Tutti row on hover with its "+" button, the tooltip "New channel or group" and the menu (New channel, New group). Right: a new empty group with the hint row "Add a channel", and the group row on hover with its menu button and "+" ("New channel in Learning"). |
| Group dialogs, dark | `FWA-0` | The New group dialog (name only), the New channel dialog opened from a group's "+" with "In Learning" under the title, and the Delete group confirm dialog. |

### Phase 5: install and run anywhere

| Artboard | ID | What it shows |
|---|---|---|
| Add worker dialog, step 1, dark | `E3F-0` | Three steps (System, Install, Connect) and step 1: choose macOS, Windows or Linux. Each choice says how the worker runs there. |
| Add worker dialog, steps 2 and 3, dark | `EI4-0` | Step 2: one copyable PowerShell command that installs the worker and pairs it with the code, the code expiry, New code, and "Waiting for the worker to connect". Step 3: "bigboi is online" with a short checklist and Add an account. |
| App update in the sidebar, dark | `EL0-0` | A quiet notice above the account row: "Update ready. Restart to install." with Restart. |
| Hub offline at start, dark | `ERQ-0` | The app opens with saved data. The pill under the mark says "Cannot reach the hub. You see saved data from 9:41 AM." with Try again. Workers show "Status unknown", approval buttons are dimmed, and a queued message says "Sends when the hub is back". |

## Decisions the plan did not settle

- Settings pages that are not lists (Hub, Clients, Notifications) use one 760 px column, with section titles and bordered row groups as in Settings > Connectors.
- The settings sidebar has "About Consort" in a footer row. About opens as a dialog over Settings.
- The storage bar shows only Consort's 4.2 GB, split by kind, in neutral text colors. Free disk space is a line of text. Status colors stay for state.
- Backup has "Back up now" as a secondary button, because it is not an answer to the assistant.
- Revoke uses an inline confirm in the row, not a dialog. The app that you use has no Revoke.
- The pairing dialog also shows the hub address, because a typed code alone does not tell the new app where the hub is.
- Notifications get a Dock badge preview and an example banner, so the user sees what "message text off" means.
- A thread hangs off a whole message. While the thread panel is open, every bubble of the root message gets an accent ring, the same rule as an open task card.
- Reaction chips in the accent mean that you reacted. A count shows only when you and the assistant chose the same emoji. A quick emoji that you already chose is marked in the menu, and a second click removes it.
- Search shows local results (channels and tasks) at once and hub results under them when they come. Matches are in bold and `text.primary`, not in the accent.
- The task item view widens the side panel from 400 px to 560 px. Output starts at the end, because the end of a command matters most.
- A worker that is offline cannot give full output. The panel says so and keeps the summary in the thread.
- The channel settings dialog is 880 px wide with two columns, so all sections fit without a scroll. It has an explicit Save changes button.
- Channel memory on or off is a switch in Assistant. Allowed local MCP servers are checkboxes, with the worker that has each one.
- The task thread shows the brief as the first bubbles, with the placement as a caption under it. The old placement table at the top of the panel goes away.
- The step 1 label of the Add worker dialog changes to the chosen system ("Windows") after step 1.
- The install command for Windows is `irm https://get.consort.dev/worker.ps1 | iex; consort worker install --hub <hub> --code <code>`. The domain is a placeholder.
- The app update notice sits above the account row in the sidebar footer. About has the full update section.
- When the hub is offline, the status pill under the mark holds the notice. Approvals and decisions cannot be answered, so their buttons are dimmed. Messages wait in a queue.
- The new channel greeting comes from the channel's assistant, not from a hub card. It has two bubbles: who it is and what the channel is, then the offer to set it up.
- Quick-reply chips are pills on `surface.raised` with a `border.strong` line. "I will type it" has a dashed border, because it only moves the focus to the composer and sends nothing.
- The writing bubble uses the three dabs of the mark at 10 px. In the app they shimmer on the shared stop-motion clock and stand still with Reduce motion.
- The Tutti row "+" has no filter button next to it. Consort has no filter that the sidebar needs today.
- I removed the "New channel" row at the end of the sidebar list in the group design, because the Tutti "+" and the group "+" do its job. An empty group shows the hint row "Add a channel".
- A new channel from the Tutti "+" goes into no group. A new channel from a group's "+" goes into that group.
- Deleting a group moves its channels to the top of the sidebar, outside any group. The dialog does not submit on Enter.

## Contract notes for the builder

- **`suggestions` on assistant messages.** A list of at most 4 short strings. `post_message` and `reply` can set it. The client shows chips only under the newest assistant message. A click sends the chip text as your message. Any reply from you (a chip, typed text or an attachment) hides the chips.
- **A `channel_created` wake event.** The hub sends it to the new channel's coordinator, so the coordinator posts the greeting with suggestions.
- **Typing state.** The writing bubble needs a "composing" signal for a coordinator turn. The activity pill already uses the turn state. The bubble can use the same state.
- **Hub requests.** Settings > Hub needs `hub.status` (address, versions, data folder, storage use) and `hub.backup` and `hub.backup_settings`, as in the plan.

## Questions for the user

1. Should Back up now also show a progress bar while a backup runs, or is the row text enough?
2. Is the mobile client in Settings > Clients now, or do we hide mobile until phase 6?
3. Should "Show message text" stay one switch, or be one switch per kind of notification?
4. Should a reply in a thread also post to the timeline when you choose (like "Also send to channel")?
5. Is a full emoji picker behind More enough, or do you want the system emoji picker?
6. Should search results open in place (the plan says so) or open the thread panel for thread replies?
7. Do you want Save changes in channel settings, or should changes save at once like Settings pages?
8. For the worker install, which domain serves the scripts? Is `get.consort.dev` a placeholder you want to keep?
9. Should the app update itself without asking ("Get updates on their own" on by default)?
10. While the hub is offline, should the app let you answer approvals and queue the answers, or keep them blocked as designed?
11. Should the "New channel" row at the end of the sidebar list come back for first use, for example while you have fewer than three channels?
12. Should the setup questions after the purpose (connectors, repos, model) each get their own chips, such as the names of your connectors?
