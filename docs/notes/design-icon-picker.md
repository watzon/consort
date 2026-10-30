# Design notes: icon picker

These are the Paper designs for a better channel icon picker. They change the New channel dialog (`EYQ-0`) and the same picker in the channel settings dialog (`CYJ-0`). The Paper file is `01M6342D512B1CKH0D29HMDN8V`, page Desktop. The row starts at worldX -720, worldY 15612, below the voice row (the tallest artboard there ends at 15532). The gap between artboards is 80 px.

The user asked for two ways to choose from every icon. Today the picker has seven tiles and a "More" button. "More" opens a menu with nine old icon names. Both options below let the user choose any of the 85 icons in `crates/consort-desktop/assets/icons/`. (The brief said 84. The folder holds 85 `.svg` files. `bot` is the only one that is not hand drawn.)

## Artboards

| Artboard | ID | What it shows |
|---|---|---|
| Icon picker A, inline, dark | `I0L-0` | Option A in four dialogs side by side (2080 by 720 px). 1: collapsed, with a hovered tile, the selected tile and a keyboard focus ring. 2: expanded, search empty, a grid that scrolls in a fixed height with top and bottom fades. 3: expanded, search "serv", three results. 4: expanded, search "xyz", the empty state. |
| Icon picker B1, All icons dialog, dark | `IIJ-0` | Option B. The New channel dialog dims and moves up. The "All icons" dialog sits over it: a title, a search field with focus, six categories with section labels, all icons in a grid, one selected tile, one hovered tile with its tooltip, a fade at the bottom, a hint line and Cancel. |
| Icon picker B2, All icons with search, dark | `IQF-0` | The same dialog with the search "arrow". Only the three categories that have matches stay (9 icons). The search field shows the count and a clear button. One tile has the keyboard focus ring, one is hovered with its tooltip. |
| Icon picker B3, after a pick, dark | `IAN-0` | The New channel dialog after a pick. The picked icon (`connector`) is in the last quick slot and is selected. Focus is back on "All icons". |

Screenshots: `docs/screenshots/design-icon-picker-a-inline.png`, `design-icon-picker-b1-all-icons.png`, `design-icon-picker-b2-search.png` and `design-icon-picker-b3-after-pick.png`.

The icons in the artboards are the real files from the icon set, one version (v0), set to `text.secondary` or `text.primary`. They are image fills, not vectors, so the colors in them are the hex values of the tokens. The hand-drawn "boil" shows only in the app.

## States (both options)

| State | Look |
|---|---|
| Rest | The icon in `text.secondary`, no ground, no border. The tile is 38 px in the quick row and 36 px (A) or 40 px (B) in the grid. The icon is 18 px (A) or 20 px (B). |
| Hover (the active state) | `accent.fill` at `opacity.tint` (16 percent) as the ground, `radius.md` corners, the icon in `text.primary`. The icon shimmers: it plays its three versions (v0, v1, v2) in turn at 8 frames a second, as the shared stop-motion clock does. It stands still with Reduce motion, and then only the ground and the color show. |
| Selected | `surface.selected` ground, a 1 px `text.secondary` border, the icon in `text.primary`. It does not shimmer. If the mouse is over the selected tile, the hover ground shows as well. |
| Keyboard focus | The focus ring: a 2 px `accent.text` line outside the tile, so the layout does not move. |

The user asked for the selected icon to show the active state, and then said hover is better. I followed the second idea. The accent means your turn, and a selected tile that shimmers all the time would compete with the hover. So hover has the accent and the shimmer, and the selected tile has the quiet selection style that the dialog has today. The selected style and the focus ring stay apart, because the ring is the only accent line.

## Decisions

### Both options

- **The quick row keeps 7 icons:** `code`, `globe`, `server`, `home`, `memory`, `appearance`, `star`. They cover software projects, web, a homelab, a house, notes and reading, design, and "anything else I like". The old row had `globe`, `memory`, `server`, `worker`, `appearance`, `routine` and `bell`. Four stay. `worker`, `routine` and `bell` are Consort terms for other things, so they are easy to find in the full list but are not the best first choices.
- **Search matches names and keywords.** A word matches when it is the start of a word in the name or in a keyword ("serv" finds `server`, `connector` (service) and `worker` (build server)). The match is not case sensitive. Results keep the category order, with name matches first. No match gives the message `No icon matches "xyz". Try another word.`
- **The picked icon fills the last quick slot.** When the user picks an icon that is not in the quick row, it replaces the 7th tile (`star`) for this dialog, and it is selected. If the user then picks a quick icon, the 7th tile goes back to `star`. The row keeps its width and its order, so nothing jumps. The channel settings dialog does the same: when the saved icon is not in the quick row, it shows in the 7th slot.
- **The default selected icon** is the first one in the row, `code`, unless the product picks a default. The mock shows `memory`, as `EYQ-0` does.
- **The channel settings dialog** uses the same field and the same behavior. It is 880 px wide with two columns, so Option A would grow its left column. Option B does not change its size.
- **Group variant (`FWA-0`).** The only difference is the line "In Learning" under the title. The picker does not change.

### Option A, inline

