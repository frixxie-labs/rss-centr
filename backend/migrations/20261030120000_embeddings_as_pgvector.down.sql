DROP FUNCTION IF EXISTS nearest_feed_item_embeddings(TEXT, vector, BIGINT, INT, BIGINT[]);
DROP FUNCTION IF EXISTS ensure_feed_item_embedding_index(TEXT, INT);
DROP FUNCTION IF EXISTS feed_item_embedding_index_name(TEXT, INT);

ALTER TABLE feed_item_embeddings ADD COLUMN embedding_bytes BYTEA;

UPDATE feed_item_embeddings e
SET embedding_bytes = (
    SELECT string_agg(float4send(v), ''::BYTEA ORDER BY n)
    FROM unnest(e.embedding::REAL[]) WITH ORDINALITY AS u(v, n)
);

-- Also drops the per-model HNSW indexes.
ALTER TABLE feed_item_embeddings DROP COLUMN embedding;
ALTER TABLE feed_item_embeddings RENAME COLUMN embedding_bytes TO embedding;
ALTER TABLE feed_item_embeddings ALTER COLUMN embedding SET NOT NULL;

CREATE INDEX idx_feed_item_embeddings_model_embedded_at
ON feed_item_embeddings (model, embedded_at);
