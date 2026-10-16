-- Packed bytes cannot be converted back in SQL; drop the embeddings so the
-- worker re-indexes every item.
DROP INDEX IF EXISTS idx_feed_item_embeddings_model_embedded_at;
DELETE FROM feed_item_embeddings;
ALTER TABLE feed_item_embeddings DROP COLUMN embedding;
ALTER TABLE feed_item_embeddings ADD COLUMN embedding REAL[] NOT NULL;
