ALTER TABLE feeds
ADD COLUMN last_inserted_at TIMESTAMPTZ;

CREATE TABLE feed_update_queue (
    feed_id          BIGINT      PRIMARY KEY REFERENCES feeds(id) ON DELETE CASCADE,
    due_at           TIMESTAMPTZ NOT NULL,
    leased_at        TIMESTAMPTZ,
    lease_expires_at TIMESTAMPTZ,
    lease_token      TEXT,
    attempts         BIGINT      NOT NULL DEFAULT 0,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO feed_update_queue (feed_id, due_at)
SELECT id,
       CASE
           WHEN last_success_at IS NULL THEN NOW()
           ELSE last_success_at + (poll_interval_seconds * INTERVAL '1 second')
       END
FROM feeds;

CREATE INDEX idx_feed_update_queue_due_lease
ON feed_update_queue (due_at, lease_expires_at, feed_id);
