-- Story notes generated for a batch prompt, keyed by a hash of the model and
-- prompt, so unchanged batches are not sent to the model again.
CREATE TABLE summary_note_cache (
    prompt_hash BYTEA PRIMARY KEY,
    notes TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_summary_note_cache_created_at ON summary_note_cache (created_at);
