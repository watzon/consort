-- Version 13: Tutti defaults to the Auto approval mode.
--
-- New channels and a new Tutti start in Auto. An existing Tutti moves from
-- Ask to Auto once, here. Only a Tutti that is still in Ask moves: a Tutti
-- that the user set to another mode keeps it. Other channels keep their
-- mode. The feed rows of Tutti change too, so a client that replays them
-- does not bring Ask back.
UPDATE channels
SET data = json_set(data, '$.default_approval_mode', 'auto')
WHERE kind = 'tutti'
  AND json_extract(data, '$.default_approval_mode') = 'ask';

UPDATE feed
SET payload = json_set(payload, '$.default_approval_mode', 'auto')
WHERE entity = 'channel'
  AND json_extract(payload, '$.kind') = 'tutti'
  AND json_extract(payload, '$.default_approval_mode') = 'ask';
