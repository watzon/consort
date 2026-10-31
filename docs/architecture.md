# Architecture

This page explains how the system fits together and why. It is the starting point for the other design docs:

- [CONTEXT.md](../CONTEXT.md): the terms
- [data-model.md](data-model.md): entities and state machines
- [protocol.md](protocol.md): the client API, the worker link and the hub tools
- [harness-adapters.md](harness-adapters.md): how workers drive Claude Code, Codex, Pi, OpenCode and ACP agents
- [design-system.md](design-system.md): themes, color roles, type, spacing, corners, motion, glyphs and the logo
- [adr/](adr/): the decisions and the options we rejected

## The shape of the system

The system copies the user-facing shape of Claude Duet: one assistant, a Tutti channel, project channels, tasks that run as their own sessions, routines and memory. It changes where the work runs. Duet runs tasks in Anthropic's cloud with Anthropic models. Here, tasks run on the user's own machines, with any harness and any model.

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
2. The hub creates an approval, posts an approval card in the task thread, and moves the task to `needs_input`. The client shows a notification. The hub does not wake the coordinator, because the coordinator cannot answer an approval.
3. The user answers on any client. The hub sends `session.respond` to the worker, and the harness continues.

A decision works the same way from the user's side, but the assistant starts it with `ask_decision`. A decision does not block the turn. The turn ends, and the answer arrives as input to the next turn. So a session never holds a tool call open for hours.

### A routine runs

The hub's scheduler checks `next_run_at` once a second. For a `coordinator_turn` routine, it adds a `routine` wake event to the channel. For a `start_task` routine, it creates a task. The morning brief is a routine in the Tutti channel. Each run is recorded and stays `running` until its turn or its task ends. A scheduled run that would start while the last run still runs is `skipped`, and so is a run that the hub missed by 1 hour or more while it was down (see [protocol.md](protocol.md#routine-runs)).

### A machine goes offline

A worker that loses the hub keeps its sessions running and spools their events on disk. When it reconnects, it sends what the hub has not stored. A command for an offline worker waits in a queue on the hub. That includes a message that the user sends in the thread of a task whose worker is offline: the message waits until the worker comes back. The hub does not move the task to another worker, because the session's native state lives on the first worker. The task card shows the worker as offline, so the user can see why nothing moves.

If the home network is down, the phone still reaches the hub. Chat, memory, routines and the coordinators keep working on the hub's own worker. Tasks on home workers wait for those workers to come back.

## The assistant

### Coordinators and Tutti

Each channel has one coordinator session. The coordinator is the assistant in that channel. Its system prompt holds the channel's purpose, its instructions, the index of global memory and channel memory, and the rules for the hub tools.

