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
| `chart.1` to `chart.6` | The fills and strokes of a chart in a visual. They are the pigments of the Three voices mark (lapis, verdigris, gold, cobalt violet and madder), then slate for "Other". Gold is not first, because the accent means "your turn". Never on `surface.card` |
| `chart.ink` | Text on a chart fill, such as a number in a bar |

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
| `chart.1` | `#77B4ED` | `#2F74C0` |
| `chart.2` | `#80C7A8` | `#2E8466` |
| `chart.3` | `#E7BE62` | `#B08419` |
| `chart.4` | `#B2A0D6` | `#7559A6` |
| `chart.5` | `#E3929C` | `#B9506A` |
| `chart.6` | `#8E9A94` | `#7A8781` |
| `chart.ink` | `#131E1A` | `#FFFFFF` |

The names come from the materials of a consort: spruce for the surfaces, lead white for the dark-mode text, brass for the accent, and Renaissance pigments for the statuses (azurite, verdigris, vermilion and folium). The values were set in OKLCH and converted to sRGB.

### Velvet values

Velvet has the same chart pigments as Spruce, except slate and the ink. The other roles are in `design/themes/velvet.json`.

| Role | Dark | Light |
|---|---|---|
| `chart.1` to `chart.5` | The values of Spruce | The values of Spruce |
| `chart.6` | `#968E97` | `#857B85` |
| `chart.ink` | `#1F181F` | `#FFFFFF` |

A theme file with no `chart` group loads. It gets the chart roles of Spruce in the same mode.

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
| `chart.1` to `chart.6` | `surface.canvas` (a graphic on the page background, WCAG 1.4.11) | 3.0 |
| `chart.ink` | `chart.1` to `chart.6` (bold numbers in a bar) | 3.0 |

Spruce and Velvet pass every rule in both modes. The lowest text ratio is 4.51, for Spruce light. The lowest chart ratio is 3.29, for `chart.3` on the canvas in light mode. Gold on `surface.card` in light mode is 2.98, so a chart never sits on a card fill.

### Task status

Each status has one glyph and one color, everywhere: the sidebar, task cards, the Tasks panel, notifications and the mobile app.

| Status | Glyph | Color role |
|---|---|---|
| Up next | glyph `status-up-next.svg` (dashed ring) | `text.tertiary` |
| Scheduled | icon `routine` (the metronome) | `status.scheduled` |
| Working | icon `working` (an arc on a faint ring), turns while live | `status.working` |
| Needs input | icon `needs-input` (fermata), shimmers while live | `accent.text` |
| Ready for review | icon `done` (ring with check) | `accent.text` |
| Failed | icon `failed` (ring with cross) | `status.failed` |
| Resolved | glyph `status-resolved.svg` (solid disc with check) | `status.done` |

Needs input and Ready for review both use the accent, because both are your turn. The fermata is filled, and the review ring is open, so the two stay apart without a second color.

Up next and Resolved keep Consort's own glyphs, because the icon set has nothing like them. A live status (Working or Needs input) shimmers through the three versions of its icon at 8 frames a second. Working also turns, one turn in 12 steps (1.5 s), because a shimmer at 14 px is too small to see. Both stand still under Reduce motion.

### Channel icons

A channel can have any icon of the set. The New channel dialog shows seven quick icons (`code`, `globe`, `server`, `home`, `memory`, `appearance`, `star`) and All icons. All icons opens a dialog over it with a search and every icon in six categories (`ui/icon_catalog.rs`). A word finds an icon when it starts a word of its name or of a keyword; the keywords include the old Lucide names, so "cloud" finds `globe`. An icon picked in All icons takes the place of the last quick icon. The channel settings and Change icon in a channel's menu open the same dialog.

An icon tile has four states. At rest: no ground. Hover (the active state): `tint_accent` ground, the icon in `text.primary`, and the shimmer through its three versions. Picked: `surface.selected` ground with a `text.secondary` border; it does not shimmer, so the accent stays the color of hover and focus. Keyboard: the focus ring. Designs: `docs/notes/design-icon-picker.md`.

A worker that is online shows a solid dot in `status.done`. A worker that is offline shows an open ring in `text.tertiary`.

### Where status and requests appear

