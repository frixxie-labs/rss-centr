CREATE TABLE feed_items (
    id          BIGSERIAL PRIMARY KEY,
    feed_id     BIGINT      NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    external_id TEXT      NOT NULL,
    title       TEXT      NOT NULL,
    url         TEXT      NOT NULL,
    inserted_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(feed_id, external_id)
);
