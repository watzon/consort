# Architecture

This page explains how the system fits together and why. It is the starting point for the other design docs:

- [CONTEXT.md](../CONTEXT.md): the terms
- [data-model.md](data-model.md): entities and state machines
- [protocol.md](protocol.md): the client API, the worker link and the hub tools
- [harness-adapters.md](harness-adapters.md): how workers drive Claude Code, Codex, Pi, OpenCode and ACP agents
- [design-system.md](design-system.md): themes, color roles, type, spacing, corners, motion, glyphs and the logo
- [adr/](adr/): the decisions and the options we rejected

## The shape of the system

The system copies the user-facing shape of Claude Duet: one assistant, an Everything channel, project channels, tasks that run as their own sessions, routines and memory. It changes where the work runs. Duet runs tasks in Anthropic's cloud with Anthropic models. Here, tasks run on the user's own machines, with any harness and any model.

```
             Phone (Expo or native)        Desktop (GPUI)
                       \                      /
                        \   Client API (WSS) /
                         v                  v
                   +-----------------------------+
                   |  Hub (VPS)                  |
                   |  timeline, tasks, memory,   |
                   |  routines, placement,       |
                   |  hub tools, push            |
                   |  +-----------------------+  |
                   |  | Worker on the hub     |  |
                   |  | (coordinators run     |  |
                   |  | here by default)      |  |
                   |  +-----------------------+  |
                   +-----------------------------+
                      ^           ^           ^
         Worker link  |           |           |   (workers dial out)
                      |           |           |
              MacBook Pro      bigboi       homelab
              (worker)        (worker,      (worker,
                              Windows)       Linux)
```

There are three roles:

- **The hub** stores everything and decides what runs where. It runs on a VPS, off the home network, so it stays up and stays reachable from a phone when the home network is down. The hub runs no model itself.
- **A worker** runs sessions. It owns the harness logins, the repo checkouts, the shell and the access to its local network. Every worker dials out to the hub, so a worker needs no open ports and can sit behind NAT.
- **A client** shows the conversation and sends the user's input. Clients talk only to the hub.

The hub machine can also run a worker, the way a Kubernetes control-plane node can also run workloads. That worker is where coordinators run by default, so the assistant keeps answering when every home machine is offline.

## How a message becomes work

### The user sends a message

1. A client sends `message.send` to the hub. The hub stores the message, adds a feed entry, and sends the entry to every subscribed client.
2. The hub adds a `user_message` wake event to the channel's coordinator queue.
3. If no coordinator turn runs in that channel, the hub starts one. Events that arrive during a turn go into the next turn, so one channel never has two turns at the same time.
4. The worker that runs the coordinator gives the turn to the harness. The coordinator replies by calling `post_message` or `reply`, which the hub stores as messages. It can call `no_reply_needed` to stay silent.

One turn at a time per channel is the rule that keeps "one assistant" true. Two replies to one message cannot happen, because the second wake event waits for the first turn to end.

### The assistant starts a task

