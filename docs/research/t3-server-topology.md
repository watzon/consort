# T3 Code: server, multi-machine and client architecture

Source: shallow clone at `/tmp/t3-re/src`, commit `dac8b11d` (2026-10-28). Read-only study. Nothing was built or run.
Paths below are relative to that clone. "FACT" means read in code or docs. "INFERENCE" means my reading, not stated in the source.

Scope note: another agent covers the provider adapters. This report covers everything around them.

## 0. Summary

- FACT: T3 Code has no hub. Every `t3` server is an "environment". It owns its projects, threads, harness processes, files, secrets and SQLite database. The glossary says: "**environment** means one running T3 server and the machine, filesystem, provider credentials, and state it owns." (`AGENTS.md`)
- FACT: Clients (web, desktop, mobile) are multi-environment. One client connects to many servers at once. The client, not a server, picks where a new thread runs.
- FACT: Server-to-server links exist ("peer links"). They are a beta. One server signs in to the other server's `/mcp` endpoint as an outside MCP client. Agents can then call T3 tools, including `delegate_task`, on the other machine.
- FACT: There is no NAT traversal in the server itself. Reach comes from LAN, Tailscale, SSH port forward, or "T3 Connect". T3 Connect is a hosted relay run by T3 (Cloudflare Workers, Clerk login, Cloudflare Tunnels).
- FACT: Nothing in the repo models channels, memory notes, a per-channel task list, a files folder, a purpose line, or an always-on coordinator. Scheduled tasks and delegated tasks are the two close primitives.
- INFERENCE: The Duet topology (hub off LAN, workers behind NAT that dial out) is the reverse of T3's connection direction. T3 expects clients and peers to dial in to the environment.

## 1. Server process, CLI, persistence

### What `t3` runs

- FACT: `apps/server/package.json` has `"name": "t3"`, `"bin": { "t3": "./dist/bin.mjs" }`, version `0.1.3`.
- FACT: `apps/server/src/bin.ts` dispatches hot-path subcommands before it loads the full CLI (`acp-mcp-bridge`, `acp-mcp-call`, `__managed-chatgpt-token`, `__document-text`). All else goes to `binCli.ts`.
- FACT: Subcommands (`apps/server/src/cli/*.ts`): `start`, `serve`, `app`, `auth pairing|session`, `browser setup`, `connect login|link|status|publish|unlink|logout`, `environment link|list|unlink`, `pair`, `service install|update|restart|uninstall|status`, `update`, `uninstall`, `doctor`, `project`, `thread`, `trace`, `triage`.
- FACT: Headless start is `t3 serve`: `Command.withDescription("Run the T3 Code server without opening a browser and print headless pairing details.")` (`cli/server.ts`).
- FACT: Flags (`cli/config.ts`): `--mode`, `--port`, `--host`, `--base-dir`, `--dev-url`, `--no-browser`, `--bootstrap-fd`, `--auto-bootstrap-project-from-cwd`, `--log-websocket-events`, `--tailscale-serve`, `--tailscale-serve-port`.
- FACT: Env vars: `T3CODE_PORT`, `T3CODE_HOST`, `T3CODE_HOME`, `T3CODE_MODE`, `T3CODE_NO_BROWSER`, `T3CODE_TAILSCALE_SERVE`, `T3CODE_TAILSCALE_SERVE_PORT`, OTLP and trace variables.
- FACT: Default remote port is 3773 (`packages/ssh/src/tunnel.ts` `DEFAULT_REMOTE_PORT = 3773`; `docs/user/background-service.md` example `T3CODE_PORT=3773`).
- FACT: One HTTP server on one port carries everything: WebSocket RPC at `GET /ws`, HTTP API (`/api/auth/*`, `/api/orchestration/*`, `/api/projects`), OAuth (`/oauth/*`), MCP at `/mcp`, scheduled-task webhooks at `/api/hooks`, descriptor at `/.well-known/t3/environment` (`packages/contracts/src/environmentHttp.ts`, `apps/server/src/ws.ts:3258`).

### Is it meant to run on a remote Linux box?

- FACT: Yes. `docs/user/background-service.md`: `t3 service install` makes a systemd user service on Linux. Setup enables lingering so it "starts at boot and keeps running after logout". macOS uses launchd (stops at logout). "Windows background services are not supported."
- FACT: Releases ship a self-contained Node single-executable archive per platform: macOS arm64, Linux x64/arm64, Windows x64/arm64 (`docs/operations/release.md`). The `t3` npm package is only a launcher that downloads it.
- FACT: A headless service with no host set is deliberately not public. `docs/internals/remote.md`: it "listens on every IPv4 interface and closes each connection unless both addresses of the direct peer are loopback, private, or tailnet IPv4. That keeps a VPS's public address closed".
- FACT: `T3CODE_HOST` in `service.env` replaces that default.
- FACT: The issuer and resource URLs for OAuth come from the request `Host` and `X-Forwarded-Proto`. A reverse proxy must pass both (`docs/internals/environment-auth.md`).
- INFERENCE: A public VPS hub needs an explicit host setting plus a TLS reverse proxy. Or it stays private behind Tailscale. The code supports this, but the docs steer away from it.
- FACT: SSH launch (`packages/ssh/src/tunnel.ts`) can install and start a remote server from a desktop client. It downloads the CLI archive, scans for a free port from 3773, keeps state in `~/.t3/ssh-launch/<key>`, and mints a pairing token with `t3 auth pairing create`.

