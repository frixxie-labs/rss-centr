-- Store embeddings as packed big-endian f32 bytes: decoding REAL[] dominated
-- semantic search time.
ALTER TABLE feed_item_embeddings ADD COLUMN embedding_bytes BYTEA;

UPDATE feed_item_embeddings e
SET embedding_bytes = COALESCE(
    (
        SELECT string_agg(float4send(v), ''::BYTEA ORDER BY n)
        FROM unnest(e.embedding) WITH ORDINALITY AS u(v, n)
    ),
    ''::BYTEA
);

ALTER TABLE feed_item_embeddings DROP COLUMN embedding;
ALTER TABLE feed_item_embeddings RENAME COLUMN embedding_bytes TO embedding;
ALTER TABLE feed_item_embeddings ALTER COLUMN embedding SET NOT NULL;

-- Lets backends fetch only embeddings stored since their last refresh.
CREATE INDEX idx_feed_item_embeddings_model_embedded_at
ON feed_item_embeddings (model, embedded_at);
