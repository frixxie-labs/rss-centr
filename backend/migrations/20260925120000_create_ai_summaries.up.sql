CREATE TABLE ai_summaries (
    id BIGSERIAL PRIMARY KEY,
    summary TEXT NOT NULL,
    feed_ids BIGINT[] NOT NULL,
    generated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    model TEXT NOT NULL
);

CREATE INDEX idx_ai_summaries_generated_at_id
ON ai_summaries (generated_at DESC, id DESC);
