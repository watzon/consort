# Build plan

The first build, started 2026-10-29 while the user was away. It builds the desktop path only. Mobile comes later.

## Done means

1. `consort dev --data <dir>` starts a hub on `127.0.0.1:7600` and a worker in the same process, and it prints a setup code.
2. On first launch, the desktop app asks for the hub URL and the setup code, pairs, and opens the main window on Everything.
3. A message in Everything starts a coordinator turn on a worker with a harness account. The coordinator's posts appear in the timeline.
4. The coordinator can start a task. The task shows in the Tasks panel, runs on a worker, posts updates, and opens approvals and decisions that the user answers in the app. It goes to Ready for review, and the user resolves it.
5. The user can create, rename and archive channels. A channel's purpose and instructions reach its coordinator.
6. A second worker pairs with `consort worker pair <hub-url> --code <code>`. It shows in the sidebar and in Settings > Workers, with its harness accounts and their sign-in state.
7. Spruce is the default theme, Velvet the second. Settings > Appearance switches the theme and the mode (Match system, Light, Dark).
8. The worker and the desktop app build and run on Windows (`bigboi`). A Windows worker connects to the hub on the Mac and runs a task.
9. `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass.

## Phases and owners

| Phase | Work | Owner | Status |
|---|---|---|---|
| 0 | Repo, docs, `AGENTS.md`, this plan, decisions log | Lead | Done |
| A | `consort-proto`: the full wire contract | Agent (Opus) | Done |
| A | `consort-theme`, `consort-desktop` UI on mock data, then `consort-client` | Agent (Opus) | Done |
| B | `consort-hub` | Agent (Sonnet) | Done |
| B | `consort-harness`, `consort-worker`, `consort-testkit`, `consort-cli` | Agent (Opus) | Done |
| C | End-to-end on macOS, then Windows on `bigboi` | Lead | Done, see [E2E.md](E2E.md) |
| D | Up to three review cycles with independent reviewer agents | Lead | Done, three cycles |

Agents use their own build folder to avoid lock waits: `CARGO_TARGET_DIR=target/<crate-or-agent>`. Agents do not run git. The lead commits at each integration point.

## Out of scope for this build

Mobile apps, push notifications, the native Pi adapter, the hosted relay, and the Memory and Routines settings pages beyond a basic list.