- A sidebar row shows the status glyph of the most urgent task in that channel, in this order: Needs input, Ready for review, Failed, Working. A channel with something for you has its name in `text.primary`. Other channel names use `text.secondary`.
- A task in a timeline is its task line (see [The chat](#the-chat)). While the task runs, waits for review or is resolved, the line has no card under it. A task card keeps its body only when something needs you in the chat: a pending approval (shown as the waiting approval card, with its buttons, because the harness cannot continue until you answer), the reason of a failed task, or the steps of the voice task. A decision does not block the harness, so it appears in the task thread and in the Tasks panel, not inline. Resolve is in the thread.
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
| Smallest window | 420 by 560 |

The timeline is a chat of bubbles. See [The chat](#the-chat). It sticks to the bottom, next to the composer. Older messages scroll up.

On a phone:

- A top bar replaces the sidebar. Its left side switches channels. Its right side opens the Tasks screen and shows how many tasks need you.
- Every control that you tap is at least 44 by 44 px. Approval buttons fill the width of the card.
- The composer is one field with `chat.composer_radius` corners. The mic button sits in the tool row under it, after add. See [The chat](#the-chat).
- Lists have no selected row.

Settings use a fixed layout: the settings sidebar, then a label column of 160 px and the controls to its right. Dialogs are 520 to 560 px wide with `xl` corners and `shadow.float`, over a black scrim at 55 percent in dark mode. The Add account dialog is 600 px wide (`layout.dialog_account`), because its provider grid has two cards in a row.

## The chat

The main column has no header. The timeline fills it, the composer sits under it, and the Three voices mark and the top actions float over its top fade. The values come from the Paper artboard "Tutti, bubbles, dark".

- **Lanes.** The assistant's messages are bubbles at the left on `surface.card`. Your messages are bubbles at the right on `accent.surface`. A bubble is at most `layout.bubble_max_width` (560 px) wide and has `chat.bubble_radius` (16 px) corners, with `chat.bubble_pad_y` by `chat.bubble_pad_x` padding. Message text uses Body.
- **Groups.** Bubbles that follow each other in one lane form a group, `xs` (4 px) apart. Groups are `lg` (16 px) apart. The last bubble of a group has a `chat.bubble_tail` (4 px) corner on the speaker's side: bottom left for the assistant, bottom right for you. A time separator, a card or a notice ends a group.
- **One bubble for each message.** A message from the assistant is one bubble, whatever it holds: paragraphs, a heading, a list, a code block, a table, a quote or a rule. The old rule split it at each paragraph, so a long answer looked like many messages. Two messages are still two bubbles. Your own message is one bubble too (DECISIONS 731).
- **A brief** (a message with a title and task references) has a title bubble in semibold, then its text. The task references are a numbered list in the last bubble: a chip with the status and the task name that opens the thread, the channel in `text.tertiary`, and the line about the task.
- **Time separators** are centered, in Label size and `text.tertiary`. The first item of a day gets the day and the time ("Today 8:00 AM", "October 12, 9:00 AM"). A later item gets the time alone ("9:12 AM") when it comes 30 minutes or more after the item before it (`chat::SEPARATOR_GAP_MS`). There is no time on each message. A message on its way says "Sending" under its last bubble, and an edited message says "Edited".
- **Cards.** Task cards, approval cards and decision cards sit in the assistant lane at bubble width, with `chat.bubble_radius` corners and a `chat.bubble_tail` corner at the top left. Above each one is a task line: a curved connector (`glyphs/task-connector.svg`, `chat.task_connector`, `border.strong`), the status icon, the task name in UI strong, and its state in the status color ("Needs you" for Needs input). A task card in another channel adds "in <channel>" in `text.tertiary`. A task whose card has a body (see [Where status and requests appear](#where-status-and-requests-appear)) has no header row of its own: its first row is what asks or the newest update, with the approval mode and the worker pill at the right. Every other task is its task line alone. Its state says "Working, 2 min", "Needs you" or "Ready for review", or the summary of a resolved task. Only Needs input and Ready for review use the accent for these words. Working uses `text.tertiary`. A decision of the channel's assistant has the task line "Decision", with "Needs you", "Answered" or "Closed". An approval has no task line (see [Approvals](#approvals)).
- **Notices** are a centered line in UI size and `text.tertiary`, with their time.
- **The Three voices mark** sits at the top center, `chat.mark` (56 px), `chat.mark_top` from the top. Under it, an activity pill (`surface.card`, `border.subtle`, `full` corners, `chat.pill_pad_y` by `chat.pill_pad_x`, Label size in `text.secondary`) says what the assistant does, only while it works. See [Logo](#logo).
- **Top actions** sit at the top right, `md` from the edges, `xs` apart: Connectors (for a channel that has connections or skills, with their count), Channel (the `channel` hash icon, which opens the Channel panel), Tasks and New task. Each is `chat.top_action` (32 px) square with a `chat.top_action_icon` (18 px) icon in `text.secondary`. Tasks opens the Tasks panel, has `surface.selected` while the panel is open, and has a `component.state_dot` dot in `accent.fill` while something needs you.
- **Top tint.** The top of the timeline darkens progressively, as in Claude Duet. It is no solid zone: content that moves up under the mark stays faintly visible. `layout.top_tint` sets the canvas color at four stops: 97 percent at the top, 94 percent at the bottom of the mark (60 px), 80 percent at the bottom of the pill (86 px), and clear at 108 px. Content under the mark and the pill is only a faint trace, as the user asked on 2026-10-29 ("once you get to the avatar position, it needs to darken significantly"). A GPUI gradient has two stops, so the view draws one gradient layer between each pair of stops (three layers). The mark, the activity pill and the top actions sit on the tint. At rest, the first item starts `layout.content_top` (98 px) from the top, just below the mark and the pill (mark top, mark, `space.xxs`, the pill, and a small margin). A theme test keeps it tall enough. It stays the same when the pill is hidden, so the chat does not jump when work starts or stops.
- **Bottom fade.** `layout.fade_bottom` (32 px) above the composer, shown only while the reader is scrolled up.
- **Composer.** One field on `surface.card` with a `border.strong` line and `chat.composer_radius` (16 px) corners, as wide as the timeline. The field starts at one row. It grows upward with its text to five rows, and then the text scrolls inside it. Its bottom edge and the tool row stay still. The send key (`send` icon, `chat.send_button`, `text.tertiary`) sits inside it in the bottom right corner at every height. The text keeps out of the column of the send key on every row, so words do not move to another line when the field grows. Under the field, outside it: add and the mic (`component.composer_chip` square; the mic is the `voice` icon, and a click starts dictation, see [Dictation](#dictation)), a `border.strong` divider, "Same as Tutti" for a channel with no model of its own, the model chip and the effort chip, a spacer, and "Runs on <worker>" in Label size and `text.tertiary`. The placeholder is "Message <channel>", and "Message Tutti" in the main channel. The task thread uses the same component (`ui/composer.rs`): the same padding, field and tool row, so with a panel open the two fields and the two tool rows line up at the bottom of the window. Its placeholder is "Reply to this task". In a narrow place the tool row drops "Runs on" first, and then the account name in the chips. It never wraps and never overflows.

### Chat sizes

These sizes are in `component.chat` of `structure.json`, except the three in `layout`.

| Token | Size (px) | Use |
|---|---|---|
| `layout.bubble_max_width` | 560 | The widest bubble or card in the timeline |
| `layout.setup_column`, `layout.setup_top` | 448, 136 | The column of a setup screen, and the space above its mark |
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

- **The add button** in the composer (the chat and the task thread) opens a menu: Upload from this computer (the system file dialog) and Choose from channel files. A pasted picture or copied file also attaches.
- **Chips** sit in the field, above the text, and wrap. A picture is a `chip_image` (56 px) thumbnail with `lg` corners and a round remove badge on hover. Any other file is a `chip_file` (240 px) chip on `surface.raised` with a `chip_tile` (38 px) tile, the name, and the type and size ("PDF, 2.4 MB"). An upload shows a `progress` (4 px) bar in `status.working` on a track at `opacity.progress_track`, and its share ("62%"). A failed upload has a `status.failed` border at `opacity.failed_border`, a tinted tile, the message ("Upload failed. The hub is offline.") as selectable text, and Try again.
- **Send waits** until every upload of the message is on the hub. A failed upload stops the send, and the text stays in the box.
- **Drop.** Files dragged anywhere over the chat attach to the composer. The composer is the only visual target: it grows, gets a dashed `accent.text` border and an `accent.fill` fill at `opacity.drop_fill`, and shows the upload icon on a `tint_accent` disc, "Drop to attach 3 files" and "Up to 256 MB each". Text that you typed stays visible.
- **Bubbles.** Your pictures are `image_tile_w` by `image_tile_h` (228 by 168 px) tiles, two in a row, above your text. Your files are `file_bubble_you` (280 px) file bubbles on `accent.surface`. The assistant's files follow its text as `file_bubble` (360 px) bubbles on `surface.card`. An artifact names its task with the status glyph, and its size. On hover a file bubble shows Save and Open.
- **Documents.** A click on a `.md`, `.markdown` or `.txt` file card (a bubble or a Files row) opens the document in the right panel (see [The document panel](#the-document-panel)). Other types open in the app of the system. The card of the open document has `surface.selected`, and a click on it closes the panel.
- **Light box.** One component for pictures and visuals (Paper G4, G5), on `lightbox` (black at `derive.lightbox_alpha`). The top bar (`lightbox_bar`) has the name, the details and the caption, and icon buttons with tooltips: Save, Open and Close for a picture; Save image, Open in browser and Close for a visual. A picture shows at most `lightbox_image_w` by `lightbox_image_h`, with Previous and Next (`lightbox_nav`) and "1 of 2". The arrow keys step. A click outside the picture or the sheet of a visual closes the light box, as Escape and Close do. A click on the picture, the sheet, an arrow or a button does not. A visual opens alone, with no arrows and no count. See [Visuals](#visuals).

### Visuals

A visual is an HTML page that the assistant shows as a timeline item of its own (CONTEXT.md). The values come from Paper section 15, G1 to G5. The plan is [VISUALS-PLAN.md](VISUALS-PLAN.md).

- **The item.** A visual has no bubble and no card. It lies on the canvas at the full width of the timeline column (or of the task thread), in three parts, `xs` + `xxs` (6 px) apart:
  - the title line: the `visual` icon at `icon.dense` in `text.tertiary`, and the title in Label size and `text.secondary`, `xs` from the left. The title is chrome, so it is not selectable;
  - the page area, with `lg` corners;
  - the footer line (`chat.footer_line`). On hover it shows Open full screen, Copy image and Save image (`chat.footer_button` buttons with tooltips) and the time, as on a message. The line keeps its height without the hover, so nothing moves.
- **Height.** The page area takes the height that the live page reports at this width. Else it takes the height of the image, scaled to the width. Else it takes the height table of the notice, and then the height that the assistant gave. Until the page or the image is ready, the item keeps that height, so the timeline does not jump.
- **Loading.** "Loading the visual" in Label size at the regular weight and `text.tertiary`, under the three dabs of the mark, which shimmer on the mark clock. The area is `surface.card` at `opacity.loading_fill`. It shows while there is no image and no live page.
- **Failed.** "The visual did not load" on `surface.card` with a `status.failed` border at `opacity.error_border`, the `failed` icon, what happened (selectable), and Try again. When the page did not come: "The hub did not send the page. Check that the hub is online, then try again." Else the error of the image says what to do. An error that passes by itself (no session host online, the browser for visuals is installing, no connection) tries again by itself after 2, 4, 8, 16 and then 30 s. The item shows the failure after three tries, or at once for any other error. Try again asks at once.
- **Live or image.** The app decides in each frame, as `ui/visual_cover.rs` says. A visual is live (macOS only) while all of its page area is between the top tint and the bottom fade, nothing is over it and the window is active. It shows its image while a menu, a picker, a dialog, a notice, the drawer or the light box is over it, while a part of it is under the tint or the fade, or while it is taller than the clear part. Off screen it draws nothing, and its live page is hidden; the page goes away 30 s later. At most four live pages exist. The app asks for the image as soon as the item is on screen, at the width of the item and the scale of the window, in the active theme. A change of theme or mode loads the live pages again and asks for new images. Windows and Linux always show the image.
- **Menu (G2).** A right click on the item, or on its live page: Open full screen, Open in browser, a separator, Copy image, Save image, Save as HTML. The light box has no Open full screen. Text that the user selected in a live page gets the menu of selected text. Open in browser and Save as HTML write the page with the theme of the app and no script of the app. Save image and Save as HTML ask where with the save dialog.
- **Full screen (G5).** The light box shows the visual live on a sheet: `surface.canvas`, a `border.subtle` border, `xl` corners, `lg` padding, at most `files.lightbox_sheet_w` (1072 px) wide. The page takes the width of the sheet and its own height. A page taller than the window scrolls inside the sheet. The sheet starts below the reach of the tooltips of the top buttons, because a live page draws over anything in its bounds. While the light box shows a visual, its item in the timeline shows the image.

### The Channel panel

The `#` top action opens the Channel panel. It shares the right side with the Tasks panel: only one of them is open at a time. It is 400 px wide, with the channel name and purpose in its header, and the tabs Files, Routines, Memory and Connectors, each with its count. A routine or a memory note opens in place of the tabs, with a back button in the header.

The Connectors tab lists the connectors and then the skills of the channel, each with its switch. A switch turns an item off for this channel only. An item that the channel does not get has Change, which opens the item in Settings. The count is the number of connectors and skills that are on. Add a connector opens the Add connector dialog, and Manage all in Settings opens Settings > Connectors. In Tutti, the tab shows what Tutti can use. The hint at the bottom says that changes apply to the next session in the channel. The rows truncate their names and wrap their detail lines, so the tab fits the least panel width of 320 px. The list scrolls inside the panel with fades. The item "Connectors and skills" of the channel row menu opens the channel, the panel and this tab; there is no dialog for it.

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

Settings > Providers lists only the tools that the user knows as a provider, such as Claude Code, Codex, OpenCode, Gemini CLI and the registry agents. Helper tools (Node.js and the Claude Code ACP adapter) have no row and no section. Their install state and their failures show on the row of the provider that needs them.

Settings > Workers and Settings > Providers use these sizes from `component` in `structure.json`. The account rows and the tool rows are in Providers, and each ends with a three dots button of `row_dots` (28 px):

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
| `setup_mark` | 84 | The mark on a setup screen |
| `setup_gap` | 28 | The space between the parts of a setup screen |
| `setup_control`, `setup_field` | 36, 40 | A button and a field on a setup screen |

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
| `composer_chip` | 28 | The height of the model chip and the effort chip in the composer. The mic is this size, at rest and live |
| `dictation_select`, `dictation_cleanup_select` | 220, 270 | The Language and Input device selects, and the Cleanup model select, of Settings > Dictation |
| `chat.mic_stop`, `chat.mic_bar`, `chat.mic_bar_gap`, `chat.mic_bar_max`, `chat.mic_dot` | 7, 2, 1, 12, 6 | The live mic: its stop mark, its three level bars, and the failed dot |

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

- **Settings > Models** is a settings page of `settings_detail_max`, first in the Assistant group of the settings rail, with the `chip` icon. It has the fallback path (the first row with a lock, the built-in row "Other accounts with the same model" with a switch, the steps, the add row, and the fixed end row "Stop and show the error") and "How the path works". Sizes are in `component.fallback` of `structure.json`. A step row has a handle, its number, the logo, the model, its effort and a remove button. A step moves by drag, by Alt+Up and Alt+Down, or by the menu of the row. The first step on another harness asks in a confirm dialog. The model chip of an assistant on a fallback has a marker dot and a menu (Go back to the first choice, Choose another model, Edit path). Move notices in the timeline have a Details toggle with the lines of the walk.

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
- **The writing bubble** is a bubble with `writing_pad_x` by `writing_pad_y` (16 by 13 px) padding and three `dab` (10 px) dabs of the mark, `dab_gap` (5 px) apart: gold `dab_drop` (2 px) low, verdigris `dab_rise` (3 px) high, lapis 1 px low. The dabs are `assets/glyphs/dab-*.svg`, in three hand-drawn versions each, and shimmer on the shared mark clock. They also rise `writing_lift` (3 px) in a wave, one dab after another, two frames apart, in a loop of 8 frames. The shimmer alone is too small to see at this size. The rise does not change the layout. They stand still with Reduce motion. A `turn_failed` notice ends the bubble.
- **The Routines tab** shows the check-in first, with the `eye` icon, "Every 6 hours, uses `check-in.md`" and its last result from the log ("Stayed silent at 7:25."). Its hover actions are Run now, Run history and Open note, and its menu has no Edit. The Follow-ups section lists each follow-up with the `clock` icon, its reason as the title and its time ("Today at 15:00, set this morning"), Cancel on hover, and Cancel all.

### Write in my voice

The values come from the Paper artboards `H3V-0`, `H7T-0`, `HL7-0` and `HTB-0`, through `component.voice` in `structure.json`.

- **Settings > Voice** is a settings page of `settings_detail_max`, after Memory in the settings rail, with the `quote` icon (the `voice` icon is the dictation microphone). With the voice off: the switch, "Where Consort learns from", "What the voice never changes", and "Turn on and learn". With the voice on: the switch and the learned line with Change sources and Learn again, the style guide, Try it, Disclosure, and Turn off and Reset. Reset is a secondary button with its text in `status.failed`, and it asks first in a confirm dialog.
- **A source row** has a `check_box` (16 px) checkbox (the accent fill with a `check_icon` (12 px) check when ticked), a `source_tile` (32 px) tile on `surface.selected` with the monogram or the `file` or `edit` icon, and the title and lines.
- **The style guide** is one bordered card per section: the title, the subtitle in `text.tertiary` and "Learned from ..." at the right. A rule is a row with `rule_pad_y` (8 px) padding and a `rule_dot` (4 px) dot in `text.tertiary`. An example is a block on `surface.card` with `example_pad_y` (10 px) padding. Remove shows on hover in a fixed `remove_slot` (84 px) lane, and the hovered row is on `surface.card` (an example on `surface.selected`). "(you)" is in `accent.text`.
- **Try it** shows the sample on `surface.card` with the voice line, and "Nothing is sent" after a `sample_dot` (7 px) dot in `status.done`.
- **Disclosure** is three radio options: a `check_box` ring with a `radio_dot` (8 px) dot in `accent.text`.
- **The draft card** is described in [Drafts](#drafts). Its flag bar is `flag_bar` (2 px), and the Subject and body fields of Edit are `subject_box` (30 px) and `body_box` (150 px, at least).
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

The task thread is bubbles, as the timeline: the brief is the first bubbles with the placement as a caption under them, the activity is a chip that opens to the commands and file changes (each opens in full), and a decision has a task line ("Decision", "Needs you") above its card. An approval has no line above it (see [Approvals](#approvals)). A post of yours that the assistant passed on from the chat has "Show in channel" under it, at the right (a leaving link with the `external-link` icon). The plan of the harness is a checklist card under the newest message: the head "1 of 3 done", then one row for each step. A done step has the `done` icon in `status.done` and `text.tertiary` text. The current step has the working glyph and `text.primary` text. A next step has the open ring and `text.secondary` text. The header of the thread says "Working, <current step>" in `text.tertiary` while the task works. While the task runs and no turn-ending event came, the writing bubble (the same animated dots as in the chat) is under the plan card. A failed turn is the same error card as in the chat, and a message of yours that the hub refused stays in your lane with its reason (see [Errors](#errors)). The composer is the shared one, under the thread.

### Approvals

An approval has two views (`ui/approvals.rs`).

- **While it waits** it is a card at the bubble width (`layout.bubble_max_width`) with the `accent.text` border. The first row has the fermata glyph, the title (what the action is) and "Needs you" in `accent.text`. Under it are the folder, when the agent can choose it, and the command or target once, in the mono style in a box on `surface.raised`. A command stops at 5 lines with "Show all" (then "Show less"). All text wraps inside the box, so a long address with no spaces breaks at the edge. The buttons are one row of equal widths: Allow once (primary, with a border in its own color so it is as high as the others), Allow for this task and Deny. A label over 20 characters does not fit that row: it gets its own row under the others, at full width, and wraps inside its button. A card with no task choice has two buttons. The wide choice shows the label of the harness when it gave one.
- **After the answer** it is a line of `chat.answered_line` (36 px) in a box on `surface.card` with a `border.subtle` border and `lg` corners: a glyph, the action cut to one line in the mono style, the answer ("Allowed once", "Allowed for this task", "Denied", "Closed, no answer") and a chevron. A click opens the box in place, with the full command, the folder and "You chose: ..." when the harness gave a label. Nothing else in the timeline moves. The open state is for one approval in one place.
- **Hub cards** keep their own body while they wait and while their outcome moves. After that they use the same line.

### Access and permissions

ADR 0020 has the model. The design is in Paper.

- **The Access section** of the channel settings has one card with two rows, split by a `border.subtle` line. Permissions has a detail line such as "4 command rules, 1 tool exception" and the button Edit permissions. The count has the rules that apply in the channel, with the rules of Tutti that the channel does not override, and the exceptions of the connections that the channel has. Connectors and skills has its summary and the button Edit connectors. Each button closes the settings and opens its own view.
- **The Permissions dialog** has the shell of the channel settings: the channel icon, "<channel> permissions", one line, the close button, two columns with a line between them, and Cancel and Save changes. Every edit is a draft until Save changes. Each column is as high as its content, and scrolls with fades when the window is too low.
- **Commands** (left) has the button Add a rule and these groups: "Runs without asking", "Always asks, also in Full access" and "From Tutti, in every channel". An empty group has no heading. Tutti has no third group, and the third group has no rule that the channel overrides. A row has the pattern in the mono style (selectable), what it does, and a quiet line "Except a, b. Added by the assistant.". A row has a "..." button and a right-click menu: Edit and Remove, or Override here for a rule from Tutti. With no rule, a box says "No rules yet" and points to the assistant, with Add a rule.
- **Connections** (right) has one card for each connection of the channel. The header has the name, and at the right "The assistant's account" in `accent.text`, or "Also in <channels>". The rows Read, Change and Delete each have a select. Under a line, each tool exception has its name, a select with only Run without asking and Ask every time, an x button and a right-click menu with Remove. Add an exception opens a list of the tools with a search. An account of the assistant says that it sends without asking when its rule says so, and that it does not use the voice of the user.
- **The rule dialog** (Add a rule, Edit rule) opens over the Permissions dialog. It has Command and Except in the mono style, When it matches as two segments (Runs without asking, Always asks), and What it does. Its error shows in the dialog. Escape closes only this dialog.
- **The permissions card** in the chat has the title, "Needs you", each group with a bold heading, rows in two columns (a fixed column for the subject, mono for a command, and the words), the note of a group in the quiet style, and the risk in `status.failed`. Allow and Deny follow. After the answer it is one line: "Permissions of <place>: N changes" and Allowed or Denied.
- **On the connection page** the row "This account belongs to the assistant" has a switch. While it is on, the risk shows under the switch in `status.failed`.

### Drafts

A draft card shows text that goes out in your name (`ui/drafts.rs`). It has the bubble width and three views.

- **While it waits** it has the `accent.text` border. The title row has the `message` icon, "Draft: <title>" in UI strong, "In your voice" in `text.tertiary` when the voice is on, and Copy and Edit as icon buttons (`chat.footer_button`). Under it are the To line ("To <to>, from <connection>") and the Subject line in `text.tertiary`, with "Not how I write" as quiet Label text at the end of the first line. A `border.subtle` line, then the text at Body size. A flagged phrase is underlined in `accent.text`, and each flag is one line under the text with a `flag_bar` bar and the words "<phrase> is not in your request. Check it." in `accent.text`. The footer has the hint "Edit it here, or tell me what to change." (or, with flags, the tick "I checked the marked line"), then Not now (secondary) and Send (primary). With flags, Send is off (`surface.selected` and `text.tertiary`) until the tick.
- **In Edit** the Subject and the text are fields in one box with the `accent.text` line, inside the same card. The footer says "Consort checks the text again when you save." with Cancel and Save.
- **After the answer** it is the line of 36 px as for an approval, with the title and the outcome ("Sent to Dana", "Not sent", "Replaced by a new version"). A click opens the text that went out.
- **`show_draft`** posts the same card with a `border.subtle` border, because nothing waits. The footer says "Consort cannot send this one for you." with Edit and Copy buttons. Copy says "Copied the text." in a corner notice.

### The document panel

A click on a Markdown or text file opens it in the right panel, at the width of the other panels (`ui/document.rs`, Paper F1). The panel has `surface.canvas` and a `border.subtle` line at its left.

- **Header.** The file name in mono at the left, then Copy, Save and Open in another app as icon buttons (`copy`, `download`, `external-link`), and Close at the far right end. Rule 4: the card that opened the panel closes it, and so does Close.
- **The line under it** has the glyph of the task that wrote the file, the task name, then the size and the date ("6 KB, today") in Label size. A file that no task wrote shows the size and the date only.
- **The page** is rendered Markdown at the width of the panel, with selectable text. It scrolls inside the panel. A `.txt` file is selectable mono text that keeps its line breaks.
- **Loading** says "Loading the document." **A failure** says "Consort could not show this document.", the reason as selectable text, and the buttons Try again and Open in another app. **A file over 2 MiB** never loads: the panel says "This document is too large to show here." with the size, the limit and Open in another app. The same message and action show for a file that is not text.

### Errors

An error is in the place where the user looks for the answer (DECISIONS 732 to 735).

- **A failed turn** is a card in the assistant lane (`ui/errors.rs`, Paper E1 and E2): `chat.bubble_max_width` (560 px) wide, `surface.card`, a 1 px `status.failed` border at `opacity.error_border` (0.4), the `chat.bubble_tail` corner at the bottom left. It has the `failed` icon, a title in Body semibold, the text in Body `text.secondary`, and the actions: Sign in again, Choose another model and Try again, with Details at the right end of the row. Details opens the raw text of the harness as selectable mono text. The footer keeps the time. After the user tries again or writes another message, the card is quiet: one line with the icon, the title and a chevron, with no buttons. A click on it opens it. The task thread shows the same card. A routine run or a check-in has no Try again. The writing bubble does not show with the card.
- **A corner notice with a button:** the text scrolls at `layout.error_text_max_height`. The button stays under the scroll, so a long text never hides it.
- **A message that the hub refused** stays in your lane, dimmed (`opacity.answered_dim`), with the reason under it and the link "Put back in the message box".
- **Corner notifications** (`notice.rs`, Paper E3 and E4) are for the actions of the app: a save, a rename, an upload, a hub that did not answer. A notification is `notice_width` (360 px) wide, `space.lg` from the right, on `surface.raised` with `shadow.float`, `notice_bottom` (124 px) above the bottom edge. An error has the `failed` icon, a UI semibold title, the text in UI `text.secondary`, an optional action and the `close` button. It stays until the user closes it. An information notice is one line and leaves after 6 s. Three show at most, with the newest at the bottom. More errors wait, and a line under the stack says how many. In a window under `compact_width` (760 px), and in a wider window where the stack would cover the right lane of the chat (under 1144 px with the sidebar shown, and no panel), the stack is as wide as the window less a 16 px margin on each side, `notice_compact_top` (88 px) from the top. The information notices have an `info` icon, which the design does not show.

### Monid and access requests

The Paper page "Redesign, Oct 30", section 13 (M1, M2, M4 to M9), shows these surfaces.

- A Monid card and an access card are approval cards. The requested choice comes first. Deny is last. A card does not fill the width of the timeline.
- A call row is one line with the tool, the status and the cost. It opens to show the input, the output and the cost details. This text is selectable.
- A task card shows "Monid: N calls so far" and the total.
- Settings > Connectors shows Monid as the first row of the list when the hub has a key. The footer under the list also has Monid.
- The Channel panel shows a Temporary access section only when a grant runs. Each row has End.
- Money uses the mono type, so that amounts line up.

### Dictation

Dictation runs on this computer (`consort-dictation`, on the Sayso SDK). The plan is [DICTATION-PLAN.md](DICTATION-PLAN.md), and the designs are in [notes/design-dictation.md](notes/design-dictation.md).

- **Start and stop.** A click on the mic, or the shortcut (Control Option D by default). A tap of the shortcut starts and stops. A hold of more than about 300 ms is push to talk. The composer and the reply box work the same way.
- **The words.** Heard words come at the caret in `text.tertiary`, between the draft on each side. They turn `text.primary` when dictation ends. There is no accept step. The whole dictation is one undo step.
- **The state.** While the state is starting, listening or finishing, the field border is `accent.text` at `opacity.dictation_border`, and the placeholder of an empty box is "Listening". The mic keeps its 28 px square and has `accent.fill` at `opacity.tint`: a `chat.mic_stop` stop mark and three level bars while it listens, and a spinner while it finishes. The slot at the right of the tool row says "Getting the speech model ready", "Esc cancels" or "Fixing the text". This slot drops first in a narrow place.
- **Esc** removes only the dictated words, and the caret goes back. It shows no notice.
- **An edit.** A change in the draft keeps dictation going. A change in the dictated words gives them to the user and ends dictation.
- **Stop after silence** ends dictation as Stop does and never sends. An information notice says "Dictation stopped after 30 seconds of silence." with the number of the setting, and leaves after 6 s.
- **Send while dictating** stops dictation and holds the message. The notice has Send now. Send now sends the words on screen, without the words still to come.
- **Failures** use the corner notice styles. Information with a button stays until the user closes it or the state ends. Cleanup failed is one that leaves after 6 s, and the words stay as heard. The mic has a `status.failed` dot (`chat.mic_dot`) for no permission, no microphone, no key and a lost connection.
- **Settings > Dictation** holds the engine, the cleanup, the silence limit, the language, the microphone and the shortcut. These stay on this computer. So does the Deepgram key. S1-mini always shows as "S1-mini by Superwhisper".

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
- **No account.** "No account is signed in. Add one in Settings > Providers." with the button Open Settings. The popover then has no rail, and it is as high as its content.
- **Effort.** A second chip next to the model chip: the `effort` icon and the level. Its menu is `component.effort_menu` (240 px) wide, with the title "Effort for <model>", one row per level, "Default" after the provider's default level, and the note "<Provider> offers these levels for this model. More effort thinks longer and uses more of your plan." When the provider does not name its default, the first row is Default, which takes the effort out. The chip is not there for a model with no effort levels. In a dialog, the effort is a select of `component.effort_select` (150 px) next to the model field. A new model keeps the effort when it offers that level. Otherwise the effort goes back to the new model's default.
- **Task threads** have the model chip and the effort chip in the tool row of their composer, as the chat has. The picker lists only the models of the worker that holds the task. A pick of the same account applies from the next turn. A pick of another account says "This starts a new session in the same folder." under its row. A task that cannot change its model (it is Resolved) shows the chips with no chevron, and they do not open.

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

The clock runs only while a draw showed one of these. Work in progress moves also in a window without the focus, because the user often watches it from another app. The idle mark moves only in the active window. When the only thing that moves is the idle mark at the top center, the clock ticks at half speed (4 fps). With Reduce motion on, nothing draws on a timer.

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
- **Where.** The mark is the logo on onboarding and on the start-up error, the Tutti row in the sidebar (sharp, `component.sidebar.row_icon`, 18 px; it plays the working or needs-you frames only while Tutti itself works or needs you, and shows the first idle frame at rest. Work in another channel does not move it) and in the switcher (16 px, still), and the title bar on Windows and Linux. The app icons in `design/logo/` are the idle mark on `surface.card`.
- **Wordmark.** "consort" in lowercase, Atkinson Hyperlegible Next 600, to the right of the mark. The mark is about 1.2 times the font size high.

## Text in the UI

- Buttons say what happens, with a verb: **Allow once**, **Deny**, **Resolve**, **Start task**. An action keeps its name through a flow, so **Resolve** leads to "Resolved".
- Do not join details with middle dots. Use a comma, or put the details on separate lines.
- Do not add arrows to button or link text.
- An error says what happened and what to do next. An empty state names the situation and the next action.
- Write in the style of ASD-STE100: short sentences, one instruction per sentence, and the same word for the same thing everywhere.

## Assistant status line

When the assistant of a channel cannot answer, the chat shows one status line just above the composer, in the place of the link line ("Connecting to the hub"): the `circle-alert` icon, the reason in Detail `text.secondary` as selectable text, and, when one account is signed out, a Quiet Sign in again button at the right end. The icon is `status.failed` when the button is there, else `text.secondary`. The line is not a message and has no time. It shows only while a message of the user waits for an answer (or a turn cannot start). It waits 30 seconds after a start of the hub, and it goes away by itself when the assistant can answer. It does not show while the link line shows. The sign-in notice in the sidebar stays as it is.

A one-line system notice in the timeline has its time at the right, on the baseline of the first line of its text. GPUI aligns the bottoms of text boxes, not their baselines, so the row starts both boxes at the top and moves the time down by the difference of the two baselines (`parts::baseline_lead`).

## Sign-in notice

An account whose login expired has a notice in the sidebar, above the update notice. It uses the box of the update notice. The text is "<account> needs a new sign-in." Sign in again is a small bordered button under the text, and it opens the sign-in of that account. The notice has no close button. It leaves when the sign-in works or when the user removes the account.
