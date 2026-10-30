-- Version 6: the main channel is called Tutti (ADR 0012).
--
-- The channel kind `everything` becomes `tutti`, in the `kind` column, in the
-- JSON of the channel, and in the feed entries and wake events that name it.
-- The Rust types still read the old spelling (`serde` aliases), so a missed
-- row is harmless. The unique index moves to the new value.
DROP INDEX channels_one_everything;

UPDATE channels
SET kind = 'tutti',
    data = json_set(
        data,
        '$.kind', 'tutti',
        '$.name', CASE json_extract(data, '$.name')
            WHEN 'Everything' THEN 'Tutti'
            ELSE json_extract(data, '$.name')
        END
    )
WHERE kind = 'everything';

CREATE UNIQUE INDEX channels_one_tutti ON channels (kind) WHERE kind = 'tutti';

UPDATE feed
SET payload = json_set(
        payload,
        '$.kind', 'tutti',
        '$.name', CASE json_extract(payload, '$.name')
            WHEN 'Everything' THEN 'Tutti'
            ELSE json_extract(payload, '$.name')
        END
    )
WHERE entity = 'channel' AND json_extract(payload, '$.kind') = 'everything';

-- The wake event `everything_message` is now `tutti_message`.
UPDATE wake_events
SET event = json_set(event, '$.type', 'tutti_message')
WHERE json_extract(event, '$.type') = 'everything_message';

UPDATE coordinators
SET turn_events = replace(turn_events, '"type":"everything_message"', '"type":"tutti_message"')
WHERE turn_events LIKE '%everything_message%';
