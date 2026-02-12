CREATE TABLE feed_items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    feed_id     INTEGER   NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    external_id TEXT      NOT NULL,
    title       TEXT      NOT NULL,
    url         TEXT      NOT NULL,
    inserted_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(feed_id, external_id)
);
