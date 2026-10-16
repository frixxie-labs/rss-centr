CREATE TABLE feed_item_embeddings (
    feed_item_id BIGINT PRIMARY KEY REFERENCES feed_items(id) ON DELETE CASCADE,
    model        TEXT        NOT NULL,
    content_hash TEXT        NOT NULL,
    embedding    REAL[]      NOT NULL,
    embedded_at  TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_feed_item_embeddings_model
ON feed_item_embeddings (model);

CREATE TABLE feed_item_embedding_leases (
    feed_item_id BIGINT PRIMARY KEY REFERENCES feed_items(id) ON DELETE CASCADE,
    leased_until TIMESTAMPTZ NOT NULL
);
