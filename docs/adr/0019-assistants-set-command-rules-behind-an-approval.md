# Assistants set command rules behind an approval

A channel has command rules: shell commands that run without asking, and shell commands that always ask. The user says what they want in chat, and the assistant of the channel or Tutti proposes the rules. A change that makes a channel less strict waits for the user. The user can edit rules and add rules by hand, but never has to.

## Context

- The user wants to say "PR anything you want, just ask for merges" to a channel's assistant, and to have that hold.
- An approval mode covers all shell commands. Only Full access lets a push through with no check, and it lets a merge through too.
- In Auto, the command check is a model. It blocks a change to a shared branch that the user did not ask for, and a block is no question: the user gets no card.
- The user decided on 2026-11-01 that this is a theme of the app. Assistants can change any configuration when the user asks, behind an approval. A settings page is never the only way.

## Decision

**A rule.** A rule has a pattern (a program and its words, such as `git push`), exceptions, a result (Allow or Ask), one plain sentence, and who added it. Rules are on the channel. The rules of Tutti apply in every channel.

**The order for a shell command.**

1. Worker policy comes first. A worker that does not allow shell denies. A worker whose policy holds sessions below Auto gets no help from an Allow rule.
2. The hub splits the command into simple commands.
3. If any part has an Ask rule as its result, the user decides, in every approval mode. Full access does not cancel it.
4. If every part has an Allow rule as its result, the command runs, in every approval mode. `cd` and `true` do not count against this.
5. Otherwise the approval mode decides. In Auto, the command check reads the rules.

**Which rule wins.** For one part, the most specific rule wins: the pattern with the most words. Ask wins a tie. A channel's rule replaces Tutti's rule for the same pattern.

**Two ways to read a command.** Each way fails safe.

- Ask reads loosely. It looks through paths, variables, `sudo`, `env` and `sh -c`. In text that it cannot parse, it looks for the words in order. A false match only asks.
- Allow reads strictly. A variable, a path to the program, a wrapper, an interpreter, a redirect other than `2>&1`, a command substitution or `git -c` stops it. A missed match only falls through to the approval mode.
- Text to a running process (`process_write_stdin`) can match Ask. It never matches Allow.

**Who changes rules.**

- A channel's assistant changes the rules of its own channel, with `set_permissions`.
- Tutti changes the rules of any channel, and its own rules, which apply everywhere.
- A task session has no such tool, so a blocked task cannot widen its own room.
- A proactive turn cannot call the tool.
- The user changes any rule by hand with `channel.update`, with no card.

**What asks.** A change that only makes a channel stricter runs at once and posts an activity line: a new Ask rule, a removed Allow rule, or more exceptions on an Allow rule. Any other change posts one management card, with the exact patterns, and all of its rules apply together. The card has Allow and Deny. To change a proposal, the user answers the assistant.

## Limits

This is a convenience and not a lock. Patterns read shell text, so a script or `curl` to an API can do what a pattern names. A connector has its own approval rules, so "ask for merges" must also set those. Branch protection on the remote is the real lock, and an assistant that proposes a merge rule says so.

## Considered options

- **Only the command check, with better prompts.** A model cannot promise a card, and a block does not ask.
- **Regular expressions.** Hard to read on a card, and easy to get wrong. Words of a command are what the user approves.
- **Rules in a table of their own.** Rules follow the channel, and channel changes already reach every client in the feed. The list stays small.
- **Edit in the card.** A second editor to design and keep correct. Answering the assistant already changes a proposal.

## Consequences

- Every new setting should plan the assistant's path to it, not only a settings page.
- Allow rules give less than they seem to on a chained command: `cargo test && git push` goes to the approval mode. That is the safe side, and the command check reads the rules there.

Changed on 2026-11-01: `set_permissions` replaces `set_command_rules`, and it also changes connections. See ADR 0020.
