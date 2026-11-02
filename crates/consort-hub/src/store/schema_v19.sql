-- Version 19: the first conversation.
--
-- The hub sends the first greeting one time. The key `first_greeting` in
-- `meta` says that this is over. A hub that was used before this version
-- never gets the greeting, so it starts with the key set.
INSERT INTO meta (key, value)
SELECT 'first_greeting', 'skipped'
WHERE EXISTS (SELECT 1 FROM messages)
   OR EXISTS (SELECT 1 FROM tasks)
   OR EXISTS (SELECT 1 FROM channels WHERE kind <> 'tutti');
