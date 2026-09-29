CREATE TABLE summary_refresh_queue (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    due_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    lease_token TEXT,
    lease_expires_at TIMESTAMPTZ
);

INSERT INTO summary_refresh_queue (id) VALUES (TRUE);
