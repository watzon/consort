# Workers and clients connect out to the hub

Every worker and every client opens its own connection to the hub. The hub never connects to them. So a worker behind a home router needs no open ports, no Tailscale and no relay service.

## Considered options

- The hub connects to workers over Tailscale or SSH, as T3 Code does. Rejected because every worker would then need a route from the VPS.

## Consequences

The hub keeps a command queue for each offline worker. Each worker spools session events on disk until the hub acknowledges them.
