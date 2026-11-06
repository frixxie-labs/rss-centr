-- The models of each stage, so usage can be compared per model. `model`
-- keeps the display label.
ALTER TABLE ai_summaries
    ADD COLUMN batch_model TEXT,
    ADD COLUMN final_model TEXT;
