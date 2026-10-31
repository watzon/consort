# Plan: the hub owns models, accounts and harnesses

Status: the user chose the direction on 2026-10-30. Not built. Codex (GPT-6 Astra) reviewed it in two rounds on 2026-10-30. The ADR, and the changes to `CLAUDE.md`, `architecture.md` and ADRs 0004 and 0010, go in with step 2, because the code follows the old rule until then.

## Goal

- The user signs in once. Harness CLIs are installed only on the hub machine.
- One settings page has providers, accounts, models and the fallback path.
- A worker is a machine where work runs. It has no accounts.
- What thinks and where the tools run are two separate choices.

## The problem to solve

A CLI's built-in tools run on the machine where the CLI runs. `claude-agent-acp` 0.99.0 does not give its terminal to the ACP client. So a CLI on the hub cannot use its own `Bash` on a worker. The hub turns off the native tools and gives the CLI Consort tools that run on the worker.

Rejected:

- **The hub as a model gateway.** Workers still need the CLIs, and Anthropic's terms forbid routing plan logins.
- **Copying tokens to workers.** The same terms forbid a developer holding or passing on Claude.ai credentials.
- **A mounted workspace and a forwarding shell.** Hooks, helpers and subprocesses can still run on the hub, and mounts bring locking and disconnect problems.

## Components

| Component | Where | What it owns |
|---|---|---|
| Hub API | Hub machine, the hardened service | Settings, content, placement, the approval engine |
| Session host | Hub machine, its own service | Profiles, sign-in, CLI install, probes, CLI processes, session journals |
| Execution worker | Each machine | The link, workspaces, policy, file and process tools, local MCP servers, an execution journal |

The session host is today's `consort-worker` daemon with the execution code taken out. It is not a new program. It dials the hub over localhost, and it keeps running when the hub restarts. It stays a separate process. `deploy/hub/consort-hub.service` sets `ProtectHome`, `RestrictNamespaces` and `MemoryDenyWriteExecute`, which CLI runtimes and an OS boundary cannot live under, so it gets its own unit and its own Docker image. Split `worker_link` into a session-host protocol and an execution protocol, with separate credentials.

## What moves

| Now on the worker | Goes to |
|---|---|
| Accounts, profiles, secrets, sign-in, CLI install and leases, account probes | Session host |
| Harness processes, event translation, native session references | Session host |
| Hub tool forwarding and the MCP endpoint | Session host, and the forwarding hop goes away |
| Workspaces, checkouts, worktrees, git | Stays |
| Worker policy | Stays, and the user changes it in the app |
| Local MCP servers and local network access | Stays |
| The spool | Splits: turn events to the session host, an operation journal on the worker |
| Machine facts and detected tags | Stays. Configured tags are on the hub |

**Skills.** The hub already owns them. It stores and pins each skill, and workers only cache bundles by content hash (`consort-worker/src/skills.rs`). So the worker holds a copy, not a separate install. In the new design, the session host presents the instructions to the CLI. A worker gets the script files only when a command needs them: `skill.prepare(hash, workspace)` checks the files and returns a real path on that worker. A skill's instructions use that path. Consort does not rewrite command strings. A skill with a fixed local path needs a change.

**Memory, attachments, MCP configuration, approval rules and item details** have one copy on the hub. A worker keeps only caches that it can rebuild.

**Git stays where it is.** A command runs as the user on the worker, with the user's keys and hooks (`architecture.md`, "Workspaces and local changes"). The review proposed short-lived credentials from the hub. This plan rejects that, because the hub never needs git credentials.

## The tool surface

- `workspace.prepare`, `inspect`, `release`
- `fs.list`, `stat`, `read`, `search`, `apply_patch`, `write`, `move`, `remove`
- `process.start`, `read`, `write_stdin`, `signal`, `wait`
- `blob.put`, `get`
- `skill.read`, `prepare`
- `local_mcp.list_tools`, `call`

Each request has a workspace capability, an operation ID, a deadline and a policy version. A change to a file carries the expected hash. Paths are relative to the capability, and the worker checks containment with the rules of its own system.

## Rules

**Fail closed on the hub.** A missed tool would run commands on the machine that holds every login.

- Every mode uses a generated configuration, Full access too. Today Full access drops the restrictions. Allow only the Consort MCP endpoint. Turn off account hooks, arbitrary plugins, other MCP servers and provider extras.
- Each CLI runs in an OS boundary: its own profile and a throwaway folder, with no access to `hub.db`, blobs or other profiles. On Linux use bubblewrap, and test Landlock too.
- The network is the part that a file boundary does not stop. A CLI must not reach the hub API or loopback services. It gets one credential, scoped to its session.

