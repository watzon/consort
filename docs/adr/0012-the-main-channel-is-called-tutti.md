# The main channel is called Tutti everywhere

The main channel is called Tutti in the UI, the code, the wire, the storage, the prompts and the docs. The old name, Everything, is gone from new data. The code and the hub still read it, so old data and older peers keep working.

## Context

- The app text already said Tutti. The code, the wire and the storage said Everything. One channel had two names.
- Everything is the name of the main channel in Claude Duet. Consort should not copy that name.
- "Tutti" is the music word for "all voices play together". The main channel is the place where the assistant reads every other channel. The word fits, and it matches the Three voices mark.

## Decision

- The main channel is called Tutti in every layer. The names are `ChannelKind::Tutti`, `ToolAudience::TuttiCoordinator`, `MemoryActor::TuttiCoordinator` and `WakeEvent::TuttiMessage`. The wire value is `tutti` (`"kind": "tutti"`). The wake event element is `<tutti_message>`. The unique index is `channels_one_tutti`. The tool restriction is `TUTTI_ONLY`.
- The hub reads old values through serde aliases. `everything` is an alias of `ChannelKind::Tutti`. `everything_coordinator` is an alias of `ToolAudience::TuttiCoordinator`. `everything_message` is an alias of `WakeEvent::TuttiMessage`. The code writes only the new values.
- Hub migration 6 (`schema_v6.sql`) rewrites the stored values to `tutti`. It changes `channels.kind`, the channel JSON, the feed payloads, `wake_events` and `coordinators.turn_events`. It also renames the channel from "Everything" to "Tutti".
- The option `--screen everything` stays as an alias of `--screen tutti`.

## Considered options

- Keep Everything in the code and the wire, and say Tutti only in the UI. Rejected because one channel would keep two names, and a reader of the code would need a translation table.
- Rename the code and not read the old values. Rejected because stored data and older peers would break.

## Consequences

- An older client cannot read the new `tutti` values from a new hub. The hub and the clients ship together, so this is acceptable before the first release.
- A new client and a new hub read old stored data and the values of old peers.
- The history files keep the old name on purpose: the older ADRs, `docs/DECISIONS.md`, `docs/E2E.md` and `docs/notes/`. They record what was true when the author wrote them.
