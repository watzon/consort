# Build plan

The first build, started 2026-10-29 while the user was away. It builds the desktop path only. Mobile comes later.

## Done means

1. `consort dev --data <dir>` starts a hub on `127.0.0.1:7600` and a worker in the same process, and it prints a setup code.
2. On first launch, the desktop app asks for the hub URL and the setup code, pairs, and opens the main window on Tutti.
3. A message in Tutti starts a coordinator turn in the session host with a harness account. The coordinator's posts appear in the timeline.
4. The coordinator can start a task. The task shows in the Tasks panel, runs on a worker, posts updates, and opens approvals and decisions that the user answers in the app. It goes to Ready for review, and the user resolves it.
5. The user can create, rename and archive channels. A channel's purpose and instructions reach its coordinator.
6. A second worker pairs with `consort worker pair <hub-url> --code <code>`. It shows in the sidebar and in Settings > Workers. Accounts and their sign-in state are in Settings > Providers.
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
| E | [HUB-PLAN.md](HUB-PLAN.md): the hub machine owns accounts and agents, worker limits in the app | Agent | Built. Claude Code is on. A real model on Linux and a token refresh did not run, so step 4 of the plan is not done. See [E2E.md](E2E.md) |
| F | Monid and access requests (ADR 0016, 0017) | Lead and agent | Built: hub, client and app. One real paid run went through the hub client. A full real turn through a session has not run |
| G | [ONBOARDING-PLAN.md](ONBOARDING-PLAN.md): first run | Agent | Phases 1 to 6 built, not committed. No real model has run the guide of phase 5. `hub.sh` has never run on a real server. The Mac keychain limit is deferred (ADR 0018) |

Agents use their own build folder to avoid lock waits: `CARGO_TARGET_DIR=target/<crate-or-agent>`. Agents do not run git. The lead commits at each integration point.

## Out of scope for this build

Mobile apps, push notifications, the native Pi adapter, the hosted relay, and the Memory and Routines settings pages beyond a basic list.
