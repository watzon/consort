-- Version 10: write in the user's voice.
--
-- The voice settings are one JSON record in `meta` under the key `voice`,
-- like the proactivity settings. A style guide that waits for "Do it" is
-- in `meta` under `voice_proposal`.

-- Drafts and what the user sent, for learning from edits. The hub keeps
-- 90 days. `unchanged` pairs are text that the assistant wrote; they are
-- never used for learning. `used` is set when a `voice_edits` wake took
-- the pair.
CREATE TABLE draft_pairs (
    approval_id TEXT PRIMARY KEY,
    context     TEXT,
    unchanged   INTEGER NOT NULL,
    used        INTEGER NOT NULL DEFAULT 0,
    at          INTEGER NOT NULL,
    data        TEXT NOT NULL
);
CREATE INDEX draft_pairs_at ON draft_pairs (at);
CREATE INDEX draft_pairs_context ON draft_pairs (context, unchanged, used);

-- Each `voice_guide` call: the session and the context it asked for. The
-- connector card of the session says which voice wrote the draft.
CREATE TABLE voice_uses (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    context    TEXT NOT NULL,
    at         INTEGER NOT NULL
);
CREATE INDEX voice_uses_session ON voice_uses (session_id, id);
