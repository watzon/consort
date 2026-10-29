-- Version 4: sign-ins. One row for each run of a provider's sign-in flow
-- for an account on a worker. `data` is the `SignIn` JSON. `updated_at` is a
-- copy of `SignIn.updated_at` (Unix milliseconds), so the hub can delete old
-- rows without reading every document. The row never holds a sign-in code
-- or a secret variable.
CREATE TABLE sign_ins (
    id         TEXT PRIMARY KEY,
    worker_id  TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    data       TEXT NOT NULL
);

CREATE INDEX sign_ins_worker ON sign_ins (worker_id);
CREATE INDEX sign_ins_updated ON sign_ins (updated_at);
