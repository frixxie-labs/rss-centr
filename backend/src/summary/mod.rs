mod prompts;

pub(crate) use prompts::{
    ARTICLES_PER_BATCH, BATCH_OUTPUT_TOKENS, ContextBudget, FINAL_OUTPUT_TOKENS, InputTooLarge,
    SUMMARY_CONTEXT_TOKENS, SUMMARY_SYSTEM_PROMPT, budgeted_prompts, build_batch_prompt,
    build_final_prompt, build_merge_prompt,
};

#[derive(sqlx::FromRow)]
pub(crate) struct SummaryArticle {
    pub(crate) id: i64,
    pub(crate) feed_id: i64,
    pub(crate) title: String,
    pub(crate) feed: String,
    pub(crate) description: String,
}
