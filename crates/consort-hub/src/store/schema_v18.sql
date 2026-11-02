-- Version 18: reach from other devices.
--
-- One row for each way that is on. `address` is the URL that peers use, and
-- the hub accepts its origin in a signature. `port` is the Serve port or the
-- TLS port. `target` is what Serve forwards to, for Tailscale.
CREATE TABLE hub_reach (
    method       TEXT PRIMARY KEY,
    address      TEXT NOT NULL,
    port         INTEGER NOT NULL,
    target       TEXT,
    turned_on_at INTEGER NOT NULL
);