- "More" stays where it is, with a small chevron. After the click, the quick row is gone and "Less" shows at the right of the "Icon" label. One place for the quick row and one for the full grid, so the user never sees both.
- The grid is 10 columns of 36 px tiles with 4 px gaps (396 px) and a 14 px lane for the scroll bar. The viewport is 168 px high: four rows and a small part of the fifth. The fifth row shows through the bottom fade, so it is clear that the grid scrolls. The top fade shows only after the user scrolled.
- The grid scrolls to the selected icon when it opens.
- The search field has focus when the grid opens. Down arrow moves focus into the grid. The arrow keys move through tiles, and Enter picks.
- The height stays the same during a search (168 px). The dialog does not jump while the user types. The count ("3 icons") is in the search field.
- Categories do not show in the inline grid. They only set the order. There is no room for labels in 168 px.
- Dialog height: 363 px collapsed, 533 px expanded. Both fit in a 900 px window. At the smallest window height, the dialog scrolls as a whole.

### Option B, picker dialog

- **Name: tooltip, not a label under each tile.** Reasons: (1) Names such as `chevrons-up-down` and `pull-request` are 14 to 16 characters and do not fit under a 40 px tile. A label would need 8 columns of 76 px tiles, and the list would be about 1.7 times longer. (2) The icon is the thing the user looks for. A name under every tile adds 85 small words to read. (3) The name shows on hover and on keyboard focus after 300 ms, so the user who needs the name can get it. (4) Search finds icons by name and by keyword, so the user does not have to read names to find one.
- The dialog is 560 px wide. The list is 11 columns of 40 px tiles with 6 px gaps, and the section labels are in Label size and `text.tertiary`. The list is 300 px high, about six rows.
- The dialog opens over the New channel dialog. The first dialog dims (a second scrim at 40 percent) and moves up 150 px. This keeps its Icon row visible, so the user sees "All icons" as pressed and sees the result. The second dialog is placed 350 px from the top, so it does not hide the first one. In the app, this is a stacked modal: Escape closes only the top one.
- Search has focus when the dialog opens. Down arrow moves into the list, the arrow keys move between tiles, and Enter or a click picks the icon and closes the dialog. Escape or Cancel closes it with no change. The close x does the same.
- There is no "Choose" button. A click picks and closes, because that is the user's goal. Cancel stays as a way out. The hint line says "Click an icon to pick it. Press Esc to close."
- The quick row button is called "All icons" (not "More"), because it opens something. It keeps the dashed border of today's "More".
- Search results keep the category labels, and categories with no match hide.
- The list has a 4 px padding so the focus ring at the edge is not cut.

## Recommendation: B for the first release, with A's quick-row rules

Choose **Option B**. Reasons:

1. **Room.** 85 icons need about 10 rows. Option A shows 4 rows in a dialog that is only 460 px wide, so the user scrolls a small window inside a dialog. Option B gives the list its own space (about 6 rows, wider tiles, section labels).
2. **Categories.** The labels help the user who does not know what to search for. Option A has no room for them.
3. **The first dialog stays small and stable.** The New channel dialog is for two things: a name and an icon. Most users pick from the quick row and never open the picker. Option A makes this dialog 170 px taller for the few users who expand it, and the Name field moves down while they choose.
4. **The same picker works in the channel settings dialog** without a change to that dialog's size.
5. **Fewer states in the small dialog.** Option B adds one button state (pressed) to the first dialog. Option A adds a second layout, a search field and four sub-states to it.

The cost of B is a second modal on top of the first. This needs a stacked modal in GPUI and a rule that Escape closes only the top one. If the stacked modal is hard to build, build A first. The search rules, the keywords, the categories and the tile states are the same in both options, so most of the work carries over.

## Copy

| Where | Text |
|---|---|
| Quick row button, Option A | More (with a chevron). After the click: Less |
| Quick row button, Option B | All icons |
| Search field | Search icons |
| Count in the search field | 3 icons, 1 icon |
| No match | No icon matches "xyz". Try another word. |
| Picker title (B) | All icons |
| Hint line (B) | Click an icon to pick it. Press Esc to close. |
| Picker button (B) | Cancel |

## Icon categories and keywords

Every icon is in one category. The order in the list is the order below. The first line of the keywords is the icon's name in words. Keywords are lower case and in English. A word matches when it starts with the typed text.

### Work and code (16)

| Icon | Keywords |
|---|---|
| `code` | code, programming, dev, software, brackets |
| `terminal` | terminal, shell, command, cli, console |
| `branch` | branch, git, version, fork |
| `pull-request` | pull request, review, merge, git |
| `file` | file, document, page, paper |
| `folder` | folder, directory, files, project |
| `tasks` | tasks, list, checklist, todo |
| `compose` | compose, write, draft, new |
| `edit` | edit, pencil, change, write |
| `skill` | skill, ability, spark, tool |
| `routine` | routine, schedule, metronome, repeat, tempo |
| `calendar` | calendar, date, day, plan, schedule |
| `clock` | clock, time, hour, wait, follow-up |
| `effort` | effort, level, bars, strength, speed |
| `archive` | archive, box, store, old |
| `memory` | memory, notes, notebook, reading, remember, journal |

