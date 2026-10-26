use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tracing::{Instrument, error, info};

use super::{
    ARTICLES_PER_BATCH, BATCH_OUTPUT_TOKENS, ContextBudget, DailySummary, FINAL_OUTPUT_TOKENS,
    GeneratedSummary, MergeRound, OllamaSummaryGenerator, PostgresSummaryRepository, SummaryError,
    SummaryGenerator, SummaryModels, SummaryRepository, SummaryStoreError, SummaryUsage,
    budgeted_prompts, build_batch_prompt, plan_merge_round,
};

// Bound the whole job below its lease, including all sequential Ollama calls.
// A day's articles require multiple sequential calls. The 12B model can take
// over two minutes per batch; a 14-minute job deadline cannot cover them all.
const GENERATION_TIMEOUT: Duration = Duration::from_secs(59 * 60);

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod orchestration_tests;

#[derive(Clone)]
pub(crate) struct SummaryState<R = PostgresSummaryRepository, G = OllamaSummaryGenerator> {
    repository: R,
    generator: G,
    models: SummaryModels,
}

impl SummaryState {
    pub(crate) fn new(
        pool: PgPool,
        http: reqwest::Client,
        ollama_url: String,
        models: SummaryModels,
    ) -> Self {
        Self {
            repository: PostgresSummaryRepository::new(pool),
            generator: OllamaSummaryGenerator::new(http, ollama_url),
            models,
        }
    }
}

impl<R: SummaryRepository, G> SummaryState<R, G> {
    pub(crate) async fn latest_summary(&self) -> Result<Option<DailySummary>, SummaryStoreError> {
        self.repository.latest_summary().await
    }
}

impl<R, G> SummaryState<R, G>
where
    R: SummaryRepository + Clone + 'static,
    G: SummaryGenerator + 'static,
{
    pub(crate) async fn refresh_scheduled(self) -> Result<DateTime<Utc>, SummaryStoreError> {
        let repository = self.repository.clone();
        self.start_refresh(false).await?;
        repository.next_due_at().await
    }

    /// Claims the refresh lease and spawns generation. `force` ignores the
    /// schedule but still respects an active lease. Returns whether it started.
    pub(crate) async fn start_refresh(self, force: bool) -> Result<bool, SummaryStoreError> {
        let state = self;
        let Some(token) = state.repository.claim_refresh(force).await? else {
            return Ok(false);
        };
        info!(batch_model = %state.models.batch, final_model = %state.models.final_summary, timeout_seconds = GENERATION_TIMEOUT.as_secs(), "daily news summary refresh started");
        tokio::spawn(
            async move {
                state.run_refresh(&token).await;
            }
            .in_current_span(),
        );
        Ok(true)
    }

    async fn run_refresh(&self, token: &str) {
        match tokio::time::timeout(GENERATION_TIMEOUT, generate_summary(self, token)).await {
            Ok(Ok(summary)) => {
                info!(summary_id = summary.id, "daily news summary refreshed");
                return;
            }
            Ok(Err(err)) => error!(error = ?err, "background summary refresh failed"),
            Err(err) => error!(%err, "background summary refresh timed out"),
        }
        if let Err(err) = self.repository.release_refresh(token).await {
            error!(error = ?err, "failed to release summary refresh lease");
        }
    }
}

async fn generate_summary<R: SummaryRepository, G: SummaryGenerator>(
    state: &SummaryState<R, G>,
    token: &str,
) -> Result<DailySummary, SummaryError> {
    let started = Instant::now();
    let mut usage = SummaryUsage::default();
    let articles = state.repository.fetch_articles().await?;

    if articles.is_empty() {
        return Err(SummaryError::NoArticles);
    }
    let batch_budget = state.generator.context_budget(&state.models.batch).await?;
    let final_budget = if state.models.batch == state.models.final_summary {
        batch_budget
    } else {
        state
            .generator
            .context_budget(&state.models.final_summary)
            .await?
    };
    let article_prompts = budgeted_prompts(
        &articles,
        build_batch_prompt,
        batch_budget,
        BATCH_OUTPUT_TOKENS,
        ARTICLES_PER_BATCH,
    )?;
    info!(
        articles = articles.len(),
        batches = article_prompts.len(),
        "summary generation plan"
    );
    let (mut notes, notes_usage) = generate_notes(state, batch_budget, article_prompts).await?;
    usage += notes_usage;
    let summary = loop {
        match plan_merge_round(&notes, batch_budget, final_budget)? {
            MergeRound::Final(prompt) => {
                info!(batches = notes.len(), "generating final news summary");
                let generated = state
                    .generator
                    .generate(
                        &state.models.final_summary,
                        final_budget,
                        prompt,
                        FINAL_OUTPUT_TOKENS,
                    )
                    .await?;
                usage += generated.usage;
                break generated.text;
            }
            MergeRound::Merge(prompts) => {
                let (merged, merge_usage) = generate_notes(state, batch_budget, prompts).await?;
                notes = merged;
                usage += merge_usage;
            }
        }
    };

    let feed_ids: Vec<i64> = articles
        .iter()
        .map(|article| article.feed_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let feed_item_ids: Vec<i64> = articles.iter().map(|article| article.id).collect();
    let generated = GeneratedSummary {
        summary,
        feed_ids,
        feed_item_ids,
        model: state.models.label(),
        generation_ms: started.elapsed().as_millis() as i64,
        usage,
    };
    let result = state.repository.complete_refresh(token, &generated).await?;
    let usage = &generated.usage;
    info!(
        generation_ms = result.generation_ms,
        llm_calls = usage.llm_calls,
        prompt_tokens = usage.prompt_tokens,
        output_tokens = usage.output_tokens,
        llm_ms = usage.llm_ms,
        eval_ms = usage.eval_ms,
        "summary generation usage"
    );
    Ok(result)
}

/// Generates one batch of story notes per prompt, in order, with their total usage.
async fn generate_notes<R, G: SummaryGenerator>(
    state: &SummaryState<R, G>,
    budget: ContextBudget,
    prompts: Vec<String>,
) -> Result<(Vec<String>, SummaryUsage), SummaryError> {
    let mut batches = Vec::with_capacity(prompts.len());
    for (index, prompt) in prompts.into_iter().enumerate() {
        info!(batch = index + 1, "generating summary notes batch");
        batches.push(
            state
                .generator
                .generate(&state.models.batch, budget, prompt, BATCH_OUTPUT_TOKENS)
                .await?,
        );
    }
    let usage = batches.iter().map(|batch| batch.usage).sum();
    let notes = batches.into_iter().map(|batch| batch.text).collect();
    Ok((notes, usage))
}
