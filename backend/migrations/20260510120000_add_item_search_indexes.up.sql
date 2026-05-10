CREATE INDEX IF NOT EXISTS idx_feed_items_search
ON feed_items
USING GIN (to_tsvector('simple', title || ' ' || url));

CREATE INDEX IF NOT EXISTS idx_feed_item_details_search
ON feed_item_details
USING GIN (to_tsvector('simple', summary || ' ' || content || ' ' || author));

CREATE INDEX IF NOT EXISTS idx_feeds_search
ON feeds
USING GIN (to_tsvector('simple', COALESCE(title, '') || ' ' || url));
