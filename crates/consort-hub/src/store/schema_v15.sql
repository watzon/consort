-- Version 15: the accounts that reached a usage limit, and until when.
--
-- Placement does not choose such an account while another signed-in account
-- of the same harness has the model. A row has no use after `until_at`.
CREATE TABLE account_limits (
    account_id TEXT PRIMARY KEY,
    until_at   INTEGER NOT NULL
);
