ALTER TABLE ai_summaries
    DROP COLUMN generation_ms,
    DROP COLUMN llm_calls,
    DROP COLUMN prompt_tokens,
    DROP COLUMN output_tokens,
    DROP COLUMN llm_ms,
    DROP COLUMN eval_ms;
