CREATE INDEX IF NOT EXISTS idx_feed_items_feed_id_inserted_at_id
ON feed_items (feed_id, inserted_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_feed_items_inserted_at_id
ON feed_items (inserted_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_feeds_is_enabled_id
ON feeds (is_enabled, id);
