-- Consecutive failed refreshes, used to back off retries.
ALTER TABLE summary_refresh_queue ADD COLUMN failures INTEGER NOT NULL DEFAULT 0;