A coordinator does no shell, web or file work: it starts a task for that. The prompt says so, and the hub and the worker enforce it. A coordinator session always runs in Ask, whatever the default approval mode of the channel (that mode is for the channel's tasks), and the worker denies every approval request of the session except a call to a hub tool, with a steer that tells the model to start a task. Hub tools, including connector tools, keep their own rules. This refines [ADR 0013](adr/0013-the-assistant-is-proactive-within-limits.md): the coordinator asks nothing and refuses instead. See DECISIONS 700.

The Tutti coordinator has three more tools: `list_channels`, `read_channel` and `message_channel`. With those, it can answer questions about any channel, and it can steer a channel by posting a message in that channel. It also has the management tools for channels, workers, accounts, connections and skills (see [Connectors, skills and management](#connectors-skills-and-management)). It writes only global memory. Channel coordinators read global memory and write only their own channel memory. That split comes straight from Duet: the main chat sees everything but owns only the facts about the user.

Memory changes move up one level at a time. A task that learns something, such as "never push to main", calls `suggest_memory`. Its channel coordinator gets the suggestion in its next turn and decides whether to write a channel memory note. A channel coordinator that learns a fact about the user suggests it to the Tutti coordinator in the same way. So the assistant keeps one memory with one owner for each scope, and every note has a coordinator that chose to keep it.

### Why the coordinator speaks through tools

A coordinator's assistant text is not posted on its own. The coordinator posts with `post_message` and `reply`. Three behaviors depend on that:

- It posts one message for one reply. It does not split a long reply, because a long result is a document that a task writes.
- It can reply inside a thread instead of the timeline.
- It can decide to say nothing when a task update needs no comment.

Some models forget to call tools. If a turn ends with assistant text and no posting call, the hub posts the last assistant message.

### How a coordinator keeps its context

The hub's timeline and memory are the source of truth. A coordinator's harness session is a cache of them. The hub keeps a coordinator session as long as it is healthy. It starts a new one when the worker reports the session as `lost`, or when the harness reports that the context is more than 70 percent full. The new session gets a context pack:

- the channel's purpose and instructions,
- the memory index (the notes stay readable through `memory_read`),
- the open tasks with their status,
- the last 50 timeline messages.

The open tasks are not only in the context pack. Every later turn that has a message from the user starts with an `<open_tasks>` block (ID, title and status of at most 20 open tasks), so the coordinator can tell that a message is about a task and pass it on with `message_task` and `user_message_id` (see DECISIONS 702 and 704).

Because the session is a cache, the user can change a channel's harness or model at any time. The next coordinator session starts on the new choice with the same context pack.

### The assistant on its own

The assistant starts contact and work without a new message from the user: check-ins, the morning brief, follow-ups that it set, nudges on stale tasks, the guided start of a new channel, and (at the level Eager) a read-only research pass with offers. The decision is [ADR 0013](adr/0013-the-assistant-is-proactive-within-limits.md), and the design is [PROACTIVITY-PLAN.md](PROACTIVITY-PLAN.md). The code is `consort-hub/src/proactivity/`.

The hub, not the prompt, decides when a proactive wake may run, before a model runs:

- **Where wakes come from.** Routines with a kind (`check_in`, `brief`, `follow_up`, `research`) go through the scheduler, which hands them to `proactivity::routines::begin_proactive_run`. A hub timer finds stale tasks every 30 seconds (`proactivity::stale`). `channel.create` queues the guided start, and `message.rate` with Too noisy queues feedback.
- **The gate** (`proactivity::gate`) is pure code with no I/O: the level of the channel, Pause all, snooze, quiet hours in the user's time zone, and the daily turn limit. A check-in also needs a change: the hub builds a change summary from its own state since the last check-in (`proactivity::summary`), and skips the check-in when nothing changed and the check-in note does not start with `watch: outside`. Every skip is a row in the proactivity log, except a wake that the level stops. A skipped wake queues no wake event, so no turn and no model run.
- **The routines follow the level.** A channel gets its check-in at Normal and its research pass at Eager, and both pause below. Tutti gets the morning brief once. The hub gives existing hubs these routines once, at the first start with proactivity.
- **The turn.** A turn whose events are all proactive is a proactive turn (`proactivity::turn`). Its text without a posting call is dropped. Its messages carry a `proactive` mark. A notification needs the time outside quiet hours and snooze, and room under the daily limit. Its tasks run in Ask. Tools that widen access are refused, and a research turn may only read. When the turn ends, its log entries get their outcome, a silent check-in counts toward back-off (six in a row double the interval, up to 12 hours; a user message resets it), and a follow-up deletes itself.

The client shows the settings, the log, the marks, the offers, the quick-reply chips and the writing bubble. A proactive message notifies only when the hub marked it with `notify`, and only through the switch "When the assistant needs your attention". Nothing notifies in quiet hours or while snoozed.

### Write in my voice

When the user turns on "Write in my voice", drafts that go out in the user's name follow the style guide `voice.md`. The decision is [ADR 0014](adr/0014-the-assistant-writes-in-the-users-voice-with-consent.md), the design is [VOICE-PLAN.md](VOICE-PLAN.md), and the code is `consort-hub/src/voice/`. The hub enforces the rules in code:

- **Learning.** `voice.set` (on) and `voice.learn` start the voice task in Tutti with a fixed brief that lists only the chosen sources (`voice::learn`). That task may run only read tools of those connections, and the text that the assistant wrote leaves its results. Its `suggest_memory` becomes the offer "Save this style guide?"; only Do it writes `voice.md`, which goes through the format and the screening of `consort_proto::model::VoiceGuide`.
- **Scope.** Only a session whose channel can reach a draft or send tool gets the section "The user's voice is on" in its prompt (`Hub::voice_prompt_section`). `voice_guide` gives one context.
- **The card.** A draft or send tool's card says which voice wrote it, has Edit (`approval.edit`), and marks commitments that the request did not contain (`voice::scan`). An allow needs the tick while flags exist, and a call with flags always asks. The voice never loosens an approval rule.
- **Edits.** Allowed draft cards become draft pairs. Five edited pairs of one context wake Tutti with `voice_edits`; "Not how I write" wakes it with `voice_feedback`. Tutti proposes rule changes as offers.

The client shows Settings > Voice, the offer with "Open the full guide", the voice task's card with its steps, and the draft card with its flags, the tick, Edit and "Not how I write".

### The account of a task

A task runs on the first of these that exists: its own selection, the task selection of its channel, the task selection of Tutti, and the coordinator selection that its channel uses. The first two are choices of the user: when no worker can run them, the task waits or fails. The other two are defaults: when the task needs a worker without that account, the default rule of placement decides. When a task starts, the hub saves its account and model on the task, so a later change to the channel does not move it.

### The fallback path

The user keeps one global path of models. The hub stores it in the preferences and sends it to every client. A step is an account, a model and an effort. The path has four parts, in this order:

1. **The first choice.** This is the model of the channel (or of the task). It is not stored in the path.
2. **Other accounts with the same model.** A built-in row, on by default. It moves the work to another signed-in account of the same harness that has the same model. It replaces the old `account_fallback` switch.
3. **The steps of the user.** At most 10, in order. No account and model is in the path twice.
4. **Stop and show the error.** A fixed row. It is not stored.

Two causes start a move. A turn that fails with the kind `rate_limit` marks its account as limited in `account_limits`. A turn that fails with the kind `provider_outage` (the provider is down or overloaded) starts a move only after 3 failed tries of the same session, counted in `session_outage_tries`. The first failure and two retries run on the first choice. A sign-in problem starts no move, because the user can fix it.

The walk goes through the parts in order and takes the first step that can answer. It skips a step, and records the reason, in these cases:

| Reason | When |
|---|---|
| Limited | The account is at a usage limit, or its provider is in an outage hold |
| Signed out | The account is signed out, or its sign-in expired |
| Unavailable | No account has this label, or the account does not offer the model |
| Mode | The account cannot keep the approval mode of the channel or the task. A step never lowers the mode |

A worker is never a reason to skip a step. All accounts are on the hub machine, so the hub does not look at the worker when it walks the path.

A hold lasts until the provider's reset time. When the provider gives no time, a usage limit holds for one hour (DECISIONS 753) and an outage holds for 10 minutes. An outage hold covers every account of the same harness, because the provider is the same.

- **A task** gets the new account and model as its selection. The hub ends its session and starts a new one on the same worker, in the same workspace. The task stays where it moved, and the user can change its model.
- **The assistant** gets a new session on the step. The hub saves `coordinator_fallback` on the channel (the first choice, the step, the cause and the reset time), and the events of the failed turn go back to the queue. At the reset time the maintenance tick sends the assistant back to its first choice when it is idle, and the hub posts a `fallback_returned` notice. "Go back now" (`channel.go_back`) does the same after the reset, and is refused while a turn runs. When the provider gave no reset time, the hold time is the reset time.
- **No step can answer:** the task fails, or the channel shows a `turn_failed` notice. The notice has `path`, the lines of the walk, so the client can show what Consort tried and skipped. No turn runs in a loop, because a move needs a step that is not held.

The move adds a notice to the thread or the channel (`account_fallback`). When the walk skips a signed-out account, the hub also adds `fallback_signed_out`. When the step is on another harness than the first choice, the notice names the provider, because the work and its context go to that provider. The first step on another harness needs a confirm in the client. The approval mode stays the same on every step.

### The model of the assistant

Each channel has a coordinator selection (account, model and options such as the effort). A channel without its own selection uses Tutti's, so Tutti's selection is the default for every such channel. The user chooses it in the composer, in the New channel dialog, or with Model in the channel menu. The client sends `channel.update { coordinator_selection }` at once, and the hub stores it.

When the selection that a channel uses changes, its own or Tutti's for a channel that has none, the hub ends the open coordinator session of that channel if no turn runs. A turn that runs is not cut: the next run of the channel sees that the prompt fingerprint (which holds the selection) changed, ends the session, and starts a new one. The next turn then starts with the new model and options. The hub passes the options in `session.start`. The ACP adapter sets them with `session/set_config_option` after the model. The Codex adapter sends the effort with each turn. The hub does not switch the model inside a session.

## Connectors, skills and management

Consort, not the provider of each harness, is the source of the assistant's connectors and skills (ADR 0011). So every harness on every worker gets the same set, and the hub can enforce approvals and scope.

```
 Session ──MCP──> Worker ──tool.call──> Hub ──HTTPS──> Composio ──> Gmail, GitHub, ...
                    ^                    |
                    └─ skill_bundle.get ─┘  (skills, by content hash)
```

- **Connectors.** The user brings a Composio key, which only the hub holds. Each connection is one connected account of one service, with a handle such as `gmail_work`. A session sees the connections of its channel in a "Connectors in this channel" block, and uses them only through `list_connector_tools`, `get_connector_tool` and `run_connector_tool`. Tool schemas stay out of the context until the assistant asks for the tools of one connector. The hub checks the scope, applies the approval rule of the tool kind, runs the call with the exact connected account, and writes an audit record.
- **Monid.** Monid is a catalog of paid tools with one wallet. The hub holds the key and calls its API. A session uses `search_tools`, `get_monid_tool` and `run_monid_tool`. The hub checks access, limits and approvals on each call, starts a run once, follows it, and writes one row with what ran, what came back and what it cost. See ADR 0016.
- **Access requests.** A session sees the names of the connections and of Monid that its channel lacks, and asks with `request_access`. The user answers on a card with temporary access, lasting access or no. See ADR 0017.
- **Skills.** The hub installs a skill from a git repo, a directory entry or an upload, pins it to a commit and a content hash, and stores its files as blobs. The worker gets the skills of a session by hash and writes them where its harness reads skills (a plugin folder for Claude Code, a skills folder for Codex and Gemini CLI, the `skills` paths of OpenCode). A harness with no skill support gets `list_skills` and `read_skill`.
- **Scope.** Each connection and skill has a "Use in" scope: every channel, the channels with all of some tags, or a list of channels. A channel can turn off an item that its scope gives it (a channel exception). The effective set of a session is fixed when the session starts.
- **Cards.** A connector approval, a connect card, a sign-in card, an install card and a management approval card are approvals that the hub makes itself (hub cards). They use the approval card, `approval.answer`, push and Needs input. When a card that a coordinator posted closes, the coordinator gets a `card_answered` event.
- **Management.** The Tutti coordinator manages everything, and a channel's coordinator manages its own channel. The hub enforces the risk rule: a narrow or stricter change runs at once, and a change that deletes, widens access, loosens approvals, turns on scripts or changes tags waits for the user. The assistant never handles a secret.
- **Provider extras.** What a provider syncs to an account, such as claude.ai connectors, is always off, as far as the provider lets the session host turn it off. There is no setting. Connectors and skills come from Consort only (ADR 0011).

## Placement

Placement makes two separate choices. The model selection chooses one harness account in the session host. The placement request chooses the execution worker where the tools of the task run. The second choice follows these steps in order:

1. If the request names a worker, use it. If that worker is offline, keep the task in `up_next` with the reason. If the worker cannot satisfy the request, fail with the reason.
2. Collect the online execution workers. The session host must have a signed-in harness account with the selected label and model.
3. Remove the workers that fail the `require` constraints (OS, tags, local MCP servers) or whose policy forbids the requested approval mode or the needed folders. A worker must also have every tag of the channel. Its tags are its user tags and the tags in its inventory.
4. Prefer workers that already have a checkout of the task's repos.
5. Prefer the channel's `preferred_workers`, in order.
6. Prefer the worker with the fewest active sessions.

When no online worker has the channel's tags, a task waits, and the reason names the tags, for example "Waiting for an online worker with the tag work. Add the tag in Settings > Workers." Routine runs that start a task follow the same rule.

Tags apply to tasks, not to coordinators. A change of the tags of a channel does not stop its coordinator. Tasks that run keep running. Only new tasks follow the tags.

The coordinator can see the same inventory through `list_workers`, with each worker's tags, so it can reason about placement before it starts a task. "Test this on Windows" becomes `require: { os: "windows" }`. "Use the Home Assistant server" becomes `require: { local_mcp_servers: ["home-assistant"] }`.

A coordinator runs in the session host and needs no worker, so a channel does not go quiet when a laptop sleeps.

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
- the local SQLite cache and the feed position (`cache.sqlite3`: the app opens with its data while the hub is offline, and a reconnect resumes from the saved position),
- the offline state: a status pill with the time of the saved data, workers with an unknown status, approvals and decisions that wait for the hub, and a message queue that sends when the hub is back,
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

### Notifications

The desktop app shows a system notification when an approval or a decision waits, when a task moves to Ready for review or Failed, when the assistant posts in a channel that is not on screen, and, when the user turns it on, when a routine posts a result. The rules live in `consort-client` (`notify`): each event notifies once, nothing that was there at the first load notifies, and nothing notifies while its place is on screen in the active window. The desktop app only delivers: GPUI's `show_system_notification` posts it, and a click opens the task thread or the channel. The Dock icon shows the number of approvals and decisions that wait plus the tasks in Ready for review (macOS, through `objc2-app-kit`); Windows and Linux show the number in the window title. The switches live on the client, not on the hub. Message text is off by default. A message that the assistant posted on its own notifies only with its `notify` mark, through the switch "When the assistant needs your attention", and never as a message in a channel that is not open. Nothing notifies in quiet hours or while a snooze is on. macOS shows notifications only for an app that runs from an app bundle.

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
| `link` | The dial-out link that the session host and the worker share: pairing, keys, spool, reconnect |
| `session-host` | The session host: accounts and their profile folders, sign-in flows, managed CLIs, sessions in a boundary, the MCP endpoint and the tool surface |
| `worker` | The execution worker: journal, processes, files, workspaces, inventory |
| `tools` | Managed tools: downloads, checks, install layout and launch specs. No hub I/O |
| `hub` | The hub server: HTTP and WebSocket (axum), SQLite store, feed, scheduler, coordinator queues, placement, hub tools, push |
| `client-core` | The shared client logic |
| `client-ffi` | UniFFI bindings over `client-core`, for mobile |
| `desktop` | The GPUI app |
| `testkit` | A fake harness, a fake worker and contract tests |
| `app` | The command-line program with `hub`, `worker`, `pair` and `mcp-bridge` subcommands |

The binary name `app` is a placeholder until the product has a name.

## Deployment

**Hub.** One binary and one data folder (`hub.db` and `blobs/`). It listens on a local address and does not serve HTTPS itself: a reverse proxy such as Caddy does, with automatic certificates (decision: a documented proxy is enough, no built-in ACME). It runs as a hardened systemd service (`deploy/hub/consort-hub.service`) or as a container (`Dockerfile`, `deploy/hub/docker-compose.yml`). To run a worker on the same machine, install it as a user service with `consort worker install`. The steps are in [deploy.md](deploy.md). Settings > Hub backs the hub up: Back up now, or a daily or weekly schedule that the hub runs itself. A backup is one tar archive in a folder on the hub machine, with a consistent copy of `hub.db` from SQLite's online backup, the keys and every blob (see [data-model.md](data-model.md#backups)). The hub keeps the last N archives.

To restore, stop the hub and run `consort hub restore <backup> --data <data folder>` on the hub machine. It refuses to run while a hub uses the folder, refuses a file that is not a Consort backup (or that a newer hub made), and refuses a folder that is not empty unless you add `--force`. With `--force`, the database, the blobs, the keys and the backup settings that were in the folder move to `before-restore-<time>/` in it first, so nothing is lost. The restore checks every blob hash and runs SQLite's integrity check before it changes the folder. Then start the hub as before. The clients and workers that were paired when the backup was made connect again with their keys.

**Session host.** It runs on the hub machine, as its own user, with its own unit (`deploy/session-host/consort-session-host.service`) or its own image (`deploy/session-host/Dockerfile`). The hardened hub unit stays as it is, because bubblewrap cannot run under it. Harness logins and managed tools live in its folder. In Docker, bubblewrap needs the narrow seccomp profile in `deploy/session-host/seccomp.json`. `scripts/dev.sh` starts it as a child of `consort dev`, and on a Mac it uses `sandbox-exec`, with a warning in Settings > Hub.

**Worker.** One binary for each OS. The worker runs as the logged-in user, because tasks use that user's checkouts, git credentials and hooks. It has no account and no CLI. The service for each OS:

| OS | Service |
|---|---|
| macOS | launchd user agent (`dev.consort.worker`) |
| Linux | systemd user service (`consort-worker.service`), with lingering on for headless machines (`consort worker install --linger`) |
| Windows | Task Scheduler task at logon (`Consort worker`). On Windows, the worker kills a session's process tree with a job object |

`consort worker install` pairs the machine and installs the service for the current user, `consort worker uninstall` removes it, and `consort worker status` shows it. The install scripts and the release files are in [install.md](install.md).

## Security model

The hub can tell any worker to run a harness, and a harness can run commands. So the hub is the most valuable target in the system. The hub controls the workers, and a stolen hub compromises them. That is accepted, as a stolen laptop with SSH keys is (see the owner's decision below). The design limits what else a stolen hub gives away:

- **Model credentials stay in the session host.** Subscription logins stay in each account's profile folder in the session host, in the harness's own files or keychain entry, and Consort never reads them. An API key goes once from the app through the hub to the session host, which writes it to `env.json` in the profile folder. The hub process never stores or logs it. The code that the user pastes during a Claude sign-in goes the same way to the stdin of `claude auth login`, and nobody stores it. See ADR 0015. A key of an API endpoint for the command check is different: the hub keeps it in its database, because the hub calls the endpoint.
- **A session runs in a boundary.** Bubblewrap on Linux, `sandbox-exec` on a Mac in development, and a network proxy per session that allows only the hosts of that provider. With no boundary, no session starts. Each session has its own MCP token.
- **Connector keys stay on the hub.** The Composio key is encrypted at rest on the hub and never leaves it. Credentials for a connector with no browser step go from the app through the hub to Composio once, and Consort does not store them. The assistant never handles a key or a sign-in: a card sends the user to the app or the browser. The hub keeps a hash of each connector call's arguments, never the arguments. See ADR 0011.
- **The hub controls the workers (owner's decision, 2026-10-31).** The user changes the limits of a worker in the app: the highest approval mode, the folders and shell access. Nobody edits a file on the worker. The hub forwards the change with no signature, so a stolen hub can raise the limits. The worker saves the limits in `worker.db`, applies them at once and refuses a shell command or a folder outside them, because that is where the work runs. The approval engine on the hub applies the highest approval mode.
- **Keys, not passwords.** Workers and clients prove their identity with Ed25519 keys made during pairing. Pairing codes are single use and expire after 10 minutes. The user can revoke any worker or client.
- **Local tokens stay local.** The bearer token for a session's MCP endpoint exists only in the session host, and the endpoint listens only on 127.0.0.1.
- **Push stays small.** Push payloads hold IDs and titles, not message text, unless the user turns on previews.

Anthropic's terms allow a user to sign in to the unmodified Claude Code binary with their own plan. Workers do exactly that, on the user's own machines, and the user starts the sign-in from the app. Before a public release, ask Anthropic to confirm that a hub that starts those sessions remotely is acceptable.

## Open questions to test first

These items can change the design, and the research could not settle them:

1. Can `claude-agent-acp` use the user's installed `claude` and its login, and how does it pass `AskUserQuestion` and steering to an ACP client written in Rust?
2. Does Codex accept an HTTP MCP server with a bearer token in the `config` of `thread/start`? Answered on 2026-10-29: yes, as `config.mcp_servers.hub` with the token in `http_headers`, which the worker sends through the stdio pipe and not in an environment variable (see harness-adapters.md).
3. Which folder and keychain entry hold a Claude Code login, and does `CLAUDE_CONFIG_DIR` isolate two accounts?
4. Do a system tray and global hotkeys work with GPUI's event loop on Linux?
5. If the mobile app uses Expo: does `uniffi-bindgen-react-native` work with an Expo dev build on the current Expo SDK?

## Build order

1. Spikes for the open questions.
2. A hub, one worker on the hub machine, and the desktop app with the Tutti channel only. The coordinator runs Claude Code through ACP. Messages, memory and tasks run on that one worker.
3. Channels, repos, workers at home, placement, approvals and decisions.
4. Routines, the Codex native adapter, Pi, OpenCode for Fireworks, worker inventory in the UI and local MCP servers.
5. `client-ffi`, the mobile app and push notifications.