### Persistence

- FACT: SQLite in WAL mode through `node:sqlite` (`persistence/Sqlite.ts`, `@t3tools/shared/nodeSqliteClient`). File: `<T3 home>/userdata/statev2.sqlite` (`config.ts:148`). 72 numbered migrations plus a V2 migration set (`persistence/Migrations/`).
- FACT: Orchestration is event-sourced. Tables include `orchestration_v2_events`, `orchestration_v2_command_receipts`, `orchestration_v2_effect_outbox`, `orchestration_v2_thread_launch_workflows`.
- FACT: Projection tables (read models):
  - `projection_projects` (projects)
  - `orchestration_v2_projection_threads`
  - `orchestration_v2_projection_messages`
  - `orchestration_v2_projection_runs`
  - `orchestration_v2_projection_nodes`
  - `orchestration_v2_projection_turn_items`
  - `orchestration_v2_projection_subagents` (delegated tasks)
  - `..._provider_sessions`, `..._provider_threads`, `..._provider_turns`
  - `..._checkpoints`, `..._context_handoffs`, `..._context_transfers`
  - `..._runtime_requests`, `..._plans`
- FACT: Other tables:
  - `auth_sessions`, `auth_pairing_links`
  - `peer_environment_links`
  - `scheduled_tasks` with `scheduled_task_webhook_deliveries`, `..._pending`, `..._relay_deliveries`, `..._replay_keys`
  - `pull_request_review_guides`, `pull_request_files_viewed`
  - `mcp_app_model_context`
  - `provider_session_runtime`
- FACT: There is no "devices" table for user machines. See section 3: "device" means simulators.
- FACT: There is no "environments" table of known machines on the server. Saved environments live in the client profile. Peer links are the only server-side list of other machines.
- FACT: The environment ID is a file written atomically at startup (`environment/ServerEnvironment.ts`). It stays stable across restarts and route changes.
- FACT: Attachments and the secret store are files under T3 home, not in SQLite.

## 2. Client protocol

- FACT: Transport is WebSocket RPC on `/ws`, built on Effect RPC. `packages/contracts/src/rpc.ts` (2130 lines) has 203 `Rpc.make(...)` definitions. Method names live in `WS_METHODS` and `ORCHESTRATION_V2_WS_METHODS`.
- FACT: Orchestration methods: `orchestration.dispatchCommand`, `subscribeShell`, `subscribeThread`, `launchThread`, `getThreadSnapshot`-style reads, `searchThreads`, `getThreadFamily`, `getOlderSettledThreads`.
- FACT: Other groups: `projects.*`, `terminal.*`, `vcs.*`, `git.*`, `pullRequests.*`, `peerLinks.*`, `scheduledTasks.*`, `secrets.answerRequest`, `guestShare.create`, `device.*`, `preview.*`, `provider.auth.*`, `provider.install.*`, `server.*`.
- FACT: Version gate: `export const ORCHESTRATION_PROTOCOL_VERSION = 2;`. `/ws` returns HTTP 426 `orchestration_protocol_incompatible` to other versions. Servers also advertise capability flags in the environment descriptor (`ExecutionEnvironmentCapabilities`, `contracts/src/environment.ts`).
- FACT: The contracts are `@t3tools/contracts`, marked `"private": true`. So is `client-runtime`, `shared`, `provider-core`.
- FACT: HTTP is used for auth, snapshots and uploads. Examples: `/api/orchestration/shell`, `/api/orchestration/threads/:threadId/bounded`, `/history`, `/api/auth/websocket-ticket`.

### Auth

- FACT: Four credential forms share one scoped session model: browser cookie, bearer token, DPoP-bound token, and short-lived WebSocket tickets. 17 scopes (`contracts/src/auth.ts`), for example `orchestration:read|operate`, `terminal:operate`, `filesystem:read|write`, `access:write`.
- FACT: Each RPC declares a required scope. The `RpcScopeAuthorization` middleware checks it before the handler runs (`auth/RpcAuthorization.ts`).
- FACT: A socket-level login does not authorize every method: "Authentication of a socket does not authorize every method on it." (`docs/internals/overview.md`)
- FACT: Default pairing grants wide scope. "`terminal:operate` is a shell as the server's user and `orchestration:operate` can start a full-access agent" (`docs/internals/environment-auth.md`).

