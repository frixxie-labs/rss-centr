CREATE TABLE feeds (
    id                    BIGSERIAL PRIMARY KEY,
    url                   TEXT        NOT NULL UNIQUE,
    title                 TEXT,
    site_url              TEXT,
    etag                  TEXT,
    last_modified         TEXT,
    poll_interval_seconds BIGINT      NOT NULL DEFAULT 300,
    is_enabled            BOOLEAN     NOT NULL DEFAULT TRUE,
    last_checked_at       TIMESTAMPTZ,
    last_success_at       TIMESTAMPTZ,
    failure_count         BIGINT      NOT NULL DEFAULT 0
);
