# Consort

Consort is a personal AI assistant: one hub (on a server, or on the user's own computer), workers on the user's machines, and clients. Read [CONTEXT.md](CONTEXT.md) for the terms and use them in code, comments and UI text. The design lives in `docs/`:

- [docs/architecture.md](docs/architecture.md), [docs/protocol.md](docs/protocol.md), [docs/data-model.md](docs/data-model.md), [docs/harness-adapters.md](docs/harness-adapters.md), [docs/design-system.md](docs/design-system.md)
- [docs/adr/](docs/adr/) for decisions that are hard to reverse
- [docs/DECISIONS.md](docs/DECISIONS.md) for build decisions that the user has not reviewed yet
- [docs/BUILD-PLAN.md](docs/BUILD-PLAN.md) for the build order and status

If the code and a doc disagree, fix the one that is wrong in the same change, and say which one.

## Crates

| Crate | Owns |
|---|---|
| `consort-proto` | Wire types for the client API, the worker link and the hub tools, and the release names and log rule that all programs share. No I/O |
| `consort-theme` | Theme files, contrast checks, the chart roles, the fonts, and the GPUI Kit theme that the app uses. GPUI is behind the `gpui` feature (on by default), so the roles and the fonts build without it |
| `consort-visual` | The page contract of a visual: the page check, local images, the bootstrap, the theme variables, the ink kit. No I/O |
| `consort-harness` | The `Harness` and `HarnessSession` traits and the adapters (ACP first) |
| `consort-tls` | The pin of a self-signed hub: the certificate hash and the TLS verifier that the client and the link share |
| `consort-tools` | Managed tools: downloads, checks, install layout, launch specs and the ACP registry types |
| `consort-link` | The dial-out link that the session host and the worker share: pairing, keys, spool, reconnect |
| `consort-session-host` | The session host on the hub machine: accounts, sign-ins, managed CLIs, sessions in a boundary, the MCP endpoint, the tool surface |
| `consort-worker` | The execution worker: journal, processes, files, workspaces, inventory. It has no account and no CLI |
| `consort-hub` | The hub server: SQLite store, feed, pairing, coordinator queue, placement, hub tools, routines |
| `consort-dictation` | One live dictation on the Sayso SDK: states, the three parts of the text, models, settings of this computer. No GPUI and no hub I/O |
| `consort-client` | Client logic shared by the desktop app and, later, mobile: connection, cache, view state, the setup gate, the local hub module |
| `consort-desktop` | The GPUI desktop app |
| `consort-cli` | The `consort` binary: `hub`, `worker`, `session-host`, `local`, `dev` and `mcp-bridge` subcommands |
| `consort-testkit` | A fake harness, a fake worker and helpers for integration tests |

Dependency direction: `proto` and `theme` depend on no other Consort crate, `tls` depends only on `proto`, and `visual` depends only on `proto` and `theme` (without GPUI). `client` never depends on GPUI. `desktop` never talks to the network; it calls `client` and `dictation` (and reads constants from `proto`, such as the log rule). `hub` never runs a model; it asks the session host, a separate process on the hub machine. `worker` never holds an account or a CLI. A session host and a worker have separate routes and credentials.

## Run the app

`scripts/dev.sh` builds the debug binaries, starts a dev hub with its worker, and starts the desktop app. Quitting the app stops the hub. The first run prints a setup code to type in the app. The comment at the top of the script lists its settings. `scripts/dev.sh --mock` starts only the app, with the designs' content and no hub.

A hub can also run on the computer of the app: `consort local` (the app runs it) puts the hub, the session host and a worker in one per-user service. Installs, releases, `consort local` and app updates are in [docs/install.md](docs/install.md). Running a hub on a server (`scripts/install/hub.sh` or by hand) is in [docs/deploy.md](docs/deploy.md). `scripts/install/test-worker.sh` and `scripts/install/test-hub.sh` test the install scripts with no root and no network. `scripts/bundle-macos.sh` builds `Consort.app`.

## Rust rules

- Edition 2024, Rust 1.98.1 (see `rust-toolchain.toml`). `unsafe_code` is forbidden in every crate.
- `scripts/check.sh` must pass before a change is done. It runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and the tests with `cargo nextest run` (install it with `brew install cargo-nextest` or `cargo install cargo-nextest --locked`). While you work, check only the crates you change: `scripts/check.sh consort-hub consort-proto`. Format with `cargo fmt`.
- Library crates return typed errors (`thiserror`). Only binaries and tests use `anyhow`.
- No `unwrap` or `expect` outside tests, except for an invariant that a comment proves.
- Pin GPUI Kit and other fast-moving crates to exact versions (`=x.y.z`) in the workspace `Cargo.toml`.
- Never read or log harness credential files (`auth.json`, keychain entries, `.credentials.json`). The session host only asks each harness whether it is signed in.
- Every new behavior gets a test at the lowest level that can prove it. Integration tests use `consort-testkit`.

## GPUI rules

These come from the user's experience with GPUI. Follow them in every view.

1. **Text selection is explicit.** GPUI text is not selectable unless you make it so. Make these selectable: message text (assistant and yours), code blocks, command blocks in approvals, task thread posts, memory notes, error details, IDs and paths that a user may copy. Do not make chrome selectable: sidebar rows, buttons, tabs, labels, timestamps.
2. **Context menus are planned, not accidental.** Every surface below has a right-click menu. Keep the item names in sentence case and the order fixed.
   - Selected text: Copy, Quote in reply, Ask Consort about this.
   - A message: the six quick emoji and More above the items, then Copy text, Copy as Markdown, Reply in thread, Quote in reply. Your own message also has Edit and Delete. A reply in a thread has no Reply in thread.
   - A code or command block: Copy code.
   - A task card or task row: Open thread, Resolve (Ready for review only) or Not done (resolved only), Retry (failed, or Ready for review or Resolved whose session ended), Stop (working only), Archive (failed or resolved only), Copy task ID, Delete.
   - A channel row: Rename, Instructions, Model, Worker tags, Permissions, Connectors and skills, Change icon, Move to group, Archive, Delete. Rename, Instructions, Model and Worker tags open the channel settings dialog. Permissions opens the Permissions dialog. Connectors and skills opens the Connectors tab of the Channel panel. Change icon opens the All icons dialog.
   - The "+" of the Tutti row (a left click) and the Tutti row (a right click): New channel, New group.
   - A group row (its "..." button, a left click, or a right click on the row): New channel, Rename, Delete group.
   - A command rule row (Permissions dialog): Edit, Remove. A rule from Tutti has only Override here.
   - An exception row (Permissions dialog): Remove.
   - A connection row (Settings > Connectors): Reconnect, Activity, Copy handle, Disconnect.
   - A skill row (Settings > Skills): Turn off (Turn on when it is off), View files, Remove.
   - A skill source row (Settings > Skills): Remove. Only a source that the user added has it.
   - An account row (Settings > Providers): Account settings, Rename, Sign out, Remove.
   - A tool row (Settings > Providers): Check for updates, Copy install path, Reinstall. A tool that is not installed has only Install.
   - An endpoint row (Settings > Providers): Edit, Copy base URL, Test connection, Remove.
   - A worker row: Rename, Edit tags, Pause, Remove.
   - A step row (Settings > Models): Move up, Move down, Remove. The first row, the built-in row and the end row have no menu.
   - The model chip of an assistant on a fallback (a left click): Go back to the first choice (off until the reset), Choose another model, Edit path.
   - The composer: Cut, Copy, Paste, Select all.
   - An attachment in a message or a thread post: Open, Save, Save to channel files. An artifact is in the channel files already, so it has no Save to channel files.
   - An attachment chip in the composer or the reply box: Try again (failed only), Remove.
   - A visual (in the timeline, in a task thread and in the light box): Open full screen, Open in browser, Copy image, Save image, Save as HTML. The light box has no Open full screen. A right click on a live page opens the same menu, or the menu of selected text when the user selected text in the page.
   - A file row (Channel panel > Files): Open, Download, Rename, Move, Delete. A folder row and a task row of the artifacts have no menu.
   - The add button of the composer and the reply box (a left click): Upload from this computer, Choose from channel files.
   - A routine row and a routine opened in the panel or in Settings: Run now, Run history, Edit, Turn off (Turn on when it is off), Delete. A check-in has no Edit: the hub sets its prompt.
   - A memory note (in a tree, on its page and in Settings > Memory): Version history, Copy path, Rename, Delete.
   - A Monid call row (in the chat and in a task thread): Copy input, Copy output, Copy run ID. A call that Monid can stop also has Stop.
   - A Monid tool row (Channel panel > Connectors > Monid): Remove.
3. **The chat never has a hard edge.** The top and bottom of the timeline fade into the canvas, so text never stops at a sharp line. GPUI has no blur filter for elements, so use layered gradients. At the top, a progressive tint `layout.top_tint` (the canvas at 97 percent at the top, 94 percent at the bottom of the mark, 80 percent at the bottom of the pill, and clear at 108 px, drawn as three gradient layers) lies under the Three voices mark, the activity pill and the top actions; content under it stays faintly visible. At rest the first item starts at `layout.content_top` (98 px). Above the composer, `layout.fade_bottom` (32 px). Lists that scroll inside a panel or a dialog fade at their edges too.
4. **Dropdowns flip.** Any menu, popover or select that can run past the window edge opens downward when there is room and upward when there is not. Use GPUI's anchored positioning with anchor switching, not a fixed offset.
5. **Colors and sizes come from tokens.** Read colors from the active `consort-theme` theme, and sizes from `design/structure.json` through `consort-theme`. No hex values or ad hoc pixel sizes in view code.
6. **Copy follows the design system.** Sentence case, verbs on buttons, no all-caps labels, no middle dots between details, no arrows in button text. Errors say what happened and what to do next.
7. **Text wraps, and never pushes its row.** GPUI gives a flex item the full width of its text as its minimum. So every `flex_1` also has `min_w_0`, unless its label has `whitespace_nowrap`, and a fixed control beside text has `flex_none`. Long text beside anything else goes in a `flex_1` part, so that it wraps. A test in `layout_tests.rs` checks the `flex_1` rule.

## Writing

Write docs, comments and UI text in plain ASD-STE100 style: short sentences, one instruction per sentence, no em dashes. Use the terms in `CONTEXT.md`.