### Pairing a phone

- FACT: `t3 pair` (or `t3 auth pairing create`) mints a one-time credential. It prints a QR code, a pairing URL, and a `t3code://pair` app link (`cli/pair.ts`). The mobile app scans or pastes it (`apps/mobile/src/features/connection/pairing.ts`).
- FACT: The hosted web app puts the pairing secret in the URL fragment, so it never reaches the hosted origin.
- FACT: Other path: T3 Connect. The user signs in to the same cloud account on the phone. The relay asks the environment to mint a one-time bootstrap credential bound to the client's DPoP key. The client redeems it directly with the environment.

### Reconnect and replay

- FACT: Streams are "snapshot then live". `OrchestrationV2SubscribeThreadInput` and `...ShellInput` take `afterSequence`. A client that holds a cursor resumes after it; otherwise it gets a snapshot, then events (`ws.ts:771-900`).
- FACT: Reconnect policy (`packages/client-runtime/src/connection/supervisor.ts`, `docs/internals/connection-runtime.md`): jittered exponential backoff capped at five minutes. It probes on foreground and on network change. A dead socket is replaced if the host still answers over a fresh request.
- FACT: A saved environment holds an ordered list of routes (LAN, Tailscale, relay, SSH). The driver uses the first that answers. The supervisor moves back to a better route when one works.
- FACT: Servers report their LAN and tailnet addresses. Clients save them as "learned routes".
- FACT: Cached projections stay readable offline. "Reconnection does not automatically replay mutations, whose retry and idempotency rules belong to the operation." Commands carry a `commandId`. Duplicates return the stored receipt without re-running side effects.
- FACT: Client sizes (non-test lines): `client-runtime` 46.6k, `apps/web` 283k, `apps/mobile` 145k, `apps/desktop` 41k.
- FACT: The desktop renderer is not served by the backend. The `t3code://` scheme serves the bundle from disk. Desktop can run with no local server (`localEnvironmentEnabled`).

## 3. Multi-machine

### Vocabulary (what each word means in this repo)

| Word | Meaning | Where |
| --- | --- | --- |
| environment | One running server plus its machine, creds, files, state. Stable ID. | `AGENTS.md`, `environment/` |
| peer / peer link | A server-to-server link. Server A holds an MCP session on server B. | `apps/server/src/peer/`, `contracts/src/peerLink.ts` |
| relay | T3 Connect control plane, hosted by T3. A Cloudflare Worker stack in `infra/relay`. Not in the hot path. | `infra/relay/README.md` |
| cloud | Server side of T3 Connect: link state, `cloudflared` connector, CLI OAuth, self-update. | `apps/server/src/cloud/` |
| device | NOT a user machine. An iOS simulator or Android emulator the agent can drive. Hosts: local, SSH, EAS, Simlock. | `apps/server/src/device/`, `docs/internals/devices.md` |
| guest | A read-only share link for one thread. Guest can propose messages. HMAC token, needs T3 Connect. | `apps/server/src/guest/`, `docs/user/guest-links.md` |
| Sharing & access | Settings page: pairing links, connected sessions, outside MCP clients, peer links. Revoke here. | `docs/user/remote-access.md` |
| `relay/` in server | Client code that talks to the relay: agent-activity publish for push notifications, held-webhook waker. | `apps/server/src/relay/` |

Warning for naming: "device" and "environment" mean different things here than in Duet. Do not reuse them.

### How one server links to another

- FACT: `docs/internals/remote.md`: "The linking environment signs in to the other's `/mcp` as an outside MCP client, with a pairing code from it, and keeps that session's token in its own secret store."
- FACT: The peer sees one more OAuth client. It lists it in Sharing & access and can revoke it. Table `peer_environment_links` holds the rest (`peer/PeerLinks.ts`, migration `070_PeerLinks.ts`).
- FACT: Transport is HTTPS or Tailscale or LAN or loopback base URLs, tried in order (`PeerLinkCreateInput`: "Where the peer answers: an https, Tailscale, LAN or loopback base URL"). The token goes only to an address whose descriptor reports the linked environment ID.
- FACT: Access is a ceiling chosen at link time (read-only, or a runtime-mode ceiling up to full access). Every forwarded call carries `T3-Mode-Limit`. The peer only narrows with it.
- FACT: A link lasts 30 days. It is a beta behind `linkedEnvironmentsEnabled` (Settings, Labs) on both sides.
- FACT: Below full access, linked work is fenced by `linkOrigin` (`mcp/linkOrigin.ts`): it can change only threads with the same origin. It cannot change projects, settings or scheduled tasks. It skips project setup scripts.
- FACT: "The link is routing, not isolation: an agent the link starts runs as the receiving environment's user."
- FACT: Waits are split into 50 s chunks (`FORWARDED_WAIT_CHUNK_MS = 50_000`) because the edge drops idle requests near 100 s.