1. The coordinator calls `start_task` with a title, a brief and optional placement hints. The user can also create a task from the Tasks panel.
2. The hub creates the task in `up_next` and posts a task card in the timeline.
3. Placement picks a worker (see [Placement](#placement)). If no worker fits, the task stays in `up_next` and `status_reason` says why.
4. The hub sends `session.start` to the worker. The worker prepares the workspace, opens a harness session with the hub tools, and sends the brief as the first turn.
5. The worker streams session events. The hub stores them as session items and updates the task card. The task thread shows the task's messages, and an activity view shows the commands and file changes.
6. When the turn ends with nothing pending, the task goes to `review` and the hub wakes the coordinator with a `task_update`. The coordinator posts a short summary in the timeline.

### A harness asks for permission

1. The harness asks to run a command. The adapter emits `request.opened`.
2. The hub creates an approval, posts an approval card in the task thread, moves the task to `needs_input`, and sends a push notification.
3. The user answers on any client. The hub sends `session.respond` to the worker, and the harness continues.

A decision works the same way from the user's side, but the assistant starts it with `ask_decision`. A decision does not block the turn. The turn ends, and the answer arrives as input to the next turn. So a session never holds a tool call open for hours.

### A routine runs

The hub's scheduler checks `next_run_at` once a second. For a `coordinator_turn` routine, it adds a `routine` wake event to the channel. For a `start_task` routine, it creates a task. The morning brief is a routine in the Everything channel.

### A machine goes offline

A worker that loses the hub keeps its sessions running and spools their events on disk. When it reconnects, it sends what the hub has not stored. A command for an offline worker waits in a queue on the hub. That includes a message that the user sends in the thread of a task whose worker is offline: the message waits until the worker comes back. The hub does not move the task to another worker, because the session's native state lives on the first worker. The task card shows the worker as offline, so the user can see why nothing moves.

If the home network is down, the phone still reaches the hub. Chat, memory, routines and the coordinators keep working on the hub's own worker. Tasks on home workers wait for those workers to come back.

## The assistant

### Coordinators and Everything

Each channel has one coordinator session. The coordinator is the assistant in that channel. Its system prompt holds the channel's purpose, its instructions, the index of global memory and channel memory, and the rules for the hub tools.

The Everything coordinator has three more tools: `list_channels`, `read_channel` and `message_channel`. With those, it can answer questions about any channel, and it can steer a channel by posting a message in that channel. It writes only global memory. Channel coordinators read global memory and write only their own channel memory. That split comes straight from Duet: the main chat sees everything but owns only the facts about the user.

Memory changes move up one level at a time. A task that learns something, such as "never push to main", calls `suggest_memory`. Its channel coordinator gets the suggestion in its next turn and decides whether to write a channel memory note. A channel coordinator that learns a fact about the user suggests it to the Everything coordinator in the same way. So the assistant keeps one memory with one owner for each scope, and every note has a coordinator that chose to keep it.

### Why the coordinator speaks through tools

A coordinator's assistant text is not posted on its own. The coordinator posts with `post_message` and `reply`. Three behaviors depend on that:

- It can answer in several short messages, as Duet does.
- It can reply inside a thread instead of the timeline.
- It can decide to say nothing when a task update needs no comment.

Some models forget to call tools. If a turn ends with assistant text and no posting call, the hub posts the last assistant message.

### How a coordinator keeps its context

The hub's timeline and memory are the source of truth. A coordinator's harness session is a cache of them. The hub keeps a coordinator session as long as it is healthy. It starts a new one when the worker reports the session as `lost`, or when the harness reports that the context is more than 70 percent full. The new session gets a context pack:

- the channel's purpose and instructions,
- the memory index (the notes stay readable through `memory_read`),
- the open tasks with their status,
- the last 50 timeline messages.

Because the session is a cache, the user can change a channel's harness or model at any time. The next coordinator session starts on the new choice with the same context pack.

## Placement

Placement turns a task's model selection and placement request into one worker and one harness account. It follows these steps in order:

1. If the request names a worker, use it. If that worker is offline, keep the task in `up_next` with the reason. If the worker cannot satisfy the request, fail with the reason.
2. Collect the online workers that have a signed-in harness account with the selected label and model.
3. Remove the workers that fail the `require` constraints (OS, labels, local MCP servers) or whose policy forbids the requested approval mode or the needed folders.
4. Prefer workers that already have a checkout of the task's repos.
5. Prefer the channel's `preferred_workers`, in order.
6. Prefer the worker with the fewest active sessions.

The coordinator can see the same inventory through `list_workers`, so it can reason about placement before it starts a task. "Test this on Windows" becomes `require: { os: "windows" }`. "Use the Home Assistant server" becomes `require: { local_mcp_servers: ["home-assistant"] }`.

Coordinators use the same steps, with the hub's own worker first in the preferred list. If the coordinator's harness account is signed in only on a laptop, the coordinator runs there, and that channel goes quiet when the laptop sleeps. The fix is to sign in to that harness on the hub machine too.

## Workspaces and local changes

A task names repos and an isolation:

- **In place** runs the session in the user's own checkout. The task sees and changes the working tree as it is, including uncommitted work. Use it for "work on what I have open right now".
- **Worktree** makes a git worktree from the checkout's `HEAD`. With `carry_uncommitted`, the worker copies the uncommitted changes into it. The task gets the user's local commits without touching the user's working tree. This is the default.
- **Clone** makes a fresh clone in the worker's `workspaces/` folder. Use it on a worker that has no checkout, such as the VPS.

The worker uses the `git` command, not a git library. The task then gets the user's own credentials, hooks and configuration on that machine. The hub never handles git credentials.

## Clients

### Shared client core

Everything in a client except drawing lives in the `client-core` crate:

- the connection, authentication and reconnection,
- the local SQLite cache and the feed position,
- sending with idempotency keys,
- the reducers that turn feed entries into view state: the sidebar, the timeline, the Tasks panel groups (Ready for review, Needs input, per channel), and the unread counts.

The desktop app and the mobile apps render that view state and send user actions back. The same split is what lets Element X share matrix-rust-sdk between its iOS and Android apps.

`client-core` runs its own Tokio runtime on background threads. Its public API uses only types that UniFFI can export: records, enums, async functions and callback interfaces. That rule keeps the mobile choice open: UniFFI makes Swift and Kotlin bindings, and `uniffi-bindgen-react-native` makes bindings for Expo.

### Desktop

The desktop app uses GPUI with GPUI Kit (`gpui-kit` 0.8.1, Apache-2.0). GPUI Kit already has most of the parts this UI needs:

| UI part | GPUI Kit component |
|---|---|
| Sidebar with groups and channels | `Sidebar` |
| Timeline that follows new messages | `MessageScroller`, `Message`, `Bubble` |
| Markdown and code blocks | `TextView` with syntax highlighting |
| Composer | `Textarea`, `Attachment`, `Speech` |
| Tasks panel and task thread panel | `Dock`, `Sheet`, `Tabs` |
| Channel and task menus | `Menu`, `Popover` |

Decision cards, approval cards and task cards are our own components.

GPUI has its own executors and does not run on Tokio. The desktop app gets updates from `client-core` through a channel and updates GPUI entities on the foreground executor. GPUI Kit depends on `gpui-pre` 0.3.10 with an exact version, because the `gpui` crate on crates.io stopped at 0.2.2 in October 2025. We pin both versions exactly.

### Mobile

The mobile app is Expo or native Swift and Kotlin. We choose later, after a spike. Either way, the app renders `client-core` view state through generated bindings. Two platform limits shape the design:

- The live WebSocket runs only while the app is in the foreground. iOS throttles silent pushes and drops them after the user force-quits the app.
- The hub sends small push payloads with entity IDs. When the user opens the app, `client-core` catches up from its feed position.

## Code layout

One Cargo workspace:

| Crate | Contents |
|---|---|
| `proto` | Wire types for the client API, the worker link and the hub tools. No I/O |
| `harness` | The `Harness` and `HarnessSession` traits and the adapters, one feature per adapter |
| `worker` | The worker daemon: hub link, spool, workspaces, local MCP endpoint, inventory |
| `hub` | The hub server: HTTP and WebSocket (axum), SQLite store, feed, scheduler, coordinator queues, placement, hub tools, push |
| `client-core` | The shared client logic |
| `client-ffi` | UniFFI bindings over `client-core`, for mobile |
| `desktop` | The GPUI app |
| `testkit` | A fake harness, a fake worker and contract tests |
| `app` | The command-line program with `hub`, `worker`, `pair` and `mcp-bridge` subcommands |

The binary name `app` is a placeholder until the product has a name.

## Deployment

**Hub.** One binary and one data folder (`hub.db` and `blobs/`). It serves HTTPS itself with automatic certificates, or it sits behind a reverse proxy such as Caddy. It runs as a systemd service. To run a worker on the same machine, start a second service with `app worker` that points at `https://localhost`. Back up `hub.db` with SQLite's online backup, or stream it with Litestream.

**Worker.** One binary for each OS. The worker must run as the logged-in user, not as a system account, because harness logins live in the user's profile:

| OS | Service |
|---|---|
| macOS | launchd user agent |
| Linux | systemd user service, with lingering on for headless machines |
| Windows | Task Scheduler task at logon. On Windows, the worker kills a session's process tree with a job object |

## Security model

The hub can tell any worker to run a harness, and a harness can run commands. So the hub is the most valuable target in the system, and the design limits what a stolen hub can do:

- **Model credentials never reach the hub.** They stay in each harness's own files on each worker. The hub has nothing to leak for Anthropic, OpenAI or Fireworks.
- **Workers set the limits.** Worker policy caps the approval mode, the folders and shell access. The hub cannot raise a worker's limits.
- **Keys, not passwords.** Workers and clients prove their identity with Ed25519 keys made during pairing. Pairing codes are single use and expire after 10 minutes. The user can revoke any worker or client.
- **Local tokens stay local.** The bearer token for a session's MCP endpoint exists only on that worker, and the endpoint listens only on 127.0.0.1.
- **Push stays small.** Push payloads hold IDs and titles, not message text, unless the user turns on previews.

Anthropic's terms allow a user to sign in to the unmodified Claude Code binary with their own plan. Workers do exactly that, on the user's own machines. Before a public release, ask Anthropic to confirm that a hub that starts those sessions remotely is acceptable.

## Open questions to test first

These items can change the design, and the research could not settle them:

1. Can `claude-agent-acp` use the user's installed `claude` and its login, and how does it pass `AskUserQuestion` and steering to an ACP client written in Rust?
2. Does Codex accept an HTTP MCP server with a bearer token in the `config` of `thread/start`? Answered on 2026-10-29: yes, as `config.mcp_servers.hub` with the token in `http_headers`, which the worker sends through the stdio pipe and not in an environment variable (see harness-adapters.md).
3. Which folder and keychain entry hold a Claude Code login, and does `CLAUDE_CONFIG_DIR` isolate two accounts?
4. Do a system tray and global hotkeys work with GPUI's event loop on Linux?
5. If the mobile app uses Expo: does `uniffi-bindgen-react-native` work with an Expo dev build on the current Expo SDK?

## Build order

1. Spikes for the open questions.
2. A hub, one worker on the hub machine, and the desktop app with the Everything channel only. The coordinator runs Claude Code through ACP. Messages, memory and tasks run on that one worker.
3. Channels, repos, workers at home, placement, approvals and decisions.
4. Routines, the Codex native adapter, Pi, OpenCode for Fireworks, worker inventory in the UI and local MCP servers.
5. `client-ffi`, the mobile app and push notifications.
