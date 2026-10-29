# The hub's timeline is the source of truth, and harness sessions are caches

The hub stores messages, tasks, memory and normalized session items. A coordinator's harness session is a cache that the hub can rebuild with a context pack. So the user can change a channel's harness or model at any time, and a lost session costs nothing that the hub cannot rebuild.

## Consequences

Raw transcripts stay on the workers. The hub keeps normalized session items, and it keeps only the last 8 KiB of command output. A client gets the full output or diff on demand from the worker, which must then be online.
