CREATE TABLE analytics_events (
    id         BIGSERIAL PRIMARY KEY,
    event_type TEXT        NOT NULL,
    path       TEXT        NOT NULL,
    referrer   TEXT,
    visitor_id TEXT,
    session_id TEXT,
    feed_id    BIGINT      REFERENCES feeds(id) ON DELETE SET NULL,
    item_id    BIGINT      REFERENCES feed_items(id) ON DELETE SET NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_analytics_events_occurred_at
ON analytics_events (occurred_at DESC);

CREATE INDEX idx_analytics_events_event_type_occurred_at
ON analytics_events (event_type, occurred_at DESC);

CREATE INDEX idx_analytics_events_path_occurred_at
ON analytics_events (path, occurred_at DESC);
