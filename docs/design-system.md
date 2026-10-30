# Design system

This page is the reference for how Consort looks, on the desktop and on mobile. The values live in files, and this page explains them:

| File | Contents |
|---|---|
| [`design/structure.json`](../design/structure.json) | Type, spacing, corners, borders, shadows, motion, icons and layout. The same for every theme |
| [`design/themes/`](../design/themes/) | One file per theme with the color roles for a dark mode and a light mode. Spruce is the default |
| [`design/theme.schema.json`](../design/theme.schema.json) | The schema that every theme file must match |
| [`design/gpui-kit-mapping.json`](../design/gpui-kit-mapping.json) | How the color roles become GPUI Kit theme keys |
| [`design/logo/`](../design/logo/) | The Three voices mark and the app icons |
| [`design/mark/`](../design/mark/) | The generator of the Three voices frames (`assets/mark/`) |
| [`design/icons/`](../design/icons/) | The hand-drawn icon set and its generator (`assets/icons/`) |
| [`design/glyphs/`](../design/glyphs/) | Consort's own glyphs: the task-line connector and the task statuses that the icon set does not have |

Why themes can change only color: [ADR 0009](adr/0009-themes-change-color-only.md).

## Principles

1. **One voice.** One type family, one icon set and one corner system everywhere. A harness, a model or a machine never gets its own style.
2. **Color means state.** Surfaces stay neutral. Color appears only where something has a status. Code blocks are the one exception, and their colors stay muted.
3. **The accent means your turn.** The accent marks you: your messages, the approvals and decisions that wait for you, and the buttons that answer them.
4. **Notation signs mark Consort's own ideas.** The Three voices mark is three dabs of pigment for the voices of a broken consort. It is the logo, the icon for Tutti, and the sign of what the assistant does. The fermata means "hold until the leader gives the cue". It marks a task that needs your input. Every icon comes from Consort's own hand-drawn set.
5. **Depth comes from surfaces, not shadows.** Five surface steps and two border colors. Only floating layers cast a shadow.
6. **Calm by default.** Motion answers your actions or shows work in progress. Nothing moves for decoration, and Reduce motion stops it.

## Themes

### How a theme works

A theme sets color roles for a dark mode and a light mode, and nothing else. Consort has two built-in themes: Spruce (the default) and Velvet.

The desktop app loads a theme in these steps:

