mod config;
mod error;
mod ollama;
mod prompts;
mod repository;
mod service;

use std::future::Future;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use utoipa::ToSchema;

pub(crate) use prompts::{
    ARTICLES_PER_BATCH, BATCH_OUTPUT_TOKENS, ContextBudget, FINAL_OUTPUT_TOKENS, InputTooLarge,
    MergeRound, SUMMARY_CONTEXT_TOKENS, SUMMARY_SYSTEM_PROMPT, budgeted_prompts,
    build_batch_prompt, plan_merge_round,
};

pub(crate) use config::SummaryModels;
pub(crate) use error::SummaryError;
pub(crate) use ollama::OllamaSummaryGenerator;
#[cfg(test)]
pub(crate) use repository::LEASE_SECONDS;
pub(crate) use repository::{PostgresSummaryRepository, SummaryRepository, SummaryStoreError};
pub(crate) use service::SummaryState;

pub(crate) trait SummaryGenerator: Send + Sync {
    fn context_budget(
        &self,
        model: &str,
    ) -> impl Future<Output = Result<ContextBudget, SummaryError>> + Send;

    fn generate(
        &self,
        model: &str,
        budget: ContextBudget,
        prompt: String,
        num_predict: u32,
    ) -> impl Future<Output = Result<Generated, SummaryError>> + Send;
}

#[derive(Clone, Serialize, FromRow, ToSchema)]
pub struct DailySummary {
    pub id: i64,
    pub summary: String,
    pub feed_ids: Vec<i64>,
    pub feed_item_ids: Vec<i64>,
    pub generated_at: DateTime<Utc>,
    pub model: String,
    pub generation_ms: Option<i64>,
    pub llm_calls: Option<i32>,
    pub prompt_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub llm_ms: Option<i64>,
    pub eval_ms: Option<i64>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SummaryUsage {
    pub(crate) llm_calls: i32,
    pub(crate) prompt_tokens: i64,
    pub(crate) output_tokens: i64,
    pub(crate) llm_ms: i64,
    pub(crate) eval_ms: i64,
}

impl std::ops::Add for SummaryUsage {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            llm_calls: self.llm_calls + other.llm_calls,
            prompt_tokens: self.prompt_tokens + other.prompt_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
            llm_ms: self.llm_ms + other.llm_ms,
            eval_ms: self.eval_ms + other.eval_ms,
        }
    }
}

impl std::ops::AddAssign for SummaryUsage {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl std::iter::Sum for SummaryUsage {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), std::ops::Add::add)
    }
}

/// Generated text and the model usage it took, including retries.
#[derive(Debug)]
pub(crate) struct Generated {
    pub(crate) text: String,
    pub(crate) usage: SummaryUsage,
}

pub(crate) struct GeneratedSummary {
    pub(crate) summary: String,
    pub(crate) feed_ids: Vec<i64>,
    pub(crate) feed_item_ids: Vec<i64>,
    pub(crate) model: String,
    pub(crate) generation_ms: i64,
    pub(crate) usage: SummaryUsage,
}

#[derive(sqlx::FromRow)]
pub(crate) struct SummaryArticle {
    pub(crate) id: i64,
    pub(crate) feed_id: i64,
    pub(crate) title: String,
    pub(crate) feed: String,
    pub(crate) description: String,
}
