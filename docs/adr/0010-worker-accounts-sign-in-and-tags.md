# Accounts live on workers, the app runs the sign-in, and tags choose workers

Revised by [ADR 0015](0015-the-hub-machine-owns-accounts-and-agents.md). Accounts, profile folders and managed tools now live in the session host on the hub machine, not on a worker. Sign-in from the app, one profile folder per account and channel tags for tasks stay.

Each account of a provider belongs to one worker and has its own profile folder in the worker's Consort home. The user makes accounts and signs in from the app, through the provider's own sign-in flow, which runs on the worker. Each worker installs and updates its own copy of each harness CLI. The user puts tags on workers and channels, and all work from a channel runs only on workers that have every tag of the channel.

This replaces part of ADR 0004. Logins still stay on workers. What changes is how they get there: the user no longer signs in on the worker machine with the harness's own command, and the worker no longer picks up a login that already exists on the machine.

## Context

- The user wants several accounts of one provider on one worker, for example a personal and a work Claude plan. Logins in the harness's default folders cannot do that, and shadow homes share too much.
- Signing in on each machine with a terminal command is hard on a headless worker and impossible from a phone.
- A global install of a harness CLI can change under a running session, or be missing, or be the wrong version for the adapter.
- Labels on workers had no rule for their text, and a channel had no way to keep its work on certain machines.

## Decision

Credentials:

- Subscription logins (Claude plan, Claude Console, ChatGPT) stay in the profile folder on the worker, in the harness's own files or keychain entry. Consort never reads them.
- API keys and other secret variables travel once from the app through the hub to the worker, inside `account.create` or `account.update`. The worker writes them to `env.json` in the profile folder (mode 0600 on Unix). The hub never stores them and never logs them. Inventory reports only the variable names, never the values. Every proto type that holds them shows only the names in its `Debug` output.
- The Claude sign-in code that the user pastes travels from the app to the hub to the worker, and the worker writes it to the stdin of `claude auth login`. Nobody stores it.

Sign-in:

- The hub keeps a `SignIn` record for each run of a flow, and sends each state change in the feed. A sign-in expires after 15 minutes without an update.
- The worker runs the provider's own flow: `claude auth login` in a pseudo-terminal, the Codex app-server login, an API key in `env`, or the ACP `authenticate` method.

Managed tools:

- The worker installs each harness CLI and Node.js in `<consort home>/tools/<tool>/<version>/`, checks each download against a published hash, and never updates a tool while a session uses it. Self-updates of the harnesses are off.

Tags:

- A tag is 1 to 32 lowercase letters, digits or `-`. A worker has user tags and detected tags. A channel's tags are requirements for coordinator turns, tasks and routine runs.

## Considered options

- Keep logins in the harness's default folders, as before. Rejected because one worker could then have only one login for each provider.
- Let the hub keep API keys and send them with each session. Rejected because the hub would then hold every key, and a stolen hub could leak them all.
- Use the harness CLIs that the user installed globally. Rejected because their version and their auto-updates are outside the worker's control.

## Consequences

- A new worker has no accounts. Accounts that a user wrote in `worker.toml` by hand still load.
- An API key passes through the hub's memory once. The hub must keep it out of the database, the logs and error messages.
- Each worker downloads tools from the internet and needs disk space for two versions of each.
- Stored JSON with `labels` still loads as `tags`. A label with capitals or spaces fails the tag rule the next time the user saves the tags of that worker.
- Before a public release, ask Anthropic to confirm that an app that starts `claude auth login` on the user's own machines and relays the pasted code is acceptable.