### NAT traversal and who hosts what

- FACT: The server has no NAT traversal of its own. Routes: direct LAN, Tailscale (`packages/tailscale`, 469 lines, wraps `tailscale status` and `tailscale serve`), SSH port forward (`packages/ssh`, 2.8k lines), or T3 Connect.
- FACT: T3 Connect (`docs/internals/t3-connect.md`): "T3 Connect uses Clerk for cloud identity. The relay manages environment links, credentials for reaching environments, and managed tunnel allocations." The host runs a pinned `cloudflared` that dials out to Cloudflare. Clients then use the tunnel hostname.
- FACT: Relay stack (`infra/relay/alchemy.run.ts`): Cloudflare Workers, Durable Objects, Queues, Planetscale with Hyperdrive, Axiom, deployed with Alchemy. About 17.7k lines. T3 hosts the production instance. The URL is stored per server in the secret `cloud-relay-url`.
- FACT: Limits: 3 machines per free account, 10 with a paid T3 Chat plan (`docs/user/remote-access.md`).
- FACT: The relay also sends push notifications and iOS Live Activities (APNs, FCM) and forwards scheduled-task webhooks at `/v1/hooks/...`. It can hold webhooks for an offline environment up to 24 hours.
- INFERENCE: Self-hosting the relay means a Cloudflare account, Clerk, Planetscale and Axiom. That is a large dependency for one user.
- INFERENCE: The relay cannot be a general hub. It brokers credentials and tunnels. It never sees thread state.

### Can one machine dispatch a thread to another machine's harness?

FACT: Yes, in three ways. None of them is a central scheduler.

1. Client choice. A client holds sessions to many environments. The composer picks the environment for a new thread. Projects from different machines group by repository identity (`canonicalKey`). Optional client-side "Auto balance" picks a machine by CPU and memory (`loadBalancingEnabled`, `settings.ts:360`; `docs/user/remote-access.md` "Balance new threads across machines"). Preferences are stored per client. "Existing threads stay where they started." Mobile has manual selection only.
2. Agent `delegate_task` with a remote target. `OrchestratorMcpTarget.environmentId`: "Linked environment to run in. Provider and model default to yours". `projectId`: "With `environmentId`: its project to run in. Default: the one with this project's repository." The child is an ordinary thread on the peer. The parent records `remoteChild` instead of `childThreadId`. A follower (`peer/RemoteDelegation.ts`) waits on the remote thread (10 minute waits, backoff up to 5 minutes) and completes the task with `delegated_task.remote.complete`. Open remote tasks are followed again after a restart.
3. Other MCP tools that take `environmentId`: `t3_thread_list|read|send|wait|interrupt`, `t3_project_*`, `schedule_task`, `create_threads`, and more. Defined by `OrchestratorMcpEnvironmentTarget` in `contracts/src/orchestratorMcp.ts`.

- FACT: Identity never routes work. "A repository identity can correlate clones across environments, but never routes work between them." IDs in a call are the target environment's IDs.
- FACT: "A project and its threads belong to one environment."

## 4. Orchestration (orchestration-v2)

### Model

- FACT: Event-sourced. `Orchestrator.ts` (13.4k lines) serializes commands and decides events with no I/O. `EventSink.ts` commits events, projections, the command receipt and outbox effects in one transaction. `EffectWorker.ts` then runs side effects (start provider turn, checkpoint) and feeds results back as commands (`docs/internals/overview.md`).
- FACT: "A command acknowledgement therefore means the intent committed, not that the provider, checkpoint, or other follow-up work finished."
- FACT: Entities: AppThread, Run (a counted turn), ExecutionNode tree (tools, approvals, subagents), ProviderThread, RunAttempt, Checkpoint, ContextTransfer, ContextHandoff (`docs/orchestration-v2/README.md`). `orchestration-v2/` has 555 files, 88.8k non-test lines (includes `Adapters/`).
- FACT: Provider ids are refs. App ids are primary. A provider switch mid-thread creates an explicit context handoff artifact.

### `delegate_task` and child threads

