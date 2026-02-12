CREATE TABLE feeds (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    url                   TEXT      NOT NULL UNIQUE,
    title                 TEXT,
    site_url              TEXT,
    etag                  TEXT,
    last_modified         TEXT,
    poll_interval_seconds INTEGER   NOT NULL DEFAULT 300,
    is_enabled            INTEGER   NOT NULL DEFAULT 1,
    last_checked_at       TIMESTAMP,
    last_success_at       TIMESTAMP,
    failure_count         INTEGER   NOT NULL DEFAULT 0
);
