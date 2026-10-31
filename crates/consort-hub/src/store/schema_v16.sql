-- Version 16: the fallback path.
--
-- A hold says why an account is skipped now. `reason` is `usage_limit` or
-- `outage` (a provider that was down or overloaded for three turns).
-- `exact` is 1 when the provider gave the time when the hold ends, and 0
-- when the hub chose a fixed time. A notice says "It returns at" only for an
-- exact time.
ALTER TABLE account_limits ADD COLUMN reason TEXT NOT NULL DEFAULT 'usage_limit';
ALTER TABLE account_limits ADD COLUMN exact INTEGER NOT NULL DEFAULT 1;

-- The turns of a session that failed in a row with a provider outage. The
-- hub tries the turn again until the count reaches three, then the path
-- moves the work. A turn that ends well clears the row.
CREATE TABLE session_outage_tries (
    session_id TEXT PRIMARY KEY,
    tries      INTEGER NOT NULL
);
