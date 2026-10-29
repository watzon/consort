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

## Rust rules

- Edition 2024, Rust 1.98.1 (see `rust-toolchain.toml`). `unsafe_code` is forbidden in every crate.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` must pass before a change is done. Format with `cargo fmt`.
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
   - A message: Copy text, Copy as Markdown, Reply in thread, Quote in reply. Your own message also has Edit and Delete.
   - A code or command block: Copy code.
   - A task card or task row: Open thread, Resolve (Ready for review only) or Not done (resolved only), Retry (failed only), Stop (working only), Archive (failed or resolved only), Copy task ID.
   - A channel row: Rename, Instructions, Change icon, Move to group, Archive, Delete.
   - A worker row: Rename, Edit tags, Pause, Remove.
   - The composer: Cut, Copy, Paste, Select all.
3. **The chat never has a hard edge.** The top and bottom of the timeline fade into the canvas, so text never stops at a sharp line. GPUI has no blur filter for elements, so use layered gradients from the canvas color to transparent (about 24 px at the top and 32 px above the composer).
4. **Dropdowns flip.** Any menu, popover or select that can run past the window edge opens downward when there is room and upward when there is not. Use GPUI's anchored positioning with anchor switching, not a fixed offset.
5. **Colors and sizes come from tokens.** Read colors from the active `consort-theme` theme, and sizes from `design/structure.json` through `consort-theme`. No hex values or ad hoc pixel sizes in view code.
6. **Copy follows the design system.** Sentence case, verbs on buttons, no all-caps labels, no middle dots between details, no arrows in button text. Errors say what happened and what to do next.

## Writing

Write docs, comments and UI text in plain ASD-STE100 style: short sentences, one instruction per sentence, no em dashes. Use the terms in `CONTEXT.md`.
