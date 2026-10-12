ALTER TABLE ai_summaries
    ADD COLUMN generation_ms BIGINT,
    ADD COLUMN llm_calls INTEGER,
    ADD COLUMN prompt_tokens BIGINT,
    ADD COLUMN output_tokens BIGINT,
    ADD COLUMN llm_ms BIGINT,
    ADD COLUMN eval_ms BIGINT;
