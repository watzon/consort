# Consort is the source of connectors and skills, Composio sits behind the hub, and tags scope access

The hub, not the provider of each harness, is the default source of the assistant's connectors (MCP-style tools for outside services) and skills. Connectors come from Composio, which only the hub talks to, with a key that only the hub holds. Skills are folders in the Agent Skills format that the hub installs, pins and delivers to the workers. A connection or a skill reaches a channel through its "Use in" scope, which uses the tags that channels already have, minus the channel's own exceptions. The hub enforces the scope, the approval rules and the management rules. The prompt only describes them.

## Context

- The user runs several harnesses (Claude Code, Codex, Gemini CLI, OpenCode, ACP agents) on several workers. Each has its own way to add MCP servers and skills, and each provider syncs its own extras, such as claude.ai connectors. Set up per harness and per worker, the assistant would see different tools in different places, and the workers would drift out of sync.
- The assistant must know which connectors a channel has without searching for them, and must tell two accounts of one service apart, such as two Gmail accounts.
- A connector can send email or delete files. A skill can carry scripts. Both need approvals and an audit that do not depend on a model following its prompt.
- The assistant should be able to manage connectors, skills, channels and accounts, but never handle a secret.

## Decision

Connectors:

- The user brings a Composio project key. The hub stores it encrypted at rest with a key that the hub makes, in a file with mode 0600. The key never leaves the hub, and no client request returns it.
- The hub has one Composio `user_id`. Each connection is one Composio connected account, with a name, a unique handle, a "Use in" scope and an approval rule for each tool kind (read, change, delete).
- Sessions use connectors only through three hub tools: `list_connector_tools`, `get_connector_tool` and `run_connector_tool`. The hub checks that the connection is in the session's effective set, applies the approval rule (a connector approval card when it asks), runs the tool with the exact connected account, and writes an audit record with a hash of the arguments, never the arguments.
- Credentials for a scheme with no browser step go from the app through the hub to Composio once. Consort does not store them.

Skills:

- The hub fetches a skill from a git repo, a directory entry or an upload, checks the format and the limits, pins it to a commit and a content hash, and stores the files content-addressed. A skill with a failed audit is blocked. A skill with scripts stays off until the user turns it on. Claude-specific hooks are removed.
- A worker gets the skills of a session by content hash (`skill_bundle.get` and the blob endpoint) and writes them where its harness reads skills. A harness with no skill support gets `list_skills` and `read_skill`.

Scope:

- "Use in" is every channel, the channels with all of some tags, or a list of channels. The Everything channel matches only "every channel", unless it has tags of its own. A channel can turn off an item that its scope gives it (a channel exception), but cannot turn on one that its scope does not give it.

Management:

- Management tools are hub tools with audiences: the Everything coordinator manages everything, a channel's coordinator manages its own channel. A change that is new and narrow, or that makes access smaller or approvals stricter, runs at once. A change that deletes, widens access, loosens approvals, turns on scripts or changes tags waits for the user on a management approval card. Sign-ins and keys always go through a card that the user completes.
- Every card is an approval entity (a hub card), so the existing approval cards, `approval.answer`, push notifications and Needs input work for it.

Provider extras:

- Each harness account had an extras toggle, on by default. Later decision, 2026-10-30: the toggle is removed. Provider extras are always off, as far as the provider lets the session host turn them off (DECISIONS row 806).

## Considered options

- Configure MCP servers and skills on each worker, per harness. Rejected because the set would drift between workers and harnesses, and the hub could not enforce approvals or scope.
- Send the Composio key to the workers and let each harness talk to Composio's MCP endpoint. Rejected because every worker would then hold the key, and the hub could not audit or gate a call.
- Give each channel its own list of connections and skills. Rejected because tags already describe which channels belong together, and a list per channel goes stale when the user adds a channel.
- Put the effective set in the prompt only and trust the model. Rejected because text in a page or a file can steer a model.
- A separate card entity next to approvals. Rejected because approvals already have the card message, the answer path, the push and the Needs input status.

## Consequences

- The hub depends on Composio for connectors. Without a key, the connector tools fail with `composio_not_configured`. Composio sees the tool arguments and results of every call.
- Every connector call goes through the hub, so a call adds one hop and the hub's tool call limit applies.
- The hub stores skill files, and workers keep a cache by content hash. A new skill version reaches only new sessions.
- Old clients cannot read a hub card. The desktop app and the hub ship together, so this is acceptable before the first release.
- A provider extra that a harness cannot turn off still reaches its sessions. The account dialog says so.
