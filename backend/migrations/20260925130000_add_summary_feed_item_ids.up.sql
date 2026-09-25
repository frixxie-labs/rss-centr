ALTER TABLE ai_summaries
ADD COLUMN feed_item_ids BIGINT[] NOT NULL DEFAULT '{}';