- FACT: Tool text: "Start one child agent with only `task`. No parent history. Separate checkout or PR per child: `isolation:'worktree'`. Keep `taskId` for `task_status` and `task_cancel`."
- FACT: Input (`contracts/src/orchestratorMcp.ts:168`): `task`, `target{providerInstanceId|driverKind, model, options, environmentId, projectId}`, `title`, `role` (implementation, research, review, design, test, general), `isolation` (`shared` | `worktree`), `mode` (`async` | `wait`), `timeoutMs`, `clientRequestId`, `runtimeMode`, `interactionMode`.
- FACT: Flow (`docs/orchestration-v2/orchestrator-mcp-server.md`): `tools/call delegate_task` -> `OrchestratorMcpService` -> `ThreadManagementService` -> command `delegated_task.request` -> child thread with lineage `subagent` + child run -> parent gets an `app_owned` subagent projection and a turn item -> a `subagent_spawn` context transfer is consumed.
- FACT: Guard: "Delegation requires an active parent run owned by the MCP credential's provider session." So only a running agent can delegate.
- FACT: Policy: child runtime mode may stay equal or narrow, never escalate. Interaction mode may go `default` to `plan`, not back. Provider instance must be enabled, installed and authenticated.
- FACT: Worktree isolation (`mcp/OrchestratorMcpService.ts:2243`): resolve the parent checkout `HEAD` commit, then set `workspaceStrategy = { type: "worktree", baseRef: <sha>, startFromOrigin: false }`. Launch preparation creates a new git worktree from the project root. It fails with a clear message when the commit is not in the project root. "Uncommitted edits stay out."
- FACT: Other workspace strategies: `root` and `existing_worktree` (`OrchestrationV2ThreadLaunchWorkspaceStrategy`). Worktree tools: `t3_worktree_handoff|status|list`.
- FACT: Status values: `queued`, `running`, `waiting`, `completed`, `failed`, `cancelled`, `interrupted`. `workState`: `working`, `waiting_for_children`, `result_available`.
- FACT: Reporting back: when the child reaches a terminal state, finalization writes a `subagent_result` context transfer and the parent task. `completionWake: "always"` (async) queues a notification message that wakes the parent. `"settled_only"` (wait mode) wakes it only if the parent settled first. Delivery is claimed per generation, so it is idempotent across restarts (`Orchestrator.ts` around 12255 and 11578).
- FACT: A child question opens a pending `user_input` request. The parent gets a notification message that says to answer through `t3_pending_request` or ask the user (`DelegatedTaskQuestionReactor.ts`).
- FACT: `task_status` reads the result and marks it delivered. `task_cancel` stops running and queued turns, nested delegates and PR watches.
- FACT: `create_threads` and `t3_thread_launch` make ordinary top-level threads, not children.

### Scheduled tasks

- FACT: Contract `contracts/src/scheduledTask.ts`. Table `scheduled_tasks`. Fields: `title`, `prompt`, `enabled`, `schedule`, `projectId`, `threadId` (nullable), `workspaceStrategy`, `modelSelection`, `runtimeMode`, `interactionMode`, `nextRunAt`, `lastRunStatus`, `runCount`.
- FACT: Schedule kinds: `interval` (minimum 60 s, `MIN_SCHEDULED_TASK_INTERVAL_MS = 60_000`), `fixed_time` (local `HH:MM`, optional weekdays), `webhook` (HMAC check, filters on body/header/query, replay and coalesce keys, prompt templates like `{{body.path}}`).
- FACT: `scheduling/Scheduler.ts` ticks every 5 seconds and runs registered sources. `ScheduledTaskService.ts` launches runs through `ThreadLaunchService`. With `threadId` null it makes a new thread. Otherwise it targets the stored thread (INFERENCE from the nullable field; I did not trace the run path).
- FACT: Webhook URLs go through the relay (`/v1/hooks/:endpointKey/:hookId/:token`). Without T3 Connect, `schedule_task` returns no `webhookUrl`.
- FACT: Missed fixed-time runs are detected (`isMissedFixedTimeRun`).
- FACT: MCP tools: `schedule_task`, `list_scheduled_tasks`, `delete_scheduled_task`, `run_scheduled_task_now`.

### The T3 MCP server

- FACT: Built with Effect's `effect/ai` `McpServer`, `Toolkit`, `Tool` (`mcp/McpHttpServer.ts`). Mounted on the same HTTP port at `path: "/mcp"`. Key name for providers: `t3-code`.
- FACT: About 65 tools across toolkits in `mcp/toolkits/`: orchestrator, thread, project, worktree, environment, attachment, preview, previewControls, pullRequests, device, html. Orchestrator toolkit: `orchestrator_capabilities`, `delegate_task`, `task_status`, `task_cancel`, `schedule_task`, `list_scheduled_tasks`, `delete_scheduled_task`, `request_secret`, `create_threads`, `t3_thread_list|read|update|send|wait|interrupt`, `t3_environment_links|link|unlink`, `t3_background_wait`, `t3_workflow_stop`.
- FACT: Per-session credentials. Before a provider session opens, `McpSessionRegistry` issues a bearer token scoped to environment, parent thread, provider instance and provider session. It expires when idle and on release. The raw token is not persisted.
- FACT: Each harness gets the server differently (`docs/orchestration-v2/orchestrator-mcp-server.md`, section "Provider Injection"). Codex gets config overrides: `-c mcp_servers.t3-code.url=http://127.0.0.1:<port>/mcp` plus `T3_MCP_BEARER_TOKEN`. Claude SDK, Cursor SDK, Grok ACP, ACP registry agents and Pi have their own paths. ACP agents use a stdio bridge: `t3 acp-mcp-bridge`.
- FACT: Every tool must declare who may call it (`mcp/McpToolAccess.ts`). Read-only OAuth grants pass only tools declared as reads. Outside agents (Claude Code, Codex, ChatGPT) sign in through a small OAuth server (`auth/McpOAuth.ts`). The grant is read-only or a runtime-mode ceiling.

