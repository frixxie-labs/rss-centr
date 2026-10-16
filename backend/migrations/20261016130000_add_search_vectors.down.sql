DROP INDEX IF EXISTS idx_feeds_search_vector;
DROP INDEX IF EXISTS idx_feed_item_details_search_vector;
DROP INDEX IF EXISTS idx_feed_items_search_vector;

ALTER TABLE feeds DROP COLUMN IF EXISTS search_vector;
ALTER TABLE feed_item_details DROP COLUMN IF EXISTS search_vector;
ALTER TABLE feed_items DROP COLUMN IF EXISTS search_vector;

CREATE INDEX IF NOT EXISTS idx_feed_items_search
ON feed_items
USING GIN (to_tsvector('simple', title || ' ' || url));

CREATE INDEX IF NOT EXISTS idx_feed_item_details_search
ON feed_item_details
USING GIN (to_tsvector('simple', summary || ' ' || content || ' ' || author));

CREATE INDEX IF NOT EXISTS idx_feeds_search
ON feeds
USING GIN (to_tsvector('simple', COALESCE(title, '') || ' ' || url));
