# Consort

Consort is a personal AI assistant: one hub on a server, workers on the user's machines, and clients. Read [CONTEXT.md](CONTEXT.md) for the terms and use them in code, comments and UI text. The design lives in `docs/`:

- [docs/architecture.md](docs/architecture.md), [docs/protocol.md](docs/protocol.md), [docs/data-model.md](docs/data-model.md), [docs/harness-adapters.md](docs/harness-adapters.md), [docs/design-system.md](docs/design-system.md)
- [docs/adr/](docs/adr/) for decisions that are hard to reverse
- [docs/DECISIONS.md](docs/DECISIONS.md) for build decisions that the user has not reviewed yet
- [docs/BUILD-PLAN.md](docs/BUILD-PLAN.md) for the build order and status

If the code and a doc disagree, fix the one that is wrong in the same change, and say which one.

## Crates

| Crate | Owns |
|---|---|
| `consort-proto` | Wire types for the client API, the worker link and the hub tools. No I/O |
| `consort-theme` | Theme files, contrast checks, and the GPUI Kit theme that the app uses |
| `consort-harness` | The `Harness` and `HarnessSession` traits and the adapters (ACP first) |
| `consort-worker` | The worker daemon: hub link, spool, local MCP endpoint, workspaces, inventory |
| `consort-hub` | The hub server: SQLite store, feed, pairing, coordinator queue, placement, hub tools, routines |
| `consort-client` | Client logic shared by the desktop app and, later, mobile: connection, cache, view state |
| `consort-desktop` | The GPUI desktop app |
| `consort-cli` | The `consort` binary: `hub`, `worker`, `dev` and `mcp-bridge` subcommands |
| `consort-testkit` | A fake harness, a fake worker and helpers for integration tests |

Dependency direction: `proto` and `theme` depend on no other Consort crate. `client` never depends on GPUI. `desktop` never talks to the network; it calls `client`. `hub` never runs a model; it asks a worker.

## Run the app

`scripts/dev.sh` builds the debug binaries, starts a dev hub with its worker, and starts the desktop app. Quitting the app stops the hub. The first run prints a setup code to type in the app. The comment at the top of the script lists its settings. `scripts/dev.sh --mock` starts only the app, with the designs' content and no hub.

Installs, releases and app updates are in [docs/install.md](docs/install.md). Running a hub on a server is in [docs/deploy.md](docs/deploy.md). `scripts/bundle-macos.sh` builds `Consort.app`.

## Rust rules

- Edition 2024, Rust 1.98.1 (see `rust-toolchain.toml`). `unsafe_code` is forbidden in every crate.
- `scripts/check.sh` must pass before a change is done. It runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and the tests with `cargo nextest run` (install it with `brew install cargo-nextest` or `cargo install cargo-nextest --locked`). While you work, check only the crates you change: `scripts/check.sh consort-hub consort-proto`. Format with `cargo fmt`.
- Library crates return typed errors (`thiserror`). Only binaries and tests use `anyhow`.
- No `unwrap` or `expect` outside tests, except for an invariant that a comment proves.
- Pin GPUI Kit and other fast-moving crates to exact versions (`=x.y.z`) in the workspace `Cargo.toml`.
- Never read or log harness credential files (`auth.json`, keychain entries, `.credentials.json`). The worker only asks each harness whether it is signed in.
- Every new behavior gets a test at the lowest level that can prove it. Integration tests use `consort-testkit`.

## GPUI rules

These come from the user's experience with GPUI. Follow them in every view.

1. **Text selection is explicit.** GPUI text is not selectable unless you make it so. Make these selectable: message text (assistant and yours), code blocks, command blocks in approvals, task thread posts, memory notes, error details, IDs and paths that a user may copy. Do not make chrome selectable: sidebar rows, buttons, tabs, labels, timestamps.
2. **Context menus are planned, not accidental.** Every surface below has a right-click menu. Keep the item names in sentence case and the order fixed.
   - Selected text: Copy, Quote in reply, Ask Consort about this.
   - A message: the six quick emoji and More above the items, then Copy text, Copy as Markdown, Reply in thread, Quote in reply. Your own message also has Edit and Delete. A reply in a thread has no Reply in thread.
   - A code or command block: Copy code.
   - A task card or task row: Open thread, Resolve (Ready for review only) or Not done (resolved only), Retry (failed, or Ready for review or Resolved whose session ended), Stop (working only), Archive (failed or resolved only), Copy task ID, Delete.
   - A channel row: Rename, Instructions, Model, Worker tags, Connectors and skills, Change icon, Move to group, Archive, Delete. Rename, Instructions, Model and Worker tags open the channel settings dialog.
   - The "+" of the Tutti row (a left click): New channel, New group.
   - A group row (its "..." button, a left click): New channel, Rename, Delete group.
   - A connection row (Settings > Connectors): Reconnect, Activity, Copy handle, Disconnect.
   - A skill row (Settings > Skills): Turn off (Turn on when it is off), View files, Remove.
   - A skill source row (Settings > Skills): Remove. Only a source that the user added has it.
   - An account row (Settings > Workers): Account settings, Rename, Sign out, Remove.
   - A worker row: Rename, Edit tags, Pause, Remove.
   - The composer: Cut, Copy, Paste, Select all.
   - An attachment in a message or a thread post: Open, Save, Save to channel files. An artifact is in the channel files already, so it has no Save to channel files.
   - An attachment chip in the composer or the reply box: Try again (failed only), Remove.
   - A file row (Channel panel > Files): Open, Download, Rename, Move, Delete. A folder row and a task row of the artifacts have no menu.
   - The add button of the composer and the reply box (a left click): Upload from this computer, Choose from channel files.
   - A routine row and a routine opened in the panel: Run now, Run history, Edit, Turn off (Turn on when it is off), Delete. A check-in has no Edit: the hub sets its prompt.
   - A memory note (in a tree, on its page and in Settings > Memory): Version history, Copy path, Rename, Delete.
3. **The chat never has a hard edge.** The top and bottom of the timeline fade into the canvas, so text never stops at a sharp line. GPUI has no blur filter for elements, so use layered gradients. At the top, a progressive tint `layout.top_tint` (the canvas at 97 percent at the top, 94 percent at the bottom of the mark, 80 percent at the bottom of the pill, and clear at 108 px, drawn as three gradient layers) lies under the Three voices mark, the activity pill and the top actions; content under it stays faintly visible. At rest the first item starts at `layout.content_top` (98 px). Above the composer, `layout.fade_bottom` (32 px). Lists that scroll inside a panel or a dialog fade at their edges too.
4. **Dropdowns flip.** Any menu, popover or select that can run past the window edge opens downward when there is room and upward when there is not. Use GPUI's anchored positioning with anchor switching, not a fixed offset.
5. **Colors and sizes come from tokens.** Read colors from the active `consort-theme` theme, and sizes from `design/structure.json` through `consort-theme`. No hex values or ad hoc pixel sizes in view code.
6. **Copy follows the design system.** Sentence case, verbs on buttons, no all-caps labels, no middle dots between details, no arrows in button text. Errors say what happened and what to do next.

## Writing

Write docs, comments and UI text in plain ASD-STE100 style: short sentences, one instruction per sentence, no em dashes. Use the terms in `CONTEXT.md`.