**The real environment.** A CLI on a Linux hub can drive a Windows worker. `workspace.inspect` returns the capability, the worker, the OS, the native root, path rules, shell and runtimes. Consort gives this to the model at the start, at resume, at a change of worker and on each turn. Each session keeps a stable folder on the hub, so that native resume works.

**Restarts.** A hub restart does not stop a turn, because the session host keeps running. A session host restart interrupts the turn, and the session resumes after Consort checks the open operations. A command with an unknown result never runs again by itself. Commands on a worker can go on within their deadlines when the link is lost, and the worker gives the results at reconnect.

**Placement.** A model selection is a stable account ID, a model, options and a fallback path. Execution placement is a worker, a workspace, tags and policy. Coordinators need no worker. A fallback changes what thinks and does not move files.

## Approval modes

One approval engine on the hub decides. Worker policy is a separate upper limit.

| Tool class | Ask | Auto edits | Auto | Full access |
|---|---|---|---|---|
| Reads and process output | Allow | Allow | Allow | Allow |
| Writes and patches in the workspace | Ask | Allow | Allow | Allow |
| Remove, or a release that loses data | Ask | Ask | Ask | Allow |
| Shell | Ask | Ask | The classifier decides | Allow |
| Local MCP | By its class, unknown asks | Same | Same | Allow in policy |

The review wanted Auto to allow only known command templates and to ask for all others. The user rejected this on 2026-10-30, because it asks too often. Auto uses a classifier model.

### The classifier model

**How the others do it** (read on 2026-10-30 from Claude Code 2.1.314, Codex 0.166.0 and their docs):

| | Claude Code auto mode | Codex auto-review |
|---|---|---|
| Model | A Sonnet model, not Haiku. The server can set another one | A separate reviewer agent. The binary has a `codex-auto-review` model and names `gpt-5.6-luna` beside it. Which one is the default was not proven |
| What it is | A general model with a long policy prompt | The same |
| Stages | A short answer for each action, then reasoning for each flagged action | One review. A low-risk action gets only `{"outcome":"allow"}` |
| What it sees | User messages and tool calls. Never the agent's narration of the action | A compact transcript with tool outputs. Never hidden reasoning |
| No check needed | Reads, search, and edits in the project | Actions that need no approval |
| Too many denials | Asks the user after 3 in a row or 20 in a session | Stops the turn after 3 in a row or 10 of the last 50 |
| Review fails | Asks the user | The action does not run, and the agent is told that this is not a denial |

Neither uses a separate classifier model. Both send a policy prompt to a general model. Guard models such as ShieldGemma moderate content, and do not judge the risk of a command.