1. Read the theme file, either built in or from the user themes folder `<config dir>/consort/themes/`.
2. Check it against `theme.schema.json`.
3. Check the [contrast rules](#contrast-rules).
4. Derive every GPUI Kit color key with `gpui-kit-mapping.json` and the [derived colors](#derived-colors).
5. Register the result with GPUI Kit's `ThemeRegistry`.

If step 2 or step 3 fails, the app does not load the theme. It shows the file name, the role that failed and the measured ratio. The app watches the user themes folder and applies a changed theme at once.

The mode setting is System, Light or Dark. Each client stores its own theme and mode, so the phone and the desktop can differ. The mobile apps get the built-in themes from the same files at build time.

### Color roles

| Role | Use |
|---|---|
| `surface.sidebar` | The sidebar, the title bar, the tab bar and the status bar |
| `surface.canvas` | The timeline and the main content area |
| `surface.card` | Task cards, approval and decision cards, and code blocks |
| `surface.raised` | Secondary buttons, inputs inside cards, menus and popovers |
| `surface.selected` | The selected row in the sidebar, a list or a tab bar |
| `border.subtle` | Dividers, card borders and the window border |
| `border.strong` | Input borders, secondary button borders and link underlines |
| `text.primary` | Message text, titles and active items |
| `text.secondary` | Details, sidebar items and descriptions |
| `text.tertiary` | Timestamps, placeholders, disabled text and the Up next status. Never on `surface.selected` |
| `accent.fill` | Primary buttons and the base of the selection color |
| `accent.text` | The accent as text or as a glyph: Needs input, Ready for review, "You", the focus ring, the caret and switches |
| `accent.on_fill` | Text and glyphs on `accent.fill` |
| `accent.surface` | The background of your own messages |
| `status.working` | The Working status, progress bars and info messages |
| `status.done` | The Resolved status, online workers and success messages |
| `status.failed` | The Failed status, errors and destructive actions |
| `status.scheduled` | The Scheduled status |
| `syntax.*` | Code blocks only |

GPUI Kit uses the word "accent" for the hover and selection color of menus and lists. In the mapping, the kit's `accent.background` is our `surface.selected`. Our accent is the kit's `primary`.

### Spruce values

| Role | Dark | Light |
|---|---|---|
| `surface.sidebar` | `#0F1815` | `#EBF2EE` |
| `surface.canvas` | `#131E1A` | `#F8FCF8` |
| `surface.card` | `#1B2623` | `#E9F2ED` |
| `surface.raised` | `#222E2A` | `#FFFFFF` |
| `surface.selected` | `#293732` | `#DDE9E3` |
| `border.subtle` | `#27322E` | `#D8E1DD` |
| `border.strong` | `#384440` | `#C2CCC7` |
| `text.primary` | `#F0EBDF` | `#192622` |
| `text.secondary` | `#B3BAB1` | `#46534F` |
| `text.tertiary` | `#868F89` | `#64706C` |
| `accent.fill` | `#E7BE62` | `#E1B856` |
| `accent.text` | `#EAC673` | `#93640B` |
| `accent.on_fill` | `#2B2005` | `#2B2005` |
| `accent.surface` | `#3E3519` | `#F2E8C7` |
| `status.working` | `#77B4ED` | `#1364B0` |
| `status.done` | `#80C7A8` | `#237356` |
| `status.failed` | `#F1735A` | `#C1351E` |
| `status.scheduled` | `#B2A0D6` | `#6D5398` |

The names come from the materials of a consort: spruce for the surfaces, lead white for the dark-mode text, brass for the accent, and Renaissance pigments for the statuses (azurite, verdigris, vermilion and folium). The values were set in OKLCH and converted to sRGB.

### Derived colors

The mapping derives these colors. Theme authors do not set them.

| Color | Rule |
|---|---|
| Hover | OKLCH lightness up by 0.035 in dark mode, down by 0.035 in light mode |
| Pressed | OKLCH lightness up by 0.07 in dark mode, down by 0.07 in light mode |
| Status tint (badge backgrounds) | The status color at 16 percent opacity |
| Selection | `accent.fill` at 28 percent opacity |
| Scrim behind dialogs | Black at 55 percent opacity in dark mode, 30 percent in light mode |
| Text on a status fill | `surface.sidebar` or `text.primary`, whichever has the higher contrast |

### Contrast rules

The theme loader measures WCAG contrast ratios for each mode. A theme that fails any rule does not load.

| Foreground | Backgrounds | Minimum ratio |
|---|---|---|
| `text.primary`, `text.secondary` | Every surface and `accent.surface` | 4.5 |
| `text.tertiary` | `surface.sidebar`, `surface.canvas`, `surface.card` | 4.5 |
| `accent.text` and every `status.*` role | `surface.sidebar`, `surface.canvas`, `surface.card` | 4.5 |
| `accent.on_fill` | `accent.fill` | 4.5 |
| Every `syntax.*` role | `surface.card` | 4.5 |
| `accent.text` | `surface.canvas`, `surface.selected` | 3.0 |

Spruce and Velvet pass every rule in both modes. The lowest text ratio is 4.51, for Spruce light.

### Task status

Each status has one glyph and one color, everywhere: the sidebar, task cards, the Tasks panel, notifications and the mobile app.

| Status | Glyph | Color role |
|---|---|---|
| Up next | glyph `status-up-next.svg` (dashed ring) | `text.tertiary` |
| Scheduled | icon `routine` (the metronome) | `status.scheduled` |
| Working | icon `working` (an arc on a faint ring), shimmers while live | `status.working` |
| Needs input | icon `needs-input` (fermata), shimmers while live | `accent.text` |
| Ready for review | icon `done` (ring with check) | `accent.text` |
| Failed | icon `failed` (ring with cross) | `status.failed` |
| Resolved | glyph `status-resolved.svg` (solid disc with check) | `status.done` |

Needs input and Ready for review both use the accent, because both are your turn. The fermata is filled, and the review ring is open, so the two stay apart without a second color.

Up next and Resolved keep Consort's own glyphs, because the icon set has nothing like them. A live status (Working or Needs input) shimmers through the three versions of its icon at 8 frames a second. It stands still under Reduce motion.

### Channel icons

A channel can have any icon of the set. The New channel dialog shows seven quick icons (`code`, `globe`, `server`, `home`, `memory`, `appearance`, `star`) and All icons. All icons opens a dialog over it with a search and every icon in six categories (`ui/icon_catalog.rs`). A word finds an icon when it starts a word of its name or of a keyword; the keywords include the old Lucide names, so "cloud" finds `globe`. An icon picked in All icons takes the place of the last quick icon. The channel settings and Change icon in a channel's menu open the same dialog.

An icon tile has four states. At rest: no ground. Hover (the active state): `tint_accent` ground, the icon in `text.primary`, and the shimmer through its three versions. Picked: `surface.selected` ground with a `text.secondary` border; it does not shimmer, so the accent stays the color of hover and focus. Keyboard: the focus ring. Designs: `docs/notes/design-icon-picker.md`.

A worker that is online shows a solid dot in `status.done`. A worker that is offline shows an open ring in `text.tertiary`.

### Where status and requests appear

- A sidebar row shows the status glyph of the most urgent task in that channel, in this order: Needs input, Ready for review, Failed, Working. A channel with something for you has its name in `text.primary`. Other channel names use `text.secondary`.
- A task card in a timeline sits under its task line (see [The chat](#the-chat)). It shows a pending approval inline, with its buttons, because the harness cannot continue until you answer. A decision does not block the harness, so it appears in the task thread and in the Tasks panel, not inline.
- When a task thread is open in the side panel, its task card in the timeline has an `accent.text` border.
- In a decision card, the recommended option has an `accent.text` border on `surface.raised` and the word "Recommended". The other options have `border.strong` borders. Every option shows its consequence under its label. Only the option's Choose button answers the decision. The option text stays selectable and is not a click target, so selecting text cannot send an answer.
- Every control that a mouse can use also takes keyboard focus. A focused control shows the focus ring: a `component.focus_ring` line in `accent.text`, drawn outside the control so the layout does not move. Enter or Space activates a focused control, and Enter in a form field submits the open form, except in dialogs that delete something or open a link.

## Type

Consort uses one family: Atkinson Hyperlegible Next for text and Atkinson Hyperlegible Mono for code, paths, IDs and commands. The family was drawn so that letters such as `I`, `l` and `1`, or `0` and `O`, never look alike. That matters in an app that shows task IDs, hashes and paths all day. Both fonts use the SIL Open Font License, so every client can ship them.

| Style | Size and line height (px) | Weight | Use |
|---|---|---|---|
| Display | 30 / 36 | 600 | Onboarding and large empty states only |
| Heading | 22 / 28 | 600 | Dialog and page titles |
| Title | 17 / 24 | 600 | Channel names, panel titles and task titles in a panel |
| Body | 15 / 24 | 400 | Message text. At most 68 characters per line |
| UI | 13 / 20 | 400 | Sidebar rows, lists, menus and buttons |
| UI strong | 13 / 20 | 500 | Button labels, card titles and the speaker name |
| Label | 12 / 16 | 500 | Field labels |
| Caption | 11 / 16 | 400 | Timestamps and small details |
| Mono | 13 / 20 | 400 | Code blocks. Inline code is 0.92 em of the surrounding text |

Display and Heading use tracking of -0.015 em and -0.01 em. All other styles use none.

Set every label, button and title in sentence case. Do not set text in all caps, and do not add a label above a heading.

## Spacing and layout

All spacing uses one scale on a 4 px grid:

| Token | Value (px) |
|---|---|
| `xxs` | 2 |
| `xs` | 4 |
| `sm` | 8 |
| `md` | 12 |
| `lg` | 16 |
| `xl` | 24 |
| `xxl` | 32 |
| `page` | 48 |

Fixed layout sizes:

| Item | Size (px) |
|---|---|
| Sidebar width | 288 |
| Side panel width (Tasks, task thread, Channel panel) | 400 |
| Timeline content width | At most 720 |
| Bubble and card width in the timeline (`layout.bubble_max_width`) | At most 560 |
| Space above the first item at rest, below the mark and the pill (`layout.content_top`) | 92 |
| Top tint (`layout.top_tint`): canvas opacity at 0, 60, 86 and 108 px | 97, 94, 80, 0 percent |
| Bottom fade, above the composer (`layout.fade_bottom`) | 32 |
| Smallest window | 960 by 600 |

The timeline is a chat of bubbles. See [The chat](#the-chat). It sticks to the bottom, next to the composer. Older messages scroll up.

On a phone:

- A top bar replaces the sidebar. Its left side switches channels. Its right side opens the Tasks screen and shows how many tasks need you.
- Every control that you tap is at least 44 by 44 px. Approval buttons fill the width of the card.
- The composer is a pill with `full` corners. The microphone sits at its right end.
- Lists have no selected row.

Settings use a fixed layout: the settings sidebar, then a label column of 160 px and the controls to its right. Dialogs are 520 to 560 px wide with `xl` corners and `shadow.float`, over a black scrim at 55 percent in dark mode. The Add account dialog is 600 px wide (`layout.dialog_account`), because its provider grid has two cards in a row.

## The chat

The main column has no header. The timeline fills it, the composer sits under it, and the Three voices mark and the top actions float over its top fade. The values come from the Paper artboard "Tutti, bubbles, dark".

- **Lanes.** The assistant's messages are bubbles at the left on `surface.card`. Your messages are bubbles at the right on `accent.surface`. A bubble is at most `layout.bubble_max_width` (560 px) wide and has `chat.bubble_radius` (16 px) corners, with `chat.bubble_pad_y` by `chat.bubble_pad_x` padding. Message text uses Body.
- **Groups.** Bubbles that follow each other in one lane form a group, `xs` (4 px) apart. Groups are `lg` (16 px) apart. The last bubble of a group has a `chat.bubble_tail` (4 px) corner on the speaker's side: bottom left for the assistant, bottom right for you. A time separator, a card or a notice ends a group.
- **Paragraphs.** A message from the assistant becomes one bubble for each paragraph (`consort_client::chat::split_paragraphs`). A paragraph or a heading starts a bubble, except right after a heading. A list, a code block, a table, a quote or a rule stays whole in the bubble before it. Your own message stays one bubble.
- **A brief** (a message with a title and task references) has a title bubble in semibold, then its text. The task references are a numbered list in the last bubble: a chip with the status and the task name that opens the thread, the channel in `text.tertiary`, and the line about the task.
- **Time separators** are centered, in Label size and `text.tertiary`. The first item of a day gets the day and the time ("Today 8:00 AM", "October 12, 9:00 AM"). A later item gets the time alone ("9:12 AM") when it comes 30 minutes or more after the item before it (`chat::SEPARATOR_GAP_MS`). There is no time on each message. A message on its way says "Sending" under its last bubble, and an edited message says "Edited".
- **Cards.** Task cards, approval cards and decision cards sit in the assistant lane at bubble width, with `chat.bubble_radius` corners and a `chat.bubble_tail` corner at the top left. Above each one is a task line: a curved connector (`glyphs/task-connector.svg`, `chat.task_connector`, `border.strong`), the status icon, the task name in UI strong, and its state in the status color ("Needs you" for Needs input). A task card in another channel adds "in <channel>" in `text.tertiary`. A card has no header row of its own: its first row is what asks or the newest update, with the approval mode and the worker pill at the right. A resolved task with nothing pending is the task line alone, with its summary. An approval or a decision of the channel's assistant has the task line "Approval" or "Decision", with "Needs you", "Answered" or "Closed".
- **Notices** are a centered line in UI size and `text.tertiary`, with their time.
- **The Three voices mark** sits at the top center, `chat.mark` (56 px), `chat.mark_top` from the top. Under it, an activity pill (`surface.card`, `border.subtle`, `full` corners, `chat.pill_pad_y` by `chat.pill_pad_x`, Label size in `text.secondary`) says what the assistant does, only while it works. See [Logo](#logo).
- **Top actions** sit at the top right, `md` from the edges, `xs` apart: Connectors (for a channel that has connections or skills, with their count), Channel (the `channel` hash icon, which opens the Channel panel), Tasks and New task. Each is `chat.top_action` (32 px) square with a `chat.top_action_icon` (18 px) icon in `text.secondary`. Tasks opens the Tasks panel, has `surface.selected` while the panel is open, and has a `component.state_dot` dot in `accent.fill` while something needs you.
- **Top tint.** The top of the timeline darkens progressively, as in Claude Duet. It is no solid zone: content that moves up under the mark stays faintly visible. `layout.top_tint` sets the canvas color at four stops: 97 percent at the top, 94 percent at the bottom of the mark (60 px), 80 percent at the bottom of the pill (86 px), and clear at 108 px. Content under the mark and the pill is only a faint trace, as the user asked on 2026-10-29 ("once you get to the avatar position, it needs to darken significantly"). A GPUI gradient has two stops, so the view draws one gradient layer between each pair of stops (three layers). The mark, the activity pill and the top actions sit on the tint. At rest, the first item starts `layout.content_top` (98 px) from the top, just below the mark and the pill (mark top, mark, `space.xxs`, the pill, and a small margin). A theme test keeps it tall enough. It stays the same when the pill is hidden, so the chat does not jump when work starts or stops.
- **Bottom fade.** `layout.fade_bottom` (32 px) above the composer, shown only while the reader is scrolled up.
- **Composer.** One field on `surface.card` with a `border.strong` line and `chat.composer_radius` (16 px) corners, as wide as the timeline. The send key (`send` icon, `chat.send_button`, `text.tertiary`) sits inside it at the right. Under the field, outside it: add and voice (`component.composer_chip` square), a `border.strong` divider, "Same as Tutti" for a channel with no model of its own, the model chip and the effort chip, a spacer, and "Runs on <worker>" in Label size and `text.tertiary`. The placeholder is "Message <channel>", and "Message Tutti" in the main channel.

### Chat sizes

These sizes are in `component.chat` of `structure.json`, except the three in `layout`.

| Token | Size (px) | Use |
|---|---|---|
| `layout.bubble_max_width` | 560 | The widest bubble or card in the timeline |
| `layout.content_top`, `layout.fade_bottom` | 92, 32 | The space above the first item at rest, and the bottom fade of the timeline |
| `bubble_radius`, `bubble_tail` | 16, 4 | Bubble and card corners, and the small corner |
| `bubble_pad_x`, `bubble_pad_y` | 14, 10 | The padding of a bubble |
| `composer_radius`, `send_button` | 16, 32 | The composer field and its send key |
| `top_action`, `top_action_icon` | 32, 18 | A top action and its icon |
| `task_connector` | 14 | The curved connector of a task line |
| `mark`, `mark_small`, `mark_top` | 56, 16, 4 | The mark over the timeline, in the sidebar, and its top space |
| `pill_pad_x`, `pill_pad_y` | 10, 3 | The padding of the activity pill |
| `brief_number` | 16 | The number column of a brief's list |

### The sidebar

The sidebar and the settings rail use `component.sidebar` in `structure.json`. The values come from the Paper artboard "Base: Tutti with top tint and larger sidebar, dark".

| Token | Size (px) | Use |
|---|---|---|
| `layout.sidebar_width` | 288 | The sidebar and the settings rail |
| `inset` | 10 | The space between the sidebar edges and its rows |
| `row_pad_y`, `row_pad_x`, `row_gap` | 7, 10, 10 | A row: 36 px high with its text, `lg` corners, also on the selected row |
| `row_text`, `row_icon` | 15 / 22, 18 | The name and the icon of a row, and the status glyph |
| `label_text`, `label_pad_y`, `group_top` | 13 / 18 weight 500, 6, 20 | A group label, the Workers label and a settings group label |
| `title_bar` | 52 | The strip at the top where the macOS window buttons sit |
| `worker_text`, `worker_pad_y` | 14 / 20, 5 | A worker row: 30 px |
| `avatar`, `account_pad_top`, `account_pad_bottom` | 28, 10, 6 | The account row |

### Attachments

The sizes are in `component.files` of `structure.json`. The values come from the phase 1 Paper artboards.

- **The add button** in the composer and the reply box opens a menu: Upload from this computer (the system file dialog) and Choose from channel files. A pasted picture or copied file also attaches.
- **Chips** sit in the field, above the text, and wrap. A picture is a `chip_image` (56 px) thumbnail with `lg` corners and a round remove badge on hover. Any other file is a `chip_file` (240 px) chip on `surface.raised` with a `chip_tile` (38 px) tile, the name, and the type and size ("PDF, 2.4 MB"). An upload shows a `progress` (4 px) bar in `status.working` on a track at `opacity.progress_track`, and its share ("62%"). A failed upload has a `status.failed` border at `opacity.failed_border`, a tinted tile, the message ("Upload failed. The hub is offline.") as selectable text, and Try again.
- **Send waits** until every upload of the message is on the hub. A failed upload stops the send, and the text stays in the box.
- **Drop.** Files dragged anywhere over the chat attach to the composer. The composer is the only visual target: it grows, gets a dashed `accent.text` border and an `accent.fill` fill at `opacity.drop_fill`, and shows the upload icon on a `tint_accent` disc, "Drop to attach 3 files" and "Up to 256 MB each". Text that you typed stays visible.
- **Bubbles.** Your pictures are `image_tile_w` by `image_tile_h` (228 by 168 px) tiles, two in a row, above your text. Your files are `file_bubble_you` (280 px) file bubbles on `accent.surface`. The assistant's files follow its text as `file_bubble` (360 px) bubbles on `surface.card`. An artifact names its task with the status glyph, and its size. On hover a file bubble shows Save and Open.
- **Light box.** A click on a picture opens it large on `lightbox` (black at `derive.lightbox_alpha`), with its name and details, Save, Open, close, Previous and Next, and "1 of 2". The arrow keys step, and Escape closes it.

### The Channel panel

The `#` top action opens the Channel panel. It shares the right side with the Tasks panel: only one of them is open at a time. It is 400 px wide, with the channel name and purpose in its header, and the tabs Files, Routines and Memory, each with its count. A routine or a memory note opens in place of the tabs, with a back button in the header.

The Files tab lists folders first, then files, then a section Artifacts with one group per task (the status glyph, the task name and its status). Each row has the size in a `size_col` (64 px) column and the date ("Today" or "Oct 24") in a `date_col` (52 px) column. The row menu is Open, Download, Rename, Move, then Delete in `status.failed`, and Delete asks first. Upload and a drop on the panel put files in the open folder. An empty channel shows its empty state, which is also a drop zone. The list scrolls inside the panel with fades.

### Routines and memory

The sizes are in `component.knowledge` of `structure.json`. The values come from the phase 2 Paper artboards.

- **A routine row** has the `routine` icon, three lines and a small switch: the name, the schedule in words with the action ("Every day at 17:30, asks the assistant"), and a `status_dot` (6 px) with the last result and the next run. The dot is `status.done` for a run that worked, `status.failed` for a failure (the result is `status.failed` too), `status.working` while a run runs, and a ring for a routine that is off ("Off. It does not run until you turn it on."). On hover the row is `surface.card` and shows Run now, Run history and the row menu. Its menu is Run now, Run history, Edit, Turn off (Turn on), then Delete in `status.failed`.
- **The small switch** is `switch_w` by `switch_h` (32 by 18 px) with a `switch_thumb` (14 px) thumb: `accent.text` with an `accent.on_fill` thumb when on, `border.strong` with a `text.tertiary` thumb when off.
- **A routine opened in the panel** shows its summary with a `summary_label` (80 px) label column, Run now and Edit, and the run history of the last 30 days. A run row has the result glyph, the time (with "Run now" for a manual run), the result word, a `run_duration` (64 px) column and a `run_link` (76 px) column with Message or Task. A failed run is on `surface.card` and shows its error as selectable text.
- **The routine dialog** is `layout.dialog_wide` (560 px). Name and a `channel_field` (190 px) Channel select share a row. The schedule presets are round chips: `accent.text` border and text on `tint.accent` when chosen. The time field is `time_field` (84 px), the interval field `every_field` (72 px) with a `unit_field` (120 px) select, and a Weekly day is a `day_button` (50 px). The time zone select opens a `zone_popover` (340 px) with a search and a `zone_list` (280 px) list, and the date field opens a calendar of `calendar_cell` (32 px) days. Each error sits under its field in `status.failed` with the `failed` glyph. Save stays off while a field has an error.
- **The memory tree** indents one `tree_indent` (22 px) per folder level. Note names are mono. A note that the assistant just changed has a `status_dot` in `accent.text` and "Just now".
- **The note editor** is mono on `surface.canvas`, at least `editor_min_height` (320 px). The stale-write message is on `tint.failed` with a `status.failed` border at `opacity.failed_border`. It keeps the text and offers Load the new version and Save my version.
- **The version history** has a `version_list` (240 px) list and a preview. A changed line has a `change_bar` (3 px) bar in `status.working` on `tint.working`, not red or green, because a change is not an error.
- **The memory saved line** sits in the assistant lane: the `memory` icon at `line_icon` (14 px), the words in `text.tertiary` and the note path in mono as a link.

Settings > Workers uses these sizes from `component` in `structure.json`:

| Token | Size (px) | Use |
|---|---|---|
| `tag_label` | 92 | The label column of the Tags and Detected rows |
| `provider_logo` | 18 | A provider logo in an account row, a tool row or a provider card |
| `account_state` | 112 | The status column of an account row: a dot and a word |
| `account_action` | 96 | The action column of an account row: Sign out, Sign in, Sign in again |
| `tool_version` | 112 | The version column of a tool row, in mono |
| `tool_state` | 150 | The state column of a tool row: Up to date, Update to X, Install now |
| `agent_tile` | 32 | The tile around the 16 px icon of an ACP registry agent |
| `agent_list_max` | 152 | The ACP registry list shows three rows, then scrolls |
| `sign_in_tile`, `sign_in_logo` | 40, 22 | The logo tile before the title of the sign-in step |
| `step_marker` | 22 | The round marker of a numbered step in a sign-in flow |

The model picker uses these sizes from `component`:

| Token | Size (px) | Use |
|---|---|---|
| `model_picker` | 440 | The width of the model picker popover, with its rail |
| `model_picker_height` | 420 | The height of the model picker popover. The model list scrolls inside it |
| `picker_rail` | 48 | The width of the rail at the left of the picker |
| `picker_rail_button` | 36 | A square button in the rail: Favorites or an account |
| `picker_rail_badge` | 14 | The height and the least width of the badge on an account button |
| `picker_rail_badge_text` | 9 | The text size of that badge |
| `picker_rail_marker`, `picker_rail_marker_height` | 3, 20 | The accent bar that marks the selected rail button |
| `effort_menu` | 240 | The width of the effort menu |
| `effort_select` | 150 | The effort select next to a model field in a dialog |
| `composer_chip` | 28 | The height of the model chip and the effort chip in the composer |

Connectors and skills use these sizes. The Add connector and Add skill dialogs are `layout.dialog_catalog` (760 px) wide. Their list scrolls inside the dialog and fades at its edges.

| Token | Size (px) | Use |
|---|---|---|
| `connector_tile` | 44 | The tile around a service logo at the top of a connector page and of the Connect dialog |
| `connection_field` | 240 | The Name field in a row of a connector page |
| `catalog_list` | 310 | The height of the service grid in the Add connector dialog |
| `skill_results` | 300 | The height of the result list in the Add skill dialog |
| `usage_bar` | 4 | The Composio usage bar under the connector list |
| `files_list`, `files_view` | 200, 360 | The file list and the height of the skill files dialog |

A service logo is the image that Composio gives. The client loads it (`https` only, at most 256 KiB, SVG, PNG, JPEG, WebP or GIF). Until it loads, and when it does not, the logo is a monogram: the first letter of the service on `surface.selected`.

### The assistant on its own

The values come from the Paper artboards `FYE-0`, `G2C-0`, `G8V-0`, `F6N-0`, `FDX-0` and `FJC-0`, through `component.proactivity` in `structure.json`.

- **Settings > Proactivity** is a settings page of `settings_detail_max`. Its parts, in order: When the assistant may start things (Pause all, Quiet hours, Snooze, Follow-ups), Each channel, Today, and Activity. The settings rail item is "Proactivity" with the `eye` icon, after Routines.
- **The quiet hours** are two `time_box` (72 px) boxes in mono with no chevron, and a `zone_select` (168 px) time zone select.
- **Each channel** shows its icon, its name, the meaning of its level, and a `level_select` (112 px) select with no color. A channel at Off says "The assistant only answers here." The channel settings dialog reuses the same select, wider, with "Default for this channel" and a helper line.
- **Today** has two numbers in Heading and one `count_bar` (4 px) bar in `status.working` on `surface.selected`. A full bar is not an error.
- **The log** has four lanes: `log_time` (68 px), `log_place` (112 px, the channel and the kind), `log_result` (176 px) and the detail. A result is in words: Notified in `accent.text`, Failed in `status.failed`, Posted in `text.primary`, every skip and Stayed silent in `text.tertiary`. "Open message" is an underlined link. The filters are two `filter_select` (146 px) selects.
- **A proactive mark** is a caption above the first bubble of the group: a `mark_icon` (14 px) icon, `mark_gap` (6 px), the kind in Label and `text.tertiary`. A follow-up's reason sits under it, indented `reason_indent` (20 px). The icons: `eye` for a check-in, `sun` for the morning brief, `clock` for a follow-up, `compose` for an offer.
- **Useful and Too noisy** show under the bubble on hover, in Label with `feedback_pad_x` by `feedback_pad_y` (8 by 2 px) padding, no border; hover and the chosen rating are on `surface.selected`.
- **The offer card** is a bubble-shaped card on `surface.card` with a `border.subtle` line, `offer_pad_top` (12 px) and `offer_pad` (14 px) padding: the question in Body at the title weight, the context in UI and `text.secondary`, then Do it (primary), Not now (secondary) and Never about this (quiet).
- **The morning brief** is one bubble: the first line, then the group labels in Label (Needs you and Ready for review in `accent.text`, Done since yesterday in `text.tertiary`), and numbered items whose name is an underlined link.
- **Quick-reply chips** are pills on `surface.raised` with a `border.strong` line and `chip_pad_x` by `chip_pad_y` (14 by 5 px) padding, `chip_gap` (6 px) apart. "I will type it" has a dashed border and no fill.
- **The writing bubble** is a bubble with `writing_pad_x` by `writing_pad_y` (16 by 13 px) padding and three `dab` (10 px) dabs of the mark, `dab_gap` (5 px) apart: gold `dab_drop` (2 px) low, verdigris `dab_rise` (3 px) high, lapis 1 px low. The dabs are `assets/glyphs/dab-*.svg`, in three hand-drawn versions each, and shimmer on the shared mark clock; they stand still with Reduce motion.
- **The Routines tab** shows the check-in first, with the `eye` icon, "Every 6 hours, uses `check-in.md`" and its last result from the log ("Stayed silent at 7:25."). Its hover actions are Run now, Run history and Open note, and its menu has no Edit. The Follow-ups section lists each follow-up with the `clock` icon, its reason as the title and its time ("Today at 15:00, set this morning"), Cancel on hover, and Cancel all.

### Write in my voice

The values come from the Paper artboards `H3V-0`, `H7T-0`, `HL7-0` and `HTB-0`, through `component.voice` in `structure.json`.

- **Settings > Voice** is a settings page of `settings_detail_max`, after Memory in the settings rail, with the `quote` icon (the `voice` icon is the dictation microphone). With the voice off: the switch, "Where Consort learns from", "What the voice never changes", and "Turn on and learn". With the voice on: the switch and the learned line with Change sources and Learn again, the style guide, Try it, Disclosure, and Turn off and Reset. Reset is a secondary button with its text in `status.failed`, and it asks first in a confirm dialog.
- **A source row** has a `check_box` (16 px) checkbox (the accent fill with a `check_icon` (12 px) check when ticked), a `source_tile` (32 px) tile on `surface.selected` with the monogram or the `file` or `edit` icon, and the title and lines.
- **The style guide** is one bordered card per section: the title, the subtitle in `text.tertiary` and "Learned from ..." at the right. A rule is a row with `rule_pad_y` (8 px) padding and a `rule_dot` (4 px) dot in `text.tertiary`. An example is a block on `surface.card` with `example_pad_y` (10 px) padding. Remove shows on hover in a fixed `remove_slot` (84 px) lane, and the hovered row is on `surface.card` (an example on `surface.selected`). "(you)" is in `accent.text`.
- **Try it** shows the sample on `surface.card` with the voice line, and "Nothing is sent" after a `sample_dot` (7 px) dot in `status.done`.
- **Disclosure** is three radio options: a `check_box` ring with a `radio_dot` (8 px) dot in `accent.text`.
- **The draft card** keeps the connector card's frame. The fields have a `field_label` (52 px) label lane. A flagged phrase in the text is underlined in `accent.text`. The voice line has the `quote` icon at `line_icon` (12 px) and says "Written in your voice (Formal email)" or "Neutral style". Each flag has a `flag_bar` (2 px) bar in `accent.text`, the phrase in UI strong, its kind in `text.tertiary`, and its note in `accent.text`. "I checked the marked lines" keeps Allow off (`surface.selected` and `text.tertiary`) until the tick. Then Edit (secondary), Don't (quiet), and "Not how I write" as quiet Label text at the right. In Edit, Subject is a `subject_box` (30 px) field and the text is a field of at least `body_box` (150 px) with the `accent.text` line; the flags and the tick leave until Save changes.
- **The voice task card** shows its finished steps with `check` icons in `status.done` in a box on `surface.raised`, and the line "Raw text stays in this task. ..." with Open thread. The offer "Save this style guide?" has the link "Open the full guide" with the `external-link` icon.

### Hub, Clients and Notifications

Settings pages that are not lists (Hub, Clients, Notifications) use one column of `settings_detail_max` (760 px), with section titles in Body at the title weight and bordered row groups (`border.subtle`, `lg` corners, rows with 12 by 16 px padding). The values come from the Paper artboards `7JP-0`, `86L-0`, `8L0-0` and `8XN-0`, through `component.admin` in `structure.json`:

| Token | Size (px) | Use |
|---|---|---|
| `storage_bar`, `legend_swatch`, `legend_gap` | 6, 8, 28 | The storage bar of Settings > Hub and its legend. The bar shows only Consort's own use, split into database, blobs and skills in `text.primary`, `text.tertiary` and `text.secondary`. Free disk space is a line of text. Status colors stay for state |
| `time_select` | 120 | The time select (and the day select of a weekly schedule) of the backup schedule. The Keep select is `settings_select` |
| `status_col`, `action_col` | 150, 72 | The status column (a dot and "Online now" or "Seen 2 hours ago") and the Revoke column of a client row |
| `client_tile`, `client_icon`, `row_gap` | 32, 16, 14 | The tile at the left of a client row |
| `confirm_indent` | 62 | The inline Revoke confirm starts under the client name. Its row is on `tint.failed`, with Cancel and a `status.failed` Revoke. The app that you use has no Revoke |
| `code_text`, `code_pad_x` | 34/42, 18 | The setup code of the Pair a new app dialog, in mono |
| `qr`, `qr_pad` | 148, 8 | The QR code: dark modules on a light ground in both modes |
| `countdown_bar` | 3 | The bar under the code that empties over the 10 minutes |
| `copy_button`, `copy_small` | 28, 26 | The copy button of an address row and of the address field |
| `dock_tile`, `dock_mark`, `dock_badge` | 40, 24, 18 | The Dock icon preview with its count on `status.failed` |
| `banner_width`, `banner_tile`, `banner_mark` | 360, 32, 20 | The example banner of Settings > Notifications |

### The hub is offline

Paper `ERQ-0`. The app opens with its saved data, and the same state shows when the hub goes away while the app runs.

- The status pill under the mark says "Cannot reach the hub. You see saved data from 9:41 AM." (a day name when the data is older than today) and has Try again. It uses the activity pill's size and `surface.card`. The mark rests at `opacity.offline_mark` (0.7).
- Workers: the group label says "Status unknown", and every dot is hollow.
- Approval and decision buttons use `opacity.offline_dim` (0.45), and a tooltip says "Waiting for the hub. You can answer when it is back." A click shows the same text in a notice.
- A message that waits has `opacity.queued` (0.75) and the line "Sends when the hub is back" with the working glyph. The composer line says "Messages wait here until the hub is back", in place of "Runs on <worker>".

### Conversation and tasks

The values of phase 4 come from the Paper artboards `95W-0` to `FWA-0` through `component.convo` in `structure.json`:

| Token | Size (px) | Use |
|---|---|---|
| `reaction_chip`, `reaction_pad_x`, `reaction_gap`, `reaction_emoji` | 24, 8, 4, 13/16 | A reaction chip under a bubble. Yours is on `tint.accent` with an `accent.text` border, the assistant's on `surface.card` with `border.subtle`. A count shows only when both chose the emoji |
| `quick_emoji`, `quick_emoji_text`, `menu_width` | 30, 16/20, 248 | The six quick emoji and More at the top of the message menu. A chosen one has the accent ring, and a second click removes it |
| `emoji_picker_width`, `emoji_cell`, `emoji_picker_height`, `emoji_cell_text` | 320, 36, 300, 20/24 | The emoji picker behind More, grouped, on `surface.raised` |
| `root_ring` | 1 | The `accent.text` ring around every bubble of a message whose thread is open, as an open task card has |
| `panel_wide`, `line_number_col` | 560, 44 | The task panel while it shows one item in full, and the line numbers of output and of each side of a diff. Added lines are on `tint.done`, removed lines on `tint.failed`, hunk headers on `tint.scheduled` |
| `search_width`, `search_row`, `search_results_max` | 640, 36, 460 | The search palette (Command K) and its Go to rows. Matches are bold in `text.primary`, never in the accent |
| `skeleton_dot`, `skeleton_bar`, `skeleton_title`, `skeleton_line` | 18, 8, 120, 440 | The skeleton rows while the hub searches, on `surface.card` |
| `dialog_settings`, `settings_column_gap` | 880, 28 | The channel settings in two columns with a line between them, and the review of a new skill version |
| `dialog_small`, `dialog_group`, `dialog_about` | 460, 400, 440 | New channel, the group dialogs, and About |
| `icon_choice` | 38 | An icon choice of New channel and the channel settings |
| `about_mark`, `wordmark` | 80, 28/34 700 | The Three voices mark and the wordmark in About |
| `brief_box`, `instructions_box` | 96, 100 | The least height of the brief of New task and the instructions of the channel settings |
| `drag_line` | 2 | The `accent.text` line above a group where a dragged group goes |

The task thread is bubbles, as the timeline: the brief is the first bubbles with the placement as a caption under them, the activity is a chip that opens to the commands and file changes (each opens in full), and approvals and decisions have a task line ("Approval", "Needs you") above the card.

## Provider logos

Every provider shows with its logo, never as text alone. The logos are in `crates/consort-desktop/assets/providers/` (see the NOTICE there).

- Claude Code and Gemini CLI keep their own colors.
- Codex, OpenCode and the ACP registry icons draw in the color of the text next to them, like an icon. Registry icons are SVGs that draw in `currentColor`. The client loads them from the registry (`https` only, at most 64 KiB, SVG only), and the app draws them like an icon.
- A provider with no logo, such as a custom command, or a registry icon that did not load, gets the `bot` glyph. It is the one Lucide drawing left, because it stands for a provider logo.
- A provider that is not signed in or not installed shows its logo at 60 percent opacity, with its name in `text.secondary`.

## Model picker

One component chooses a model everywhere: the composer, the New channel dialog, and Model in the channel menu.

- **Trigger.** The provider logo, the model name in medium weight, and the account name in `text.tertiary`, then a chevron. In the composer it is a chip, `component.composer_chip` (28 px) high, with `md` corners and no ground until hover. While its popover is open, the chip has `surface.selected`. In a dialog it is a select field, as wide as its column. A dialog field with no model of its own says "Same as Tutti", with Tutti's model in `text.tertiary`. A channel that uses Tutti's model shows "Same as Tutti" before the chip in the composer.
- **Popover.** `component.model_picker` (440 px) wide and `component.model_picker_height` (420 px) high, `surface.raised`, `lg` corners, `border.strong`, `shadow.float`. A rail is at the left, and the main pane fills the rest.
- **Rail.** `component.picker_rail` (48 px) wide, on `surface.card`, with a `border.subtle` line on its right. From the top: Favorites (a filled star in `accent.text`), a `border.strong` divider, then one button per signed-in account name, in the order of the inventory, with its provider logo at `component.provider_logo`. Each button is `component.picker_rail_button` (36 px) square with `md` corners. The selected button has `surface.selected` and an `accent.text` bar of `component.picker_rail_marker` by `component.picker_rail_marker_height` on the rail's right edge. The pointer on a button shows a tooltip with the account name and its workers (or Favorites and "From every account"). The rail scrolls when it has more accounts than room.
- **Badges.** When two or more accounts have the same provider, each of their buttons has a badge at its bottom right: a pill of `component.picker_rail_badge` on `surface.raised` with a `border.strong` line, in semibold text of `component.picker_rail_badge_text`. The text is `text.primary` on the selected button and `text.secondary` on the others. The label comes from the account names (`picker::badge_labels`): keep the words that no other account of that provider has, after a number joins the word before it. Take the first letter of each kept word in upper case, plus the digits at the end of the word, and cut it to 3 characters. So "Claude Code Personal 1" and "Claude Code Personal 2" give P1 and P2, and "Claude Work" next to them gives W. An account with no word of its own, or two accounts with the same label, get their position (1, 2).
- **Search.** At the top of the main pane: a search icon and the field "Search every account". Typing filters by model name, model ID, account name and provider name, hides the rail, and shows the matches from every account as rows of the Favorites kind. Escape clears the search first, and the rail comes back. A second Escape closes only the popover.
- **Header line.** Under the search field, outside the list, so it stays there while the list scrolls (this is the sticky header of a long account). It has a title in UI size and semibold, and a detail in caption style and `text.tertiary`: the account name and its workers, "Favorites" and "From every account", or "Search results" and "From every account". A channel with its own model has the text button Use the Tutti model at the right end of the line, in every view.
- **Account rows.** The model name, the effort range in caption style (such as "Low to Max effort"), a star and a check. The selected row has `surface.selected`, the name in medium weight, and an `accent.text` check.
- **Favorites and search rows.** The provider logo at 14 px, the model name, then the account name in caption style and `text.tertiary`, then the star and the check. The Favorites list ends with the note "Star a model in any account to add it here.", which is also its empty state.
- **Stars.** A favorite has a filled star in `accent.text`. Another row shows an outline star in `text.secondary` only while the pointer or the keyboard is on the row. A click on the star adds or removes the favorite. It does not choose the model or close the popover.
- **First view.** The picker opens on Favorites when at least one favorite has a row, else on the account of the current model, with the keyboard on the current model.
- **Keyboard.** Up and Down move in the list. Left (with an empty search) moves to the rail, where Up and Down change the view and the selected button shows the focus ring. Right, Enter or typing go back to the list. Enter in the list chooses.
- **Scroll.** The list scrolls inside the popover. It fades into `surface.raised` at the top and the bottom while more rows are there.
- **No account.** "No account is signed in. Add one in Settings > Workers." with the button Open Settings. The popover then has no rail, and it is as high as its content.
- **Effort.** A second chip next to the model chip: the `effort` icon and the level. Its menu is `component.effort_menu` (240 px) wide, with the title "Effort for <model>", one row per level, "Default" after the provider's default level, and the note "<Provider> offers these levels for this model. More effort thinks longer and uses more of your plan." When the provider does not name its default, the first row is Default, which takes the effort out. The chip is not there for a model with no effort levels. In a dialog, the effort is a select of `component.effort_select` (150 px) next to the model field. A new model keeps the effort when it offers that level. Otherwise the effort goes back to the new model's default.
- **Task threads** show the task's model under the reply box as text, such as "Runs with Opus 5.5, Claude Personal, High effort". A task keeps the model it started with.

## Worker tags

- A user tag is a chip with `sm` corners and a `border.strong` border, with an x to remove it. A user tag that a channel requires shows in the accent: `tint.accent` ground, `accent.text` border and text. Add tag is a chip with a dashed border.
- A detected tag is mono caption text on `surface.card`, with no border. The user cannot remove it.
- In the New channel dialog and the Worker tags dialog of a channel, a chosen tag is an accent pill with `full` corners. Add tag opens a menu of every worker tag: the user tags, a line, then the detected tags. The note under the field says how many workers match, and turns `status.failed` when none does.

## Corners

| Token | Radius (px) | Use |
|---|---|---|
| `sm` | 4 | Badges, inline code and keyboard keys |
| `md` | 6 | Buttons, inputs and list rows. Sidebar rows use `lg` |
| `lg` | 10 | Cards outside the chat, menus and popovers |
| `xl` | 14 | Panels, dialogs and the app icon at 64 px |
| `full` | 9999 | Avatars, status dots and pills |

When one rounded box sits inside another, the inner radius is the outer radius minus the padding between them. A card with `lg` (10 px) corners and 4 px padding holds a box with 6 px corners.

## Borders and elevation

- Every border is 1 px, in `border.subtle` or `border.strong`.
- The five surface steps show depth: sidebar, canvas, card, raised, selected.
- Only menus, popovers, dialogs and toasts cast a shadow. They use `shadow.float` from `structure.json`. Cards and panels never cast a shadow.

## Motion

| Duration | Use |
|---|---|
| 120 ms | Hover and press |
| 180 ms | Open and close a menu, a popover or a card |
| 280 ms | Move a side panel in or out |

Elements that enter use the curve `cubic-bezier(0.2, 0, 0, 1)`. Elements that leave use `cubic-bezier(0.4, 0, 1, 1)`. These durations match GPUI Kit's motion tokens, so kit components and Consort components move the same way.

Stop motion runs at 8 frames a second (`motion.frame_ms`, 125 ms) on one shared clock. Three things use it:

- The Three voices mark while it works or needs you.
- A live status icon (Working or Needs input): it shows its three hand-drawn versions in turn.
- An icon button under the pointer: it shows its three versions in turn, only while the pointer is on it.

The clock runs only while a draw showed one of these and the window is active. When the only thing that moves is the idle mark at the top center, the clock ticks at half speed (4 fps). With Reduce motion on, nothing draws on a timer.

**Reduce motion** is a switch in Settings > Appearance. It starts from the user's choice (saved in `appearance.json`). Until the user sets it, it starts from the system setting where the app can read it (on macOS, `defaults read com.apple.universalaccess reduceMotion`), else off. With it on, the mark changes pose only when its state changes, nothing shimmers, and elements fade in 120 ms.

## Icons and glyphs

- Icons come from Consort's own hand-drawn set in `design/icons/` (84 icons on a 24 px grid, drawn with a 1.9 px stroke, in `currentColor`). They show at 16 px. Dense lists use 14 px, and toolbars use 18 or 20 px.
- Each icon has three hand-drawn versions: `assets/icons/<name>.svg`, `assets/icons/v1/<name>.svg` and `assets/icons/v2/<name>.svg`. `node design/icons/build.mjs <node_modules>` makes them.
- The hub keeps a channel's icon as a Lucide name, such as `hash` or `mic`. The app shows the nearest icon of the set (`ui::icons::resolve`).
- Consort's own glyphs in `design/glyphs/` are the task-line connector and the Up next and Resolved statuses.
- An icon takes the color of the text next to it, except a status icon, which takes its status color.

## Logo

The logo is the Three voices mark: three dabs of pigment, one for each voice of a broken consort. Gold is the accent, then verdigris and lapis. Where they overlap, the paint mixes lighter (screen) on a dark ground and darker (multiply) on a light ground, like a chord.

- **Frames.** GPUI cannot run an SVG noise filter at every frame, so `design/mark/generate.mjs` renders every frame ahead of time into `assets/mark/`: idle (3 frames: one pose, three edges), working (12 poses, each with its hand-drawn edge), needs you (24 frames), and done (1 frame). Each set is in dark pigments (`dark/`) and light pigments (`light/`). GPUI's image renderer (resvg) draws the paint filter and the blends.
- **Small sizes.** At 16 to 20 px, use the sharp frames (`dark-sharp/`, `light-sharp/`), which have no paint edges.
- **States.** Idle keeps one pose and its edges shimmer at 4 fps, so the mark looks alive at rest. Working: the voices circle each other. Needs you: gold comes forward and beats twice a second. Done: the voices close into a chord for `motion.done_hold_ms` (1.5 s) after work ends, then the mark goes back to idle or to needs you.
- **Where.** The mark is the logo on onboarding and on the start-up error, the Tutti row in the sidebar (idle, sharp, `component.sidebar.row_icon`, 18 px, always still) and in the switcher (16 px), and the title bar on Windows and Linux. The app icons in `design/logo/` are the idle mark on `surface.card`.
- **Wordmark.** "consort" in lowercase, Atkinson Hyperlegible Next 600, to the right of the mark. The mark is about 1.2 times the font size high.

## Text in the UI

- Buttons say what happens, with a verb: **Allow once**, **Deny**, **Resolve**, **Start task**. An action keeps its name through a flow, so **Resolve** leads to "Resolved".
- Do not join details with middle dots. Use a comma, or put the details on separate lines.
- Do not add arrows to button or link text.
- An error says what happened and what to do next. An empty state names the situation and the next action.
- Write in the style of ASD-STE100: short sentences, one instruction per sentence, and the same word for the same thing everywhere.
