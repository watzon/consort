-- Version 20: the image cache of visuals.
--
-- One row for each image that the session host drew for a visual. `key` is
-- the SHA-256 of the page hash, the width, the scale and the style (see
-- `ops/visuals.rs`). The PNG is a blob. The rows of a message go with the
-- message.
CREATE TABLE visual_images (
    key          TEXT PRIMARY KEY,
    message_id   TEXT NOT NULL,
    image_sha256 TEXT NOT NULL,
    size         INTEGER NOT NULL,
    height       INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
);

CREATE INDEX visual_images_message ON visual_images (message_id);
