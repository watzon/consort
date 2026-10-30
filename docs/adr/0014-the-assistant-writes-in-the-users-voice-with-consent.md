# The assistant writes in the user's voice, with consent and limits

When the user turns on "Write in my voice", drafts that go out in the user's name follow a style guide that Consort learned from text the user wrote. The guide is a normal memory note, `voice.md`, that the user can read, edit and reset. The voice changes only the wording. The full design is [VOICE-PLAN.md](../VOICE-PLAN.md).

## Context

- The user asked for the Claude Duet setting "Speak in your voice". No public source documents how Duet does it. The research is in [research/speak-in-your-voice.md](../research/speak-in-your-voice.md).
- Comparable products (Gmail, Superhuman, Grammarly, Claude styles, OpenAI dots, OpenClaw) learn from the user's own sent text. Their known problems are hidden learned state and drafts that make commitments in the user's name.
- Consort already has global memory notes with versions, tasks that read connectors, suggestions that only the coordinator writes, and approval cards for send tools.

## Decision

- Off by default. The user chooses each source. A task learns from the sources on a worker; raw text never becomes memory.
- The learned guide is the global memory note `voice.md`, with sections per context and short screened examples. The user approves the first guide and every learned change.
- The hub adds the voice only to sessions that can draft or send through a connector, and only for text that goes out in the user's name.
- The hub scans drafts for dates, amounts and agreements that the request did not contain, and the approval card makes the user check them. The voice never loosens the approval rule of a send tool.
- Edits on the approval card teach the voice. A rule changes only when at least 3 edits agree.

## Considered options

- A hidden style profile. Rejected because the user must see and correct what the assistant believes about them.
- Learning from the user's chat with the assistant. Rejected as a default because that chat is not how the user writes to other people.
- Applying the voice to every reply. Rejected because the assistant's own voice should stay plain and consistent.

## Consequences

- New hub settings, a voice task brief, a `voice_guide` tool, draft pairs, and new fields on approval cards.
- Approval cards for send and draft tools get Edit, which is useful even with the voice off.