## 5. Secrets

- FACT: `request_secret` tool text: "Ask the user to enter a secret in a private card and wait. You never see the value. Returns a one-use `secretRef` for a tool that accepts it, or with `delivery: "file"` a private file path".
- FACT: The user answers in the UI with RPC `secrets.answerRequest` (`save` or `decline`). The server stores the value as `secret-request-<hex>.bin`. Orchestration state keeps only the status (`secret_request.record`). Header comment in `secrets/SecretRequests.ts`: "Answers stay in the secret store behind one-use refs; orchestration records only request status."
- FACT: A ref is `secret-ref:` plus a 32-hex HMAC of `threadId` and turn item ID with a per-server random salt. TTL is 24 hours. The stored record is bound to the project ID. `consume` is single use.
- FACT: Consumers: tools that accept `secretRef`, for example the webhook signature secret in `schedule_task` ("From `request_secret`. Instead of `secret`. Spent by this save."). With `delivery: "file"`, the value moves to an owner-only file for the agent's own shell.
- FACT: Storage is `ServerSecretStore` (`auth/ServerSecretStore.ts`): files in the secrets directory, directory mode `0o700`, files `0o600`, created with exclusive open. Peer-link tokens and relay credentials use the same store (`cloud-relay-url`, `cloud-relay-environment-credential`).
- INFERENCE: Secrets are local to the environment that asked. A hub does not see a worker's secrets unless a tool returns them. That fits a worker model.
- UNVERIFIED: `@napi-rs/keyring` is a server dependency. I did not trace where it is used.

## 6. Fit check for hub, worker, client

### Mapping

