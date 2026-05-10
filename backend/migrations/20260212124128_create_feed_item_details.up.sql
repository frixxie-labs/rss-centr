CREATE TABLE feed_item_details (
    id           BIGSERIAL PRIMARY KEY,
    feed_item_id BIGINT      NOT NULL UNIQUE REFERENCES feed_items(id) ON DELETE CASCADE,
    summary      TEXT      NOT NULL,
    content      TEXT      NOT NULL,
    author       TEXT      NOT NULL,
    published_at TIMESTAMPTZ NOT NULL
);
