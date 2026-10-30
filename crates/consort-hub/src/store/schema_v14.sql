-- Version 14: tell clients that Tutti is in the Auto approval mode now.
--
-- Version 13 changed the row of Tutti and the old feed rows of Tutti, but a
-- client that saved its feed position asks only for rows after that
-- position, so it never saw the change. This version adds one new `upsert`
-- row for Tutti with the current channel, after the saved positions. A hub
-- that ran version 13 already gets the row here, and so does a hub that
-- moves from version 12 straight to this one. Version 13 stays as it is.
INSERT INTO feed (at, channel_id, entity, entity_id, op, payload)
SELECT CAST(strftime('%s', 'now') AS INTEGER) * 1000, id, 'channel', id, 'upsert', data
FROM channels
WHERE kind = 'tutti';