| Duet role | T3 piece today | Fit |
| --- | --- | --- |
| Client (web, mobile, desktop) | `apps/web`, `apps/mobile`, `apps/desktop` on `packages/client-runtime`. Multi-environment, route fallback, push via relay. | Strong. Tied to T3's RPC and to coding UX. |
| Worker (runs harnesses, repos, shell) | Any `t3 serve` / `t3 service`. Harness sessions, terminals (`terminal.*`), file RPC, git, worktrees, device control, MCP. | Strong. Windows has no background service (WSL archive exists). |
| Hub | No equivalent. Nearest: one `t3 serve` on a VPS with peer links to workers. The hub is then also a worker. | Weak. Peer links are agent tool forwarding, not a registry or scheduler. |
| Hub also a worker | Natural in T3: every server is both. | Good. |
| Hub reachable from phone with home LAN down | Phone connects straight to the VPS server over HTTPS. | Good, if the VPS exposes a public TLS endpoint. Needs host override and proxy. |
| Worker dials out to hub | Not supported. Only T3 Connect (T3's own cloudflared tunnel) or Tailscale. | Missing. |

### What is missing for a Duet layer

All items are FACT unless marked. I grepped the contracts for a memory concept and found none.

1. Channels. T3 has projects (one directory each, `t3.json` with scripts and settings) and a "Scratch" project for threads without a project. No purpose line, attached repos, task list, notes, or files folder.
2. Memory. No memory notes, no shared memory across threads. Context moves only by fork, handoff or `subagent_result` transfer.
3. One bot with one identity across channels. T3 has threads, each with its own provider session.
4. Always-on coordinator. Delegation needs a live parent run. Scheduled tasks start runs on a timer or webhook, which is the nearest thing. There is no persistent supervisor loop or inbox.
5. Routines. Partial: `scheduled_tasks` (interval, fixed time, webhook) with a project, workspace strategy and model. No per-channel grouping. Fixed times use the server's local time.
6. Tasks as sub-agent sessions that report back. Good fit: `delegate_task`, child thread, `task_status`, wake on completion, worktree isolation, remote tasks.
7. A timeline that merges task updates. Parent threads get completion notification messages. The shell stream lists threads per environment. No cross-environment merged timeline. No stable cross-environment IDs (calls say "Ids are then that environment's").
8. Hub-level registry. No list of workers with capabilities, health, load or labels on the server. The client-side load balancer is per client and not a policy engine.
9. Multi-hop trust. A hub that holds peer links to all workers holds full-access credentials. Links last 30 days and need renewal with a new pairing code.
10. INFERENCE: Local LAN MCP servers on a worker are not first-class. A harness gets `t3-code` MCP injected. Adding other MCP servers depends on each provider's own config. I did not verify per-thread extra MCP servers.

### Hard design constraints in T3 that affect us

- FACT: "A remote client must never substitute its own filesystem, provider credentials, or machine state for the environment's." (`docs/internals/overview.md`). Work stays where the files are.
- FACT: Settings are per environment with project overrides. There is no global setting.
- FACT: Remote servers "can outlive several client releases. Clients must use advertised capabilities and handle their absence". Protocol drift is a stated, managed risk.

## 7. Codebase facts

- FACT: TypeScript monorepo. `packageManager: pnpm@12.14.0`. `engines.node: 26.12.1` (exact; needed for Node single-executable builds). Server runtime is Node. Bun appears only in some replay-fixture scripts (`record:codex-replay`, `record:claude-replay`).
- FACT: Effect `4.0.5` (`pnpm-workspace.yaml`). Uses `effect/Schema`, `effect/ai` (MCP), `effect/cli`, `effect/http-api`, `effect/sql`, Effect RPC, `@effect/atom-react`.
- FACT: Build and tooling: `vp` is `vite-plus` 1.4.0 (`vp run`, `vp pack`, `vp test run`, `vp lint`, `vp fmt`). TypeScript is `@tsc-rs/typescript` 0.8.0, a Rust drop-in for `tsc`. Lint rules are custom, for example `t3code/no-test-timeout-or-unreasoned-skip`.
- FACT: Clients: React 19.3 with TanStack Router (web), Electron 44.9.3 (desktop), Expo 58 with React Native 0.88 (mobile). A Rust `native/resource-monitor` exists.
- FACT: Size, non-test `.ts/.tsx` lines (approximate, my count): server 236k; web 283k; mobile 145k; desktop 41k; relay infra 17.7k; client-runtime 46.6k; contracts 25.7k; shared 25.2k; ssh 2.8k; tailscale 0.5k. 5,487 test files. Whole repo about 3.4M lines of TS including tests and generated files.
- FACT: 25 workspace packages under `packages/`: provider (`provider-core`, `-acp`, `-acp-registry`, `-cursor`, `-fx`, `-grok`, `-muse`, `-opencode`, `-pi`, `-testing`), `effect-acp`, `effect-codex-app-server`, `source-control-*` (github, gitlab, bitbucket, azure-devops, forgejo, gitcafe, core, definitions, testing), `contracts`, `client-runtime`, `shared`, `ssh`, `tailscale`.
- FACT: Harness code is split. Provider packages hold protocol clients. `apps/server/src/orchestration-v2/Adapters/` and `ProviderSessionManager.ts` (2.4k lines) hold the adapter layer. The server also depends on `@anthropic-ai/claude-agent-sdk`, `@cursor/sdk`, `@muse-code/sdk` (their licenses are not checked).
- FACT: Library packages `contracts`, `client-runtime`, `shared`, `provider-core` are `"private": true`. Nothing is published for reuse. Only the `t3` CLI is on npm.
- FACT: License: MIT, "Copyright (c) 2026 T3 Tools Inc." (`LICENSE`).
- FACT: Release cadence (`docs/operations/release.md`): a scheduled nightly check every 30 minutes; manual stable promotion of the latest nightly; channels stable, nightly, preview. npm tags `latest`, `nightly`, `preview`. Version string is `0.1.3` and the product label is Beta.
- LIMIT: The clone is shallow with one commit (`dac8b11d`, Theo Browne, 2026-10-28). `git shortlog` shows one author. Contributor count, release dates and churn rate cannot be read from git. `AGENTS.md` claims "over 500,000 users" and says "A large number of our users run forks."
- FACT: `AGENTS.md` rules that matter for a fork:
  - Three product invariants: open core, performance without compromise, "Remote ready", and web + desktop + mobile all supported.
  - "The three ways to hurt yourself": never kill by pattern, never write to `~/.t3/userdata`, use a seeded copy for test data.
  - Test rules: event-sourced code is tested by draining the effect worker. No sleeps, no polling, no `Date.now()` deltas. Lint enforces this.
  - Verification: `vp run check:changed`, not repo-wide checks.
  - Tone: "I like ambitious ideas, simple systems... Fight scope creep." (Theo's note).
  - A CI workflow `agent-context-budget.yml` exists. It likely caps `AGENTS.md` size (INFERENCE from file name).
- FACT: `CONTRIBUTING.md`: "We are not actively seeking outside contributions and have limited review capacity." Features need prior maintainer approval. INFERENCE: a Duet layer will not merge upstream. A fork will diverge for good.
- FACT: Persisted events must stay decodable on replay. Schema changes affect old databases at startup (`docs/internals/overview.md`). A fork that changes event schemas must carry its own migrations.

## 8. Assessment of options

### (a) Fork T3 Code and add the Duet layer

- Effort: Getting a hub running is small: deploy `t3 serve` on the VPS and link workers. The Duet layer is large. You add channels, memory, routines grouping, a coordinator and a merged timeline inside an event-sourced system with a 13.4k-line `Orchestrator.ts` and 72+ migrations. Expect months, plus a permanent rebase cost.
- Gain: Every harness. Full orchestration (child threads, worktrees, checkpoints, approvals, queues). Scheduled tasks and webhooks. Secrets flow. Three working client apps with route fallback and reconnect. SSH launch. Tailscale. Push notifications (but only through T3's relay).
- What fights us:
  - Environment-local state everywhere. Projects, threads and IDs belong to one server. A hub "channel" that spans machines is against the grain.
  - Upstream moves nightly. Nothing will merge back (CONTRIBUTING). A long-lived fork must either freeze or re-port.
  - Bleeding-edge stack: Node 26.12.1, Effect 4.0.5, vite-plus, tsc-rs, Electron 44, RN 0.88. A small team inherits a very large surface (about 750k lines of app code).
  - AGENTS.md invariants (web, desktop, mobile all supported) and a coding-first UX.
  - Connection direction (worker must be dialed). The relay is Clerk + Cloudflare + Planetscale run by T3. You would replace it or avoid it.
  - Windows workers: no background service.

### (b) New hub that drives T3 servers as workers through T3's own protocol

Two ways to drive a worker:
- MCP/peer-link style: the hub is an MCP client of the worker's `/mcp` (about 65 tools). Simple, narrow, and documented. But `delegate_task` needs a live parent run, so the hub would use `t3_thread_launch`/`create_threads`, `t3_thread_send`, `t3_thread_read`, `t3_thread_wait` instead.
- WebSocket RPC style: the hub acts like the mobile app. It sends `orchestration.dispatchCommand` / `launchThread` and mirrors `subscribeShell` / `subscribeThread` with `afterSequence` into its own store. Richer, but the wire format is internal (compact IDs, deduped bodies, ID tables, protocol version 2) and the contracts package is private. You would vendor it.

- Effort: Medium. A hub service plus a worker connector, perhaps a few thousand lines. Duet data (channels, memory, task list, routines) lives in the hub's own store.
- Gain: All harnesses with no adapter work. Worker install is `t3 service install` (Linux, macOS) or desktop (Windows). Hub is also a worker by running `t3 serve` beside it. T3's clients still work for debugging. You avoid owning the orchestrator.
- What fights us:
  - Reach direction. The hub must dial each worker. Workers behind home NAT need Tailscale on both ends, or T3 Connect (T3's cloud, limit 3 machines free), or your own reverse tunnel (INFERENCE: not provided by T3). The server cannot register itself with a hub.
  - Protocol stability. The RPC is versioned but internal and moves nightly. Capability flags help, but you chase it.
  - Credentials. The hub holds a full-scope session per worker. Blast radius is every machine.
  - Dual control. A user can open the worker's own T3 UI and change the same threads.
  - Hub-side routines need your own scheduler, since T3's scheduled tasks live on one worker.
  - Local LAN MCP servers: you can only reach what T3's tools expose (terminal, file RPC, git, MCP injection). UNVERIFIED.
  - Task status mapping. T3 tasks are thread-based. You map each Duet task to one T3 thread and watch its run state.

### (c) Build fresh and reuse some T3 packages

- Effort: High. Harness parity is the bulk of T3's code. The adapters are Effect-based and entangled with `orchestration-v2` (adapters live in `apps/server`, not only in `packages/`).
- Gain: A clean hub, worker and outbound-connect design. Duet's data model is first-class. No protocol drift. Pick your own stack.
- Reusable candidates (INFERENCE): `packages/ssh` (2.8k lines, remote launch and port forward), `packages/tailscale` (0.5k), `effect-acp`, `effect-codex-app-server`, `provider-*` protocol clients, parts of `packages/shared`, and the connection supervisor pattern in `client-runtime`. They are private workspace packages, so reuse means copying, and all of them assume Effect 4.
- What fights us: Each harness needs live tracking of its SDK. Replay fixtures and approvals take much work. You lose T3's clients. You own mobile push.

### Recommendation (INFERENCE)

1. Start with (b) as a prototype to prove the Duet layer. Use the MCP path first. It is narrow and documented. Add the RPC path only where you need live event mirroring.
2. Put memory, channels, routines and the coordinator in the hub's own store. Treat T3 as a "harness runner" per machine.
3. Solve reach early: choose Tailscale or a reverse tunnel per worker. Do not plan around T3 Connect unless you accept T3's cloud.
4. Keep (c) as an exit. Use `provider-*`, `effect-acp` and `packages/ssh` as copyable parts if T3's protocol becomes a blocker.
5. Choose (a) only if you accept a long-lived fork and want T3's clients as your clients. Even then, build the Duet layer as a new module and keep upstream edits small.
6. Before committing, test three unknowns: (i) a hub MCP client against a real worker `/mcp` including `t3_thread_launch` and `t3_thread_wait`, (ii) injecting an extra MCP server (for LAN tools) into a T3-run harness session, (iii) the public-VPS exposure path (`T3CODE_HOST`, TLS proxy, OAuth issuer URLs).