### Machines (7)

| Icon | Keywords |
|---|---|
| `server` | server, rack, host, homelab, nas |
| `worker` | worker, laptop, machine, computer, build server, remote |
| `chip` | chip, cpu, gpu, hardware, processor |
| `bot` | bot, robot, assistant, ai, agent |
| `keyboard` | keyboard, type, keys, input |
| `connector` | connector, service, integration, plug, mcp |
| `globe` | globe, web, world, internet, site, language |

### Communication (12)

| Icon | Keywords |
|---|---|
| `message` | message, chat, talk, comment, bubble |
| `mail` | mail, email, letter, inbox |
| `send` | send, post, submit, arrow |
| `reply` | reply, answer, respond, back |
| `forward` | forward, share, pass on |
| `voice` | voice, microphone, mic, speak, audio, dictation |
| `bell` | bell, alert, notification, ring, reminder |
| `users` | users, people, team, group, family |
| `user` | user, person, profile, account, me |
| `channel` | channel, hash, tag, topic |
| `attach` | attach, paperclip, clip, attachment |
| `quote` | quote, citation, saying, text |

### Places and things (14)

| Icon | Keywords |
|---|---|
| `home` | home, house, building, smart home, lights |
| `sun` | sun, day, light, morning, bright |
| `moon` | moon, night, dark, sleep, evening |
| `star` | star, favorite, best, mark |
| `star-filled` | star filled, favorite, important, pinned |
| `pin` | pin, keep, place, location, map |
| `image` | image, picture, photo, art, gallery |
| `tag` | tag, label, price, name |
| `key` | key, secret, password, access, token |
| `lock` | lock, private, secure, closed |
| `unlock` | unlock, open, public, unsecured |
| `eye` | eye, watch, view, look, check-in, monitor |
| `eye-off` | eye off, hide, hidden, private, ignore |
| `appearance` | appearance, palette, theme, color, design, paint |

### Status and actions (18)

| Icon | Keywords |
|---|---|
| `check` | check, tick, yes, ok, correct |
| `done` | done, finished, complete, ready, ring |
| `failed` | failed, error, cross, wrong, broken |
| `needs-input` | needs input, fermata, hold, waiting, question |
| `working` | working, busy, progress, running, spinner |
| `warning` | warning, caution, danger, alert, triangle |
| `info` | info, information, about, details |
| `help` | help, question, support, faq, guide |
| `pause` | pause, hold, wait, break |
| `play` | play, start, run, go, media |
| `stop` | stop, halt, end, square |
| `retry` | retry, again, repeat, refresh, redo, reload |
| `download` | download, save, get, in, arrow down |
| `upload` | upload, publish, out, arrow up |
| `search` | search, find, look up, magnifier, zoom |
| `trash` | trash, delete, bin, remove, garbage |
| `copy` | copy, duplicate, clone, clipboard |
| `link` | link, chain, url, connect, anchor |

### Controls and arrows (18)

| Icon | Keywords |
|---|---|
| `back` | back, previous, return, arrow left |
| `chevron-down` | chevron down, arrow, open, expand, below |
| `chevron-left` | chevron left, arrow, previous, back |
| `chevron-right` | chevron right, arrow, next, forward |
| `chevron-up` | chevron up, arrow, close, collapse, above |
| `chevrons-up-down` | chevrons, up down, select, swap, arrows |
| `close` | close, cross, x, cancel, dismiss |
| `minus` | minus, subtract, less, remove, dash |
| `plus` | plus, add, new, more, create |
| `more-horizontal` | more, dots, menu, ellipsis, three dots |
| `more-vertical` | more, dots, menu, ellipsis, kebab |
| `sort` | sort, order, arrange, rank |
| `sliders` | sliders, tune, adjust, settings, controls, mixer |
| `sidebar` | sidebar, panel, layout, side, columns |
| `filter` | filter, funnel, narrow, refine |
| `settings` | settings, gear, cog, preferences, config |
| `log-out` | log out, sign out, exit, leave, door |
| `external-link` | external link, open, new window, outside |


## Build (2026-10-30)

The user chose Option B. Built in `crates/consort-desktop/src/ui/icon_picker.rs` (the dialog and the shared tile), `ui/icon_catalog.rs` (categories, keywords, search) and `icon_picker_tests.rs`. Differences from the artboards:

- The All icons dialog is centered over the New channel dialog, which dims fully under a second scrim. The design moved the first dialog up 150 px; centered reads as one clear layer on top.
- The keywords also hold the old Lucide names of the channel icons (cloud, drive, shield, bulb, book), so the old names still find an icon.
- A search with two words matches when each word starts a word of the name or of any keyword ("arrow down" finds `chevron-down` and `download`).
- Change icon in a channel's menu opens the same dialog alone; a pick saves at once. The channel settings keep the pick until Save changes.
- `bot` has one drawing, so all three versions give it and it does not shimmer.
- Screens: `--mock --screen icon-picker`, `icon-picker-search` and `icon-picked`. Screenshots: `docs/screenshots/icon-picker-dark.png`, `icon-picker-search-dark.png`.
