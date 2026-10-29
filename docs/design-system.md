# Design system

This page is the reference for how Consort looks, on the desktop and on mobile. The values live in files, and this page explains them:

| File | Contents |
|---|---|
| [`design/structure.json`](../design/structure.json) | Type, spacing, corners, borders, shadows, motion, icons and layout. The same for every theme |
| [`design/themes/`](../design/themes/) | One file per theme with the color roles for a dark mode and a light mode. Spruce is the default |
| [`design/theme.schema.json`](../design/theme.schema.json) | The schema that every theme file must match |
| [`design/gpui-kit-mapping.json`](../design/gpui-kit-mapping.json) | How the color roles become GPUI Kit theme keys |
| [`design/logo/`](../design/logo/) | The brace mark and the app icons |
| [`design/glyphs/`](../design/glyphs/) | Consort's own glyphs: the brace and the seven task statuses |

Why themes can change only color: [ADR 0009](adr/0009-themes-change-color-only.md).

## Principles

1. **One voice.** One type family, one icon set and one corner system everywhere. A harness, a model or a machine never gets its own style.
2. **Color means state.** Surfaces stay neutral. Color appears only where something has a status. Code blocks are the one exception, and their colors stay muted.
3. **The accent means your turn.** The accent marks you: your messages, the approvals and decisions that wait for you, and the buttons that answer them.
4. **Notation signs mark Consort's own ideas.** The brace joins the parts of a score. It is the logo and the icon for Everything. The fermata means "hold until the leader gives the cue". It marks a task that needs your input. All other icons come from Lucide.
5. **Depth comes from surfaces, not shadows.** Five surface steps and two border colors. Only floating layers cast a shadow.
6. **Calm by default.** Motion answers your actions or shows work in progress. Nothing moves for decoration.

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
| Up next | `status-up-next.svg` (dashed ring) | `text.tertiary` |
| Scheduled | `status-scheduled.svg` (clock) | `status.scheduled` |
| Working | `status-working-track.svg` and `status-working-arc.svg` (a turning arc on a ring) | Arc in `status.working`, ring in `border.strong` |
| Needs input | `status-needs-input.svg` (fermata) | `accent.text` |
| Ready for review | `status-review.svg` (ring with check) | `accent.text` |
| Failed | `status-failed.svg` (ring with cross) | `status.failed` |
| Resolved | `status-resolved.svg` (solid disc with check) | `status.done` |

Needs input and Ready for review both use the accent, because both are your turn. The fermata is filled, and the review ring is open, so the two stay apart without a second color.

GPUI tints an SVG with one color, so the Working glyph is two layers: the ring and the arc. Only the arc turns.

A worker that is online shows a solid dot in `status.done`. A worker that is offline shows an open ring in `text.tertiary`.

### Where status and requests appear

- A sidebar row shows the status glyph of the most urgent task in that channel, in this order: Needs input, Ready for review, Failed, Working. A channel with something for you has its name in `text.primary`. Other channel names use `text.secondary`.
- A task card in a timeline shows a pending approval inline, with its buttons, because the harness cannot continue until you answer. A decision does not block the harness, so it appears in the task thread and in the Tasks panel, not inline.
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
| Label | 12 / 16 | 500 | Group names in the sidebar and field labels |
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
| Sidebar width | 248 |
| Side panel width (Tasks, task thread) | 400 |
| Timeline content width | At most 720 |
| Speaker gutter in the timeline | 64 |
| Smallest window | 960 by 600 |

Message layout depends on the width of its container:

- **600 px or wider (the main timeline).** The speaker name and the time sit in the 64 px gutter on the left. The message, card or bubble starts 16 px to the right of the gutter. Day labels align with the message column.
- **Narrower than 600 px (side panels and phones).** The speaker name and the time sit on one line above the message.
- In both layouts, the assistant's text has no bubble. Your own messages sit on `accent.surface` with `lg` corners.
- The timeline sticks to the bottom, next to the composer. Older messages scroll up.

On a phone:

- A top bar replaces the sidebar. Its left side switches channels. Its right side opens the Tasks screen and shows how many tasks need you.
- Every control that you tap is at least 44 by 44 px. Approval buttons fill the width of the card.
- The composer is a pill with `full` corners. The microphone sits at its right end.
- Lists have no selected row.

