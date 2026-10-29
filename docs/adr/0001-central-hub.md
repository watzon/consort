# One central hub, with workers and clients

The hub runs on a VPS, stores all state and decides what runs where. Workers run sessions, and clients talk only to the hub. We chose one hub over a mesh of equal nodes because "one assistant, one memory" needs one writer. A mesh brings duplicate replies when two nodes both act as the coordinator, routines that run twice or never, and consensus that fails when laptops sleep. The hub machine can also run a worker, so one machine is a complete install.

## Considered options

- A mesh of equal nodes with replicated state. Rejected for the reasons above.
- Equal nodes with one movable "home" role. Rejected because the user values hub uptime and phone access when the home network is down more than running without a server.
