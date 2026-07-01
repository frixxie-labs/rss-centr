DROP TABLE IF EXISTS feed_update_queue;

ALTER TABLE feeds
DROP COLUMN IF EXISTS last_inserted_at;
