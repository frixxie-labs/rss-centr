-- Store keyword search vectors instead of rebuilding them on every search.
DROP INDEX IF EXISTS idx_feed_items_search;
DROP INDEX IF EXISTS idx_feed_item_details_search;
DROP INDEX IF EXISTS idx_feeds_search;

ALTER TABLE feed_items
ADD COLUMN search_vector tsvector
GENERATED ALWAYS AS (to_tsvector('simple', title || ' ' || url)) STORED;

ALTER TABLE feed_item_details
ADD COLUMN search_vector tsvector
GENERATED ALWAYS AS (to_tsvector('simple', summary || ' ' || content || ' ' || author)) STORED;

ALTER TABLE feeds
ADD COLUMN search_vector tsvector
GENERATED ALWAYS AS (to_tsvector('simple', COALESCE(title, '') || ' ' || url)) STORED;

CREATE INDEX idx_feed_items_search_vector ON feed_items USING GIN (search_vector);
CREATE INDEX idx_feed_item_details_search_vector ON feed_item_details USING GIN (search_vector);
CREATE INDEX idx_feeds_search_vector ON feeds USING GIN (search_vector);