Settings use a fixed layout: the settings sidebar, then a label column of 160 px and the controls to its right. Dialogs are 520 to 560 px wide with `xl` corners and `shadow.float`, over a black scrim at 55 percent in dark mode. The Add account dialog is 600 px wide (`layout.dialog_account`), because its provider grid has two cards in a row.

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

## Provider logos

Every provider shows with its logo, never as text alone. The logos are in `crates/consort-desktop/assets/providers/` (see the NOTICE there).

- Claude Code and Gemini CLI keep their own colors.
- Codex, OpenCode and the ACP registry icons draw in the color of the text next to them, like an icon. Registry icons are SVGs that draw in `currentColor`. The client loads them from the registry (`https` only, at most 64 KiB, SVG only), and the app draws them like a Lucide icon.
- A provider with no logo, such as a custom command, or a registry icon that did not load, gets the Lucide `bot` glyph.
- A provider that is not signed in or not installed shows its logo at 60 percent opacity, with its name in `text.secondary`.

## Model picker

One component chooses a model everywhere: the composer, the New channel dialog, and Model in the channel menu.

- **Trigger.** The provider logo, the model name in medium weight, and the account name in `text.tertiary`, then a chevron. In the composer it is a chip, `component.composer_chip` (28 px) high, with `md` corners and no ground until hover. While its popover is open, the chip has `surface.selected`. In a dialog it is a select field, as wide as its column. A dialog field with no model of its own says "Same as Everything", with Everything's model in `text.tertiary`. A channel that uses Everything's model shows "Same as Everything" before the chip in the composer.
- **Popover.** `component.model_picker` (440 px) wide and `component.model_picker_height` (420 px) high, `surface.raised`, `lg` corners, `border.strong`, `shadow.float`. A rail is at the left, and the main pane fills the rest.
- **Rail.** `component.picker_rail` (48 px) wide, on `surface.card`, with a `border.subtle` line on its right. From the top: Favorites (a filled star in `accent.text`), a `border.strong` divider, then one button per signed-in account name, in the order of the inventory, with its provider logo at `component.provider_logo`. Each button is `component.picker_rail_button` (36 px) square with `md` corners. The selected button has `surface.selected` and an `accent.text` bar of `component.picker_rail_marker` by `component.picker_rail_marker_height` on the rail's right edge. The pointer on a button shows a tooltip with the account name and its workers (or Favorites and "From every account"). The rail scrolls when it has more accounts than room.
- **Badges.** When two or more accounts have the same provider, each of their buttons has a badge at its bottom right: a pill of `component.picker_rail_badge` on `surface.raised` with a `border.strong` line, in semibold text of `component.picker_rail_badge_text`. The text is `text.primary` on the selected button and `text.secondary` on the others. The label comes from the account names (`picker::badge_labels`): keep the words that no other account of that provider has, after a number joins the word before it. Take the first letter of each kept word in upper case, plus the digits at the end of the word, and cut it to 3 characters. So "Claude Code Personal 1" and "Claude Code Personal 2" give P1 and P2, and "Claude Work" next to them gives W. An account with no word of its own, or two accounts with the same label, get their position (1, 2).
- **Search.** At the top of the main pane: a search icon and the field "Search every account". Typing filters by model name, model ID, account name and provider name, hides the rail, and shows the matches from every account as rows of the Favorites kind. Escape clears the search first, and the rail comes back. A second Escape closes only the popover.
- **Header line.** Under the search field, outside the list, so it stays there while the list scrolls (this is the sticky header of a long account). It has a title in UI size and semibold, and a detail in caption style and `text.tertiary`: the account name and its workers, "Favorites" and "From every account", or "Search results" and "From every account". A channel with its own model has the text button Use the Everything model at the right end of the line, in every view.
- **Account rows.** The model name, the effort range in caption style (such as "Low to Max effort"), a star and a check. The selected row has `surface.selected`, the name in medium weight, and an `accent.text` check.
- **Favorites and search rows.** The provider logo at 14 px, the model name, then the account name in caption style and `text.tertiary`, then the star and the check. The Favorites list ends with the note "Star a model in any account to add it here.", which is also its empty state.
- **Stars.** A favorite has a filled star in `accent.text`. Another row shows an outline star in `text.secondary` only while the pointer or the keyboard is on the row. A click on the star adds or removes the favorite. It does not choose the model or close the popover.
- **First view.** The picker opens on Favorites when at least one favorite has a row, else on the account of the current model, with the keyboard on the current model.
- **Keyboard.** Up and Down move in the list. Left (with an empty search) moves to the rail, where Up and Down change the view and the selected button shows the focus ring. Right, Enter or typing go back to the list. Enter in the list chooses.
- **Scroll.** The list scrolls inside the popover. It fades into `surface.raised` at the top and the bottom while more rows are there.
- **No account.** "No account is signed in. Add one in Settings > Workers." with the button Open Settings. The popover then has no rail, and it is as high as its content.
- **Effort.** A second chip next to the model chip: the Lucide `signal` icon and the level. Its menu is `component.effort_menu` (240 px) wide, with the title "Effort for <model>", one row per level, "Default" after the provider's default level, and the note "<Provider> offers these levels for this model. More effort thinks longer and uses more of your plan." When the provider does not name its default, the first row is Default, which takes the effort out. The chip is not there for a model with no effort levels. In a dialog, the effort is a select of `component.effort_select` (150 px) next to the model field. A new model keeps the effort when it offers that level. Otherwise the effort goes back to the new model's default.
- **Task threads** show the task's model under the reply box as text, such as "Runs with Opus 5.5, Claude Personal, High effort". A task keeps the model it started with.

