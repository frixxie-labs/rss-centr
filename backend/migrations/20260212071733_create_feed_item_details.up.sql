CREATE TABLE feed_item_details (
    id           INTEGER   PRIMARY KEY AUTOINCREMENT,
    feed_item_id INTEGER   NOT NULL UNIQUE REFERENCES feed_items(id) ON DELETE CASCADE,
    summary      TEXT      NOT NULL,
    content      TEXT      NOT NULL,
    author       TEXT      NOT NULL,
    published_at TIMESTAMP NOT NULL
);
