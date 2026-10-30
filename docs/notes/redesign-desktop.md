# Chat redesign: desktop notes

These notes record the chat redesign of the desktop app. The design is the Paper artboard "Tutti, bubbles, dark", the Three voices prototype (`active-mark/three-voices.html`) and the hand-drawn icon set. `docs/design-system.md` has the rules that came from it ("The chat", "Logo", "Icons and glyphs", "Motion").

## What we built

### Client (`consort-client`)

- `chat.rs` turns the items of a timeline into the rows of the chat: bubbles in two lanes, cards, notices and time separators. It also splits a message into its paragraphs. The view state did not change shape for this. The app makes the rows when the timeline changes.
- `activity.rs` finds what the assistant of each channel does now. The result is the state of the mark (Idle, Working, Needs you) and the line in the activity pill. `ViewState::activity` holds it for each channel. The reducer makes it from the cache.
- The cache keeps the newest session item of each session, with the channel of its feed entry (`Cache::session_activity`).
- Task cards, approval cards, decision cards and notices have a `created_at` time, so the separators can measure gaps.
- The main channel shows as "Tutti" (`view::TUTTI`). The code and the wire still say Everything.
- The mock has times for its items and an activity for each channel. Tutti's activity is "Running a command in sayso", as in the design.

### Desktop (`consort-desktop`)

- **Timeline.** Bubbles, groups, time separators, task lines and cards at bubble width, in `ui/timeline.rs`. Message text, code blocks and brief items stay selectable. The message, code, selection and task menus are on the bubbles, the task lines and the cards. The top fade is 96 px and the bottom fade is 32 px.
- **Composer.** One field with the send key inside, and a tools row under it: add, voice, a divider, the model and effort chips, a spacer, and "Runs on hub". The placeholder is "Message Tutti" or "Message <channel>".
- **No header.** The Three voices mark and its activity pill float at the top center. Connectors, Tasks and New task are icon buttons at the top right. The top 48 px of the column moves the window, as the header did.
- **Three voices mark** (`ui/mark.rs`). `MarkPlayer` follows the activity of the open channel. When work ends, it holds Done for 1.5 s and then shows the new state. The frames come from `design/mark/generate.mjs`.
- **Stop-motion clock** (`ui/motion.rs`). One 8 fps timer moves the mark, the live status icons and the hovered icon buttons. It runs only while a draw showed something that moves and the window is active. It replaces the 15 fps timer of the old Working arc.
- **Icons** (`ui/icons.rs`). The Lucide files are gone, except `bot.svg`. Every icon name goes through `icons::resolve`, so old channel icons from the hub still show. `build.rs` lists every SVG in `assets/`, so `assets.rs` has no list to keep by hand.
- **Reduce motion** in Settings > Appearance. It is saved in `appearance.json` (`SavedChoice::reduce_motion`). It starts from that choice, else from `defaults read com.apple.universalaccess reduceMotion` on macOS, else off. GPUI's own `reduce_motion` flag carries it.
- **The mark replaces the brace** in the sidebar (idle, sharp, 16 px), the switcher, the Windows and Linux title bar, onboarding, the start-up error and the app icons in `design/logo/`.

### Design files

- `design/mark/generate.mjs` and its README. It writes 152 frames: idle 1, working 12, needs you 24 and done 1, in `dark/`, `light/`, `dark-sharp/` and `light-sharp/`.
- `design/icons/` holds a copy of the icon generator. It now also writes the currentColor versions 1 and 2 to `assets/icons/v1/` and `assets/icons/v2/`.
- New tokens in `design/structure.json`: `layout.bubble_max_width`, `layout.fade_top`, `layout.fade_bottom`, `component.chat.*`, `motion.frame_ms` and `motion.done_hold_ms`. `docs/design-system.md` lists them.

## Where the app differs from the design, and why

1. **The mark looks smaller in its 44 px box.** The frames use a viewBox of 80 units, so every pose of Working and Needs you fits without a cut. The Paper artboard crops one pose to 60 units.
2. **The approval card shows the folder on its own row** ("Folder C:\Code\sayso"), not inside the summary line. The agent writes the summary. The folder must always show and be selectable (review cycle 3, S6).
3. **The card's first row also shows the approval mode** ("Ask mode") before the worker pill. The old card header showed it, and nothing else does.
4. **A task line in Tutti adds "in sayso"** for a task of another channel. The old card header showed the channel.
5. **A brief item shows its channel** next to its chip. The old brief rows showed it.
6. **"Open thread" goes under the answer buttons** when the label of the wide answer is long. The buttons wrap, as before, so the user reads the whole scope.
7. **An approval or a decision of the channel's assistant keeps its inner header**, with the request icon and what it asks. That header says what is asked, not a task name. Its task line says "Approval" or "Decision" and the state.
8. **Working no longer turns.** It shimmers like the other live icons, because the brief asks for the shimmer.
9. **The Connectors button shows its count** next to its icon. The old header link showed the count, and the count tells whether a connector is on.
10. **A resolved task card is its task line only**, with its summary ("Resolved, 6 replies"). The old card collapsed to one row too.
11. **No time on each message.** The separators carry the times. A message on its way says "Sending" under its last bubble, and an edited message says "Edited".
12. **In the mock, Tutti's pill says "Running a command in sayso"** while the sayso task waits for its approval. That matches the artboard, not the mock tasks.

## Header items that moved

| Header item | Where it is now |
|---|---|
| Channel name | The composer placeholder ("Message sayso") and the selected sidebar row |
| Worker tag chips (a click opened Worker tags) | The channel menu: Worker tags |
| Purpose line | The Instructions dialog: a new Purpose field, which also saves it (`--screen instructions`) |
| Tasks button with "2 need you of 5 tasks" | The Tasks icon button, with a gold dot while something needs you. The Tasks panel header says "2 need you", and its Active tab keeps the count |
| Connectors link with its count | A Connectors icon button with the count, at the top right, for a channel that has connections or skills |
| New task button | The New task icon button at the top right |
| The window drag area | A 48 px strip at the top of the column |