## Worker tags

- A user tag is a chip with `sm` corners and a `border.strong` border, with an x to remove it. A user tag that a channel requires shows in the accent: `tint.accent` ground, `accent.text` border and text. Add tag is a chip with a dashed border.
- A detected tag is mono caption text on `surface.card`, with no border. The user cannot remove it.
- In the New channel dialog and the Worker tags dialog of a channel, a chosen tag is an accent pill with `full` corners. Add tag opens a menu of every worker tag: the user tags, a line, then the detected tags. The note under the field says how many workers match, and turns `status.failed` when none does.

## Corners

| Token | Radius (px) | Use |
|---|---|---|
| `sm` | 4 | Badges, inline code and keyboard keys |
| `md` | 6 | Buttons, inputs, sidebar rows and list rows |
| `lg` | 10 | Cards, your messages, menus and popovers |
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

The Working arc is the only motion that runs by itself. It turns once every 1.2 seconds. If the system setting to reduce motion is on, nothing moves: elements fade in 120 ms, and the Working arc stands still.

## Icons and glyphs

- Icons come from Lucide, as in GPUI Kit. They are 16 px with a 1.5 px stroke. Dense lists use 14 px, and toolbars use 20 px. We export the Lucide SVGs with a 1.5 px stroke, because GPUI draws an SVG as it is.
- Consort's own glyphs in `design/glyphs/` use the same 16 px grid and the same stroke.
- An icon takes the color of the text next to it, except a status glyph, which takes its status color.

## Logo

The mark is a brace that joins three parts. In a score, a brace joins the staves that play together. To a developer, a brace opens a block. Both readings fit: one assistant that joins many parts.

- **Mark** (`design/logo/mark.svg`). A filled brace, thick in the stems and thin at the ends and at the point, drawn on a 64 px grid. Use it at 24 px and larger.
- **Small sizes.** Under 24 px (the sidebar, the favicon and the macOS menu bar), use the line glyph `design/glyphs/brace.svg`.
- **App icon** (`design/logo/app-icon-dark.svg`, `design/logo/app-icon-light.svg`). The mark at 60 percent size on `surface.card`, in `accent.fill` for dark and `accent.text` for light.
- **Color.** One color only: `accent.fill` on dark surfaces, `accent.text` on light surfaces, or plain black or white. The macOS menu bar uses the black glyph as a template image.
- **Wordmark.** "consort" in lowercase, Atkinson Hyperlegible Next 600, tracking -0.02 em, to the right of the mark. The mark is 1.2 times the font size high, with a gap of 0.35 times the font size.
- **Clear space.** Keep space equal to three bar thicknesses (13.5 px at 64 px) around the mark.
- **Do not** add gradients, outlines or shadows, rotate the mark, change its proportions, or put it on a status color.

## Text in the UI

- Buttons say what happens, with a verb: **Allow once**, **Deny**, **Resolve**, **Start task**. An action keeps its name through a flow, so **Resolve** leads to "Resolved".
- Do not join details with middle dots. Use a comma, or put the details on separate lines.
- Do not add arrows to button or link text.
- An error says what happened and what to do next. An empty state names the situation and the next action.
- Write in the style of ASD-STE100: short sentences, one instruction per sentence, and the same word for the same thing everywhere.
