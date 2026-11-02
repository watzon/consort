# The hub machine owns accounts and agents, and a worker only runs work

The hub machine holds every provider, account, sign-in and harness CLI. A separate process on that machine, the session host, runs each CLI in a boundary. A worker is only a machine where work runs: it has no account, no CLI and no sign-in. This replaces ADR 0004 and changes ADR 0010: accounts, profile folders and managed tools move from the worker to the session host. Sign-in from the app, one profile folder per account and channel tags stay as ADR 0010 says.

## Context

- With accounts on each worker, the user signs in once per machine. A coordinator depends on a machine that can sleep.
- A CLI's built-in tools run on the machine where the CLI runs. A CLI on the hub machine cannot use its own `Bash` on a worker. So the CLI gets a fixed set of tools from Consort, and these tools run on the worker.
- The rule "the hub never runs a model" is rejected. The hub process itself still runs no model and holds no sign-in. The session host does that, with its own credentials and its own user.
- `docs/HUB-PLAN.md` holds the plan and the results of the experiment that these rules come from.

## Decision

- **Processes.** Three parts: the hub, the session host and the execution worker. The hub holds the database and the feed. The session host holds the CLIs, the accounts and the sessions. A worker holds checkouts and runs commands.
- **Credentials.** The session host and a worker pair on separate routes with separate credentials. A credential of one kind does not work on the route of the other.
- **Tools.** A task session has `process_start`, the other process tools and the file tools through the MCP server `consort`. Its CLI gets one fixed config for all approval modes: mode `default`, `settingSources: []`, no native tools, and never `bypassPermissions`. One function builds this config, and a contract test checks it.
- **Approvals.** The hub's approval engine decides for each tool call. The approval mode changes only the engine. Worker policy stays a separate upper limit.
- **Boundary.** The session host starts a session only inside a boundary: bubblewrap on Linux, `sandbox-exec` on a Mac for development. Network access goes through a proxy per session, which allows only the hosts of that provider. With no boundary, no session starts.
- **Harnesses.** A harness turns on only after its tests in the boundary pass. A harness that is not proven stays off with a clear message.
- **Placement.** The model selection and the execution placement are two separate choices. A coordinator needs no worker. Channel tags apply to tasks, not to coordinators.

## Consequences

- The user signs in once. A worker pairs with no sign-in.
- Claude plan logins now live on a machine that the user runs and that also runs the hub. ADR 0004 asked to confirm with Anthropic that a hub that starts Claude Code sessions on the user's machines is acceptable. Ask this again before a public release, for this new shape.
- A stolen hub process still has no login, but a stolen hub machine has all of them. On Linux the session host runs as its own user, to limit this.
- A Mac hub is for development only. `sandbox-exec` does not separate keychain items per profile, and Settings > Hub says so. ADR 0018 replaces this line, and the own-user rule above, for a hub on the user's own computer.
- There is no data migration. There are no users yet, and development data starts fresh at the cutover.