A new kind of model fits better. [Jev](https://openrouter.ai/docs/guides/community/jev) (`typesafe/jev-1.14`, from TypeSafe) takes a `state` and named `questions`, and returns typed answers with a `confidence`. The classes are plain language at call time, so it needs no training. It returns no reasoning, and it is not a chat model. The user wants a choice of models in this category, with no lock-in to Jev.

Anthropic [reports](https://www.anthropic.com/engineering/claude-code-auto-mode) a false block rate of 8.5 percent for the first stage and 0.4 percent for both stages, on 10,000 real actions. Both stages together missed 17 percent of 52 real overeager actions. So the classifier is a convenience and not a security boundary. Worker policy and the OS boundary still hold.

**The design for Consort:**

- The classifier runs on the hub side and has three kinds of source. The API keys stay on the hub.
  - **A model of a hub account.** This is the default: the smallest model of the accounts that the user has, for example Haiku or Luna.
  - **A decision model.** A base URL, an API key and a model name for the `state` and `questions` shape, for example Jev from TypeSafe or through OpenRouter's Decisions API. An answer below a confidence limit counts as unsure.
  - **A chat model.** A base URL, an API key and a model name for an OpenAI-compatible endpoint, for example OpenRouter or Cloudflare AI Gateway. Check Cloudflare's support when this is built.
- All three sit behind one interface, with one answer: allow, block or unsure. A new category of model adds one adapter.
- Settings > Models has the setting, next to the fallback path.
- Actions that the table allows make no call. Only shell and unknown tools reach the classifier.
- A chat model and an account model use two stages, as in Claude Code: a short answer first, and reasoning with the same model only for a flagged action. A decision model has one stage and gives no reason, so Consort names the rule that the question tested.
- It sees user messages and the tool call. It does not see assistant text or tool outputs, so an injected file cannot argue with it. It uses Consort's own policy prompt, not a copy of another product's prompt.
- A denial goes back to the agent with the reason. After 3 denials in a row or 20 in a turn, Consort asks the user.
- A timeout, an error or an unclear answer asks the user. It never allows the action.
- A check through an account uses a CLI, so it uses that plan's limits and starts slower. Measure this in the experiment. An API endpoint has neither problem, but it costs money and sends commands to that provider.

## What the experiment changed

The experiment ran on 2026-10-30. Its report is [research/hub-boundary-experiment.md](research/hub-boundary-experiment.md). The split goes ahead. These rules add to the rules above, and they win if they disagree.

1. The session host has an egress proxy, and the boundary denies all other network access. The proxy allows only the provider of that session. `sandbox-exec` cannot allow one provider by itself.
2. One fixed configuration for all four approval modes: mode `default` and `settingSources: []`. Never use `bypassPermissions`. `tools: []` alone still runs hooks and MCP servers on the host, and `["user"]` still runs `apiKeyHelper`. So the approval mode changes the hub's engine and not the CLI's configuration.
3. One function builds this configuration for a new session and for a load. A load without it brings back all native tools. A contract test on the fake provider runs for each version bump.
4. A tool that waits for an approval uses SSE with a progress notification each 15 s. A silent wait fails at 60 s or 300 s. `process.wait` stays short. A wait of hours is not tested.
5. The journal key is session, operation ID and argument hash. A patch replay checks the file. Process records persist. Each replay is marked.
6. The CLI always tells the model the hub's folder, OS and shell. Consort's environment message overrides this on each turn.
7. In Docker, bubblewrap needs a narrow seccomp profile, or use systemd. Consort refuses to run with no boundary. Landlock is only a second layer.
8. The session host has a start watchdog, and it kills by process group. A hung CLI ignored the loss of its parent for 28 minutes.
9. The development warning on a Mac says that keychain items are not separated per profile. This is not tested on a real item.
10. Subagents, the `Skill` tool, web tools and today's `plugins` route for skills are not available to a hub-side CLI. So `skill.read` comes at the start, and web tools need a Consort tool if the user wants them.
11. The MCP token stays per session, because it is on the CLI's command line. On Linux the session host runs as its own user.
12. The adapter bundles its own CLI, so a version pin needs `CLAUDE_CODE_EXECUTABLE`.
13. The command check through an account took 2.2 s (median, n=6) in the boundary. It has its own deadline, because a CLI with no proxy setting hung for more than 45 s.

Needed before step 4, and the state on 2026-10-31: a Windows worker on a second machine, a script skill and a real hub restart ran (see [E2E.md](E2E.md)). A real model on Linux and a token refresh in the boundary did not run, so step 4 is not done.

## Order

Each step leaves a working app. With no users there are no migrations, and development state starts fresh at the cutover.

1. **Experiment, about three days.** One pinned Claude version, a Linux session host and a Windows worker. Only inspect, read, patch, process and one script skill. It passes with a real edit-and-test task, correct Windows paths, no native tools, no access to another profile or hub data, a working resume, no duplicate change after a dropped reply, and a recorded result for a hub restart in a turn. A failed boundary stops the central deployment. Poor tool quality changes the adapter. A duplicate change changes the journal design.
2. **Split the crate.** Session host and execution worker, and the ADR and doc changes.
3. **Coordinators and all accounts move, with the one models page.** Coordinators do no file or shell work, so they need no bridge.
4. **Tasks move, one harness at a time.** A harness is on only after its tests pass. Enforcement is proven for none of them yet.
5. **Remove accounts from workers.**

## Questions for the user

None.

## Answered by the user

- **A stolen hub (2026-10-31, replaces the answer of 2026-10-30).** The hub controls everything, also the limits of a worker. A stolen hub compromises the workers. That is accepted, as a stolen laptop with SSH keys is. There are no signed changes. The user changes the limits in the app, and nobody edits `worker.toml`.
- **Worker setup (2026-10-31).** A worker setup over SSH that installs the CLI on the worker comes later. Worker setup must stay as easy as possible.
- **Provider plugins and hooks (2026-10-30).** Always off. There is no setting for them.
- **Hub size (2026-10-30).** A limit on running sessions, and a visible queue for other work.
- **Auto and shell commands (2026-10-30).** Use a classifier model, not command templates. See "The classifier model".
- **Hub on a Mac in development (2026-10-30).** Use a sandbox for now, with a visible warning. A Linux VM comes later, and it is not part of this work. The sandbox is not tested yet. `scripts/dev.sh` must stay one command.
