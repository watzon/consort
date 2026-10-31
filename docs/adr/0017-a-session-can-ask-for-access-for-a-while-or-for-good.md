# A session can ask for access, for a while or for good

A channel has only the connections and the Monid tools that the user gave it. A session can still see the names of the others, and it can ask for one with a normal approval card. The user chooses temporary access, lasting access or no.

## Context

- A connection's "Use in" and Monid's "Use in" limit what a channel can do. A session that cannot see what exists tells the user that it cannot do the work, or uses a paid tool in place of a connection that the user already has.
- `request_access` existed, but only a channel coordinator could call it. It always widened "Use in", and `list_connections` hid the connections it could ask for.
- A task often needs a service once. Lasting access for one job widens a channel for good.

## Decision

What a session sees:

- The prompt names the connections that the channel lacks, and says when the channel lacks Monid. It has names and handles only.
- `search_tools` shows these tools too, with `access: request`. The tools of these connections and Monid's catalog are not private. The user's data stays behind the access check.
- The connector and Monid tools stay listed when the hub has the service, because a harness may not read the list again after access changes. The hub checks access on each call, and the error says to call `request_access`.

What a session asks:

- Every session can call `request_access` with a connection, or with all of Monid, a Monid provider or one Monid tool. A skill still takes lasting access only, because a skill loads when a session starts.
- It asks for `temporary` (the default) or `permanent`, with a short reason.
- The hub posts a card and the tool waits for the answer, like `run_connector_tool`. A late answer arrives in a later turn.

What the user answers:

- The card has the length that the session asked for first, then the other length, then Deny. The user decides, whatever the session asked for.
- Temporary access in a task lasts until the task ends, and only that task has it. Temporary access in the chat lasts 24 hours, and tasks of the channel have it too.
- Temporary access is on the channel, so it shows in the Channel panel, and End removes it now. It changes no "Use in".
- Always widens "Use in", as before, and removes the channel's exception. Lasting access to one Monid tool, in a channel with no Monid, gives that channel only that tool.

Limits:

- Access does not change approvals. Each call still follows the rules of the connection or of Monid.
- The card asks in every mode, also in Full access. Full access removes approvals inside what a channel has. It does not widen what a channel has.
- A proactive turn and a research turn cannot ask (`policy_denied`, as before).
- A tool that the user marked "Never use here" does not show in a search and cannot be requested.
- A Deny stops the same request for 24 hours in that task or coordinator session. A second request while a card waits gets that card.

## Considered options

- Only lasting access. Rejected by the owner, and it widens channels for one job.
- A session picks the length with no choice for the user. Rejected, because the user owns the scope.
- Temporary access until the end of a coordinator session. Rejected, because a coordinator session can last for months.
- Hide the names of other connections. Rejected by the owner. Names give little away on a hub with one user, and a session cannot ask for what it cannot see.

## Consequences

- Each new connection changes the prompt of each channel, so idle coordinator sessions start again when one is added.
- The tool list is a little longer in channels that lack a service.
- Text in a page can steer a session to ask for access. The card is the gate, and it states what it gives.
