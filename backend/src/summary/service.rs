use std::collections::{BTreeSet, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tracing::{Instrument, error, info, warn};

use super::{
    BATCH_OUTPUT_TOKENS, ContextBudget, DailySummary, FINAL_OUTPUT_TOKENS, GeneratedSummary,
    MergeRound, OllamaSummaryGenerator, PostgresSummaryRepository, Story, SummaryError,
    SummaryGenerator, SummaryModels, SummaryRepository, SummaryStoreError, SummaryUsage,
    article_prompts, plan_merge_round, select_stories,
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

    /// Ask Ollama to keep the summary models loaded this long after a request.
    pub(crate) fn with_keep_alive(self, keep_alive: Option<String>) -> Self {
        Self {
            generator: self.generator.with_keep_alive(keep_alive),
            ..self
        }
    }

    /// Group near-duplicate articles using this model's search embeddings.
    pub(crate) fn with_embedding_model(self, model: String) -> Self {
        Self {
            repository: self.repository.with_embedding_model(model),
            ..self
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
    /// schedule and regenerates even without new articles, but still respects
    /// an active lease. Returns whether it started.
    pub(crate) async fn start_refresh(self, force: bool) -> Result<bool, SummaryStoreError> {
        let Some(token) = self.repository.claim_refresh(force).await? else {
            return Ok(false);
        };
        info!(batch_model = %self.models.batch, final_model = %self.models.final_summary, timeout_seconds = GENERATION_TIMEOUT.as_secs(), "daily news summary refresh started");
        tokio::spawn(
            async move {
                self.run_refresh(&token, force).await;
            }
            .in_current_span(),
        );
        Ok(true)
    }

    async fn run_refresh(&self, token: &str, force: bool) {
        match tokio::time::timeout(GENERATION_TIMEOUT, generate_summary(self, token, force)).await {
            Ok(Ok(Refresh::Generated(summary))) => {
                info!(summary_id = summary.id, "daily news summary refreshed");
                return;
            }
            Ok(Ok(Refresh::Unchanged)) => {
                info!("no new articles since the latest summary; refresh skipped");
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

#[derive(Debug)]
enum Refresh {
    Generated(Box<DailySummary>),
    /// The latest summary already covers every article.
    Unchanged,
}

/// Whether the stories include articles the latest summary, made with the
/// same models, did not cover. Articles that aged out do not count.
fn has_new_articles(latest: Option<&DailySummary>, stories: &[Story], model: &str) -> bool {
    let Some(latest) = latest.filter(|latest| latest.model == model) else {
        return true;
    };
    let covered: HashSet<i64> = latest.feed_item_ids.iter().copied().collect();
    stories
        .iter()
        .flat_map(|story| &story.item_ids)
        .any(|id| !covered.contains(id))
}

async fn generate_summary<R: SummaryRepository, G: SummaryGenerator>(
    state: &SummaryState<R, G>,
    token: &str,
    force: bool,
) -> Result<Refresh, SummaryError> {
    let started = Instant::now();
    let articles = state.repository.fetch_articles().await?;
    if articles.is_empty() {
        return Err(SummaryError::NoArticles);
    }
    let article_count = articles.len();
    let stories = select_stories(articles);
    let model = state.models.label();
    if !force {
        let latest = state.repository.latest_summary().await?;
        if !has_new_articles(latest.as_ref(), &stories, &model) {
            state.repository.skip_refresh(token).await?;
            return Ok(Refresh::Unchanged);
        }
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
    let article_prompts = article_prompts(&stories, batch_budget)?;
    info!(
        articles = article_count,
        stories = stories.len(),
        batches = article_prompts.len(),
        "summary generation plan"
    );
    let (mut notes, mut usage) = generate_notes(state, batch_budget, article_prompts).await?;
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

    let feed_ids: Vec<i64> = stories
        .iter()
        .flat_map(|story| story.feed_ids.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut feed_item_ids: Vec<i64> = stories
        .iter()
        .flat_map(|story| story.item_ids.iter().copied())
        .collect();
    feed_item_ids.sort_unstable();
    let generated = GeneratedSummary {
        summary,
        feed_ids,
        feed_item_ids,
        models: state.models.clone(),
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
    Ok(Refresh::Generated(Box::new(result)))
}

/// Story notes for each prompt, in order, with the usage of generating them.
/// Notes are reused from the cache when the same model saw the same prompt;
/// the cache is an optimization, so its failures are logged, not fatal.
async fn generate_notes<R: SummaryRepository, G: SummaryGenerator>(
    state: &SummaryState<R, G>,
    budget: ContextBudget,
    prompts: Vec<String>,
) -> Result<(Vec<String>, SummaryUsage), SummaryError> {
    let model = &state.models.batch;
    let cached = state
        .repository
        .cached_notes(model, &prompts)
        .await
        .unwrap_or_else(|err| {
            warn!(error = ?err, "summary note cache unavailable");
            vec![None; prompts.len()]
        });
    info!(
        batches = prompts.len(),
        cached = cached.iter().flatten().count(),
        "summary notes plan"
    );
    let mut usage = SummaryUsage::default();
    let mut notes = Vec::with_capacity(prompts.len());
    for (index, (prompt, cached)) in prompts.into_iter().zip(cached).enumerate() {
        if let Some(text) = cached {
            notes.push(text);
            continue;
        }
        info!(batch = index + 1, "generating summary notes batch");
        let generated = state
            .generator
            .generate(model, budget, prompt.clone(), BATCH_OUTPUT_TOKENS)
            .await?;
        if let Err(err) = state
            .repository
            .store_notes(model, &prompt, &generated.text)
            .await
        {
            warn!(error = ?err, "failed to cache summary notes");
        }
        usage += generated.usage;
        notes.push(generated.text);
    }
    Ok((notes, usage))
}