## Icon uses with no match

The set has no icon for these. The app shows the nearest icon, and nothing new was drawn.

| Old icon | Use | Shown as |
|---|---|---|
| `bot` | A provider with no logo | The Lucide `bot` stays, because it stands for a provider logo |
| `circle-alert` | Error boxes and failed notices | `warning` |
| `hard-drive` | A channel icon (mac-ext4) | `server` |
| `lightbulb` | A channel icon (house-lights) | `sun` |
| `smartphone` | Settings > Clients | `worker` |
| `restore` | The Restore window control on Windows and Linux | `copy` |

The Up next and Resolved statuses keep Consort's glyphs (a dashed ring and a solid disc), because the set has no icon like them.

## Events for the activity line

The client has no "the assistant is busy" event. `activity.rs` reads these:

- Task status: a task in Working. Tutti counts the tasks of every channel. Another channel counts its own.
- `session_item` feed entries: the kind, the status and the time of the newest item of each session, and the channel of the feed entry. An item in progress gives the line: a command is "Running a command", a file change is "Changing files", a web search is "Searching the web", a tool call is "Using a tool", a subagent is "Working on a part of the task", reasoning is "Thinking", text is "Writing" and a plan is "Making a plan". The line adds "in <channel>" for work in another channel. A session that is no task's current session is the coordinator of its channel.
- The newest timeline message: a message that you sent, with no reply after it, means the coordinator works.
- Your turn: a task that needs input or waits for review, or an approval or a decision in the timeline that waits. This gives Needs you when nothing works.
- With no item in progress, the line says "Working".

## Decisions the plan did not settle

- Time separators: the day and the time for the first item of a day, then the time after a gap of 30 minutes or more.
- A date label with a number gets a comma before the time ("October 12, 9:00 AM").
- A paragraph or a heading starts a bubble. A heading keeps its first paragraph. A list, code, a table, a quote or a rule joins the bubble before it.
- Only the assistant's messages split into bubbles. Your message stays one bubble.
- Reference link definitions go to every bubble of a message, so links still work.
- A brief shows its title as its own semibold bubble.
- Cards, notices and separators end a group of bubbles.
- Working wins over Needs you for the mark.
- Done shows for 1.5 s after Working ends, also under Reduce motion, because it is a change of state.
- A change of channel shows the new channel's state at once, with no Done.
- The working loop is 12 frames: pose `f % 12` and edge `f % 3` repeat after 12 frames, so 36 frames would only repeat them.
- Sharp frames exist for every state, not only idle.
- The mark pre-loads the frames of its loop, so a frame never draws empty.
- The stop-motion clock also stops when the window is not active.
- Hovered icon buttons and live status icons share the one 8 fps clock with the mark.
- The status icons Working, Needs input, Ready for review, Failed and Scheduled come from the set: `working`, `needs-input`, `done`, `failed` and `clock`.
- Channel icons stay Lucide names on the hub, and the Change icon menu still offers them. `icons::resolve` maps them.
- The main channel shows as Tutti in the client view state, whatever the hub calls it.
- Reduce motion is saved with the appearance choice and written only when the user sets it.
- `build.rs` makes the asset table from the files in `assets/`.

## Checks

- `cargo fmt --all --check`: passes.
- `cargo clippy --workspace --all-targets -- -D warnings`: passes, with the hub agent's work in progress in the tree.
- `cargo test -p consort-client -p consort-desktop -p consort-theme`: passes. consort-client: 197 unit, 46 fake hub and 6 start-up tests. consort-desktop: 158 tests. consort-theme: 29 tests.
- New client tests: 16 in `chat.rs` (paragraphs, groups, keys, separators) and 9 in `activity.rs` (the line, the state, the events).
- New desktop tests: 8 headless window tests in `chat_tests.rs` (the bubbles and their lanes and gaps, the composer, the Tasks button and its gold dot, New task, the mark states and Done, 8 fps and Reduce motion, the shimmer on hover). Unit tests in `ui/mark.rs` check that every frame is in the binary and that resvg 0.46, the renderer of GPUI's images, draws the paint filter and the screen and multiply blends. `ui/icons.rs` and `ui/motion.rs` have their own tests.
- Both generators give byte-identical output on a second run.

## Screenshots

New files in `docs/screenshots/`, from `scripts/dev.sh --mock` with `--screen`:

- `tutti-bubbles-dark.png`, `tutti-bubbles-light.png` (no `--screen`, or `--screen tutti`)
- `sayso-bubbles-dark.png` (`sayso`)
- `channel-requests-bubbles-dark.png` (`channel-requests`): approval and decision cards of the assistant
- `tasks-panel-bubbles-dark.png` (`tasks`)
- `mark-idle-dark.png`, `mark-working-dark.png`, `mark-needs-you-dark.png`, `mark-done-dark.png`, `mark-working-light.png` (`mark-idle`, `mark-working`, `mark-needs-you`, `mark-done`)
- `settings-reduce-motion-dark.png` (`settings`)
- `instructions-purpose-dark.png` (`instructions`)

## What is left

- The rename of Everything to Tutti in Rust names, wire names and the hub.
- The mobile client: `chat.rs` and `activity.rs` are in the shared client, so it can use them.
- The task thread panel still shows posts with a speaker line, not bubbles. The plan did not ask for it.
- An About screen does not exist yet. When it comes, it uses `ui::mark::still_mark`.
- The app has no bundle yet, so the new app icons in `design/logo/` are not in a `.icns` file.
