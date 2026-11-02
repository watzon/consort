# Permissions cover connections, and an assistant can own an account

The user says what an assistant can do in one sentence, such as "PR anything, just ask for merges". Command rules (ADR 0019) read shell commands only, so the same limit on a connector stayed open. And a connection had one rule per tool kind, so "ask before a merge" also asked before a pull request. The user also wants to give an assistant its own account, such as its own Gmail, and let it send with no card.

## Decision

- `set_permissions` replaces `set_command_rules`. One call and one card change command rules and connections together.
- A connection has **tool exceptions**: one tool with its own rule, which wins over the rule of its tool kind. An exception runs without asking or asks every time. It cannot ask once per task, because that allow counts for a whole tool kind.
- A connection can be **the assistant's account** (`assistant_owned`). What it sends does not go out in the user's name. So the voice does not apply, and proactive turns and draft tasks follow the rules of the connection instead of asking each time. The research pass stays read only.
- A connection keeps one set of rules, not one per channel, because it is one account. An assistant's connection change asks in both directions, also when it is stricter, because it reaches other channels. The card names them. A channel's assistant can change only a connection of its channel.
- The user edits permissions in the channel settings: Access, then Edit permissions. The user's own edits need no card.
- The Details of a command that a rule allowed name the rule. At rest it looks like any other command.

## Considered options

- **Rules on the Connectors tab.** Rejected by the user. Permissions answer "what can this channel do without asking", which is the question of the approval mode, so they sit next to it.
- **Connection rules per channel.** More to read and more to get wrong. "Use in" and the channel exceptions already say where an account works.
- **Turn off the checks with the rule "run without asking" alone.** That rule on a user's own account must not send in the user's name from a routine. Only the owner of the account changes the meaning.

## Consequences

- An assistant with its own account that reads and sends without asking can obey an email that it reads, and it can send channel files to anyone. The card says so in the failed color, and only the user can allow it. The activity list of the connection keeps each send.
- A tool name for the user comes from its slug, with no service prefix.
