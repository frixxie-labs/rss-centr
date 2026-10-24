use std::time::{Duration, Instant};

use axum::{Json, extract::State, http::StatusCode};
use rss_centr_core::summary_refresh::SummaryRefreshResponse;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{Instrument, error, info, instrument, warn};

use crate::summary::{
    ARTICLES_PER_BATCH, BATCH_OUTPUT_TOKENS, ContextBudget, FINAL_OUTPUT_TOKENS, GeneratedSummary,
    PostgresSummaryRepository, SUMMARY_CONTEXT_TOKENS, SUMMARY_SYSTEM_PROMPT, SummaryRepository,
    SummaryUsage, budgeted_prompts, build_batch_prompt, build_final_prompt, build_merge_prompt,
};

use super::error::HandlerError;

pub use crate::summary::DailySummary;

const DEFAULT_MODEL: &str = "gemma4:e4b";
const OLLAMA_REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);
// Bound the whole job below its lease, including all sequential Ollama calls.
// A day's articles require multiple sequential calls. The 12B model can take
// over two minutes per batch; a 14-minute job deadline cannot cover them all.
const GENERATION_TIMEOUT: Duration = Duration::from_secs(59 * 60);

#[derive(Clone)]
pub struct SummaryState {
    repository: PostgresSummaryRepository,
    http: reqwest::Client,
    ollama_url: String,
    models: SummaryModels,
}

#[derive(Clone)]
struct SummaryModels {
    batch: String,
    final_summary: String,
}

impl SummaryModels {
    fn from_overrides(
        legacy: Option<String>,
        batch: Option<String>,
        final_summary: Option<String>,
    ) -> Self {
        Self {
            batch: batch
                .or_else(|| legacy.clone())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            final_summary: final_summary
                .or(legacy)
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        }
    }

    fn label(&self) -> String {
        if self.batch == self.final_summary {
            self.final_summary.clone()
        } else {
            format!("{} → {}", self.batch, self.final_summary)
        }
    }
}

impl SummaryState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            repository: PostgresSummaryRepository::new(pool),
            http: reqwest::Client::new(),
            ollama_url: std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://desktop:11434".to_string()),
            models: SummaryModels::from_overrides(
                std::env::var("OLLAMA_MODEL").ok(),
                std::env::var("OLLAMA_BATCH_MODEL").ok(),
                std::env::var("OLLAMA_FINAL_MODEL").ok(),
            ),
        }
    }
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    system: &'static str,
    prompt: &'a str,
    stream: bool,
    think: bool,
    options: GenerateOptions,
}

#[derive(Serialize)]
struct GenerateOptions {
    num_ctx: u32,
    num_predict: u32,
    temperature: f32,
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
    prompt_eval_count: u32,
    eval_count: u32,
    done_reason: String,
    #[serde(default)]
    total_duration: u64,
    #[serde(default)]
    eval_duration: u64,
}

#[derive(Deserialize)]
struct ShowResponse {
    model_info: serde_json::Map<String, serde_json::Value>,
}

async fn fetch_context_budget(
    state: &SummaryState,
    model: &str,
) -> Result<ContextBudget, HandlerError> {
    let response = state
        .http
        .post(format!(
            "{}/api/show",
            state.ollama_url.trim_end_matches('/')
        ))
        .timeout(Duration::from_secs(30))
        .json(&serde_json::json!({"model": model}))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| {
            error!(model, error = ?err, "failed to fetch Ollama model metadata");
            HandlerError::new(
                StatusCode::BAD_GATEWAY,
                "Summary model unavailable".to_string(),
            )
        })?;
    let metadata: ShowResponse = response.json().await.map_err(|err| {
        error!(%err, "invalid Ollama model metadata");
        HandlerError::new(
            StatusCode::BAD_GATEWAY,
            "Invalid summary model metadata".to_string(),
        )
    })?;
    let model_context = metadata
        .model_info
        .iter()
        .filter(|(key, _)| key.ends_with(".context_length"))
        .filter_map(|(_, value)| value.as_u64())
        .min()
        .filter(|limit| *limit > 0)
        .ok_or_else(|| {
            HandlerError::new(
                StatusCode::BAD_GATEWAY,
                "Summary model context limit missing".to_string(),
            )
        })?;
    let num_ctx = model_context.min(u64::from(SUMMARY_CONTEXT_TOKENS)) as u32;
    info!(
        model,
        model_context, num_ctx, "summary model context budget"
    );
    Ok(ContextBudget { num_ctx })
}

async fn generate(
    state: &SummaryState,
    model: &str,
    budget: ContextBudget,
    prompt: String,
    num_predict: u32,
    usage: &mut SummaryUsage,
) -> Result<String, HandlerError> {
    if !budget.fits(&prompt, num_predict) {
        return Err(HandlerError::internal(
            "Summary input exceeds model context budget",
        ));
    }
    for (attempt, allowance) in [num_predict, num_predict.saturating_mul(2)]
        .into_iter()
        .enumerate()
    {
        info!(
            model,
            estimated_input_tokens = budget.estimated_input_tokens(&prompt),
            num_ctx = budget.num_ctx,
            num_predict = allowance,
            attempt = attempt + 1,
            "generating summary within context budget"
        );
        let request = GenerateRequest {
            model,
            system: SUMMARY_SYSTEM_PROMPT,
            prompt: &prompt,
            stream: false,
            think: false,
            options: GenerateOptions {
                num_ctx: budget.num_ctx,
                num_predict: allowance,
                temperature: 0.0,
            },
        };
        let response = state
        .http
        .post(format!(
            "{}/api/generate",
            state.ollama_url.trim_end_matches('/')
        ))
        .timeout(OLLAMA_REQUEST_TIMEOUT)
        .json(&request)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| {
            error!(model, error = ?err, timeout = err.is_timeout(), timeout_seconds = OLLAMA_REQUEST_TIMEOUT.as_secs(), "Ollama request failed");
            HandlerError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "Summary service unavailable".to_string(),
            )
        })?;
        let generated: GenerateResponse = response.json().await.map_err(|err| {
            error!(error = ?err, timeout = err.is_timeout(), "invalid Ollama response");
            HandlerError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "Invalid summary response".to_string(),
            )
        })?;
        let duration_ms = (generated.total_duration / 1_000_000) as i64;
        let eval_ms = (generated.eval_duration / 1_000_000) as i64;
        usage.llm_calls += 1;
        usage.prompt_tokens += i64::from(generated.prompt_eval_count);
        usage.output_tokens += i64::from(generated.eval_count);
        usage.llm_ms += duration_ms;
        usage.eval_ms += eval_ms;
        info!(model, prompt_tokens = generated.prompt_eval_count,
        output_tokens = generated.eval_count, num_ctx = budget.num_ctx,
        duration_ms, eval_ms, done_reason = %generated.done_reason, "summary token usage");
        if generated
            .prompt_eval_count
            .saturating_add(generated.eval_count)
            > budget.num_ctx
            || generated.prompt_eval_count.saturating_add(allowance) > budget.num_ctx
        {
            error!(
                model,
                prompt_tokens = generated.prompt_eval_count,
                output_tokens = generated.eval_count,
                num_predict = allowance,
                num_ctx = budget.num_ctx,
                "summary generation exceeded context budget"
            );
            return Err(HandlerError::new(
                StatusCode::BAD_GATEWAY,
                "Summary generation exceeded context budget".to_string(),
            ));
        }
        if generated.done_reason == "length" {
            let retry_allowance = num_predict.saturating_mul(2);
            let retry = attempt == 0
                && budget.fits(&prompt, retry_allowance)
                && generated.prompt_eval_count.saturating_add(retry_allowance) <= budget.num_ctx;
            warn!(
                model,
                output_tokens = generated.eval_count,
                num_predict = allowance,
                retry,
                retry_num_predict = retry_allowance,
                "summary generation truncated"
            );
            if retry {
                continue;
            }
            return Err(HandlerError::new(
                StatusCode::BAD_GATEWAY,
                "Summary generation truncated".to_string(),
            ));
        }
        if generated.done_reason != "stop" {
            error!(model, done_reason = %generated.done_reason, "summary generation incomplete");
            return Err(HandlerError::new(
                StatusCode::BAD_GATEWAY,
                "Summary generation incomplete".to_string(),
            ));
        }
        if generated.response.trim().is_empty() {
            return Err(HandlerError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "Empty summary response".to_string(),
            ));
        }
        return Ok(generated.response.trim().to_string());
    }
    Err(HandlerError::new(
        StatusCode::BAD_GATEWAY,
        "Summary generation truncated".to_string(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/items/summary",
    tag = "items",
    responses(
        (status = 200, description = "AI overview of items collected in the last 24 hours", body = DailySummary),
        (status = 404, description = "No summary is available yet"),
        (status = 500, description = "Database error"),
    )
)]
#[instrument(skip(state))]
pub async fn fetch_daily_summary(
    State(state): State<SummaryState>,
) -> Result<Json<DailySummary>, HandlerError> {
    state
        .repository
        .latest_summary()
        .await?
        .map(Json)
        .ok_or_else(|| HandlerError::not_found("No summary is available yet."))
}

#[utoipa::path(
    post,
    path = "/internal/items/summary/refresh",
    tag = "items",
    responses(
        (status = 202, description = "Summary refresh accepted, already in progress, or not due yet", body = SummaryRefreshResponse),
        (status = 500, description = "Summary service unavailable"),
    )
)]
#[instrument(skip(state))]
pub async fn refresh_daily_summary(
    State(state): State<SummaryState>,
) -> Result<(StatusCode, Json<SummaryRefreshResponse>), HandlerError> {
    let repository = state.repository.clone();
    start_refresh(state, false).await?;
    let next_due_at = repository.next_due_at().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SummaryRefreshResponse { next_due_at }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/items/summary/generate",
    tag = "items",
    responses(
        (status = 202, description = "Summary generation started"),
        (status = 409, description = "Summary generation already in progress"),
        (status = 500, description = "Summary service unavailable"),
    )
)]
#[instrument(skip(state))]
pub async fn generate_daily_summary(
    State(state): State<SummaryState>,
) -> Result<StatusCode, HandlerError> {
    if start_refresh(state, true).await? {
        Ok(StatusCode::ACCEPTED)
    } else {
        Ok(StatusCode::CONFLICT)
    }
}

/// Claims the refresh lease and spawns generation. `force` ignores the
/// schedule but still respects an active lease. Returns whether it started.
async fn start_refresh(state: SummaryState, force: bool) -> Result<bool, HandlerError> {
    let Some(token) = state.repository.claim_refresh(force).await? else {
        return Ok(false);
    };
    info!(batch_model = %state.models.batch, final_model = %state.models.final_summary, timeout_seconds = GENERATION_TIMEOUT.as_secs(), "daily news summary refresh started");
    tokio::spawn(
        async move {
            match tokio::time::timeout(GENERATION_TIMEOUT, generate_summary(&state, &token)).await {
                Ok(Ok(summary)) => {
                    info!(summary_id = summary.id, "daily news summary refreshed");
                    return;
                }
                Ok(Err(err)) => error!(%err, "background summary refresh failed"),
                Err(err) => error!(%err, "background summary refresh timed out"),
            }
            if let Err(err) = state.repository.release_refresh(&token).await {
                error!(error = ?err, "failed to release summary refresh lease");
            }
        }
        .in_current_span(),
    );
    Ok(true)
}

async fn generate_summary(state: &SummaryState, token: &str) -> Result<DailySummary, HandlerError> {
    let started = Instant::now();
    let mut usage = SummaryUsage::default();
    let articles = state.repository.fetch_articles().await?;

    if articles.is_empty() {
        return Err(HandlerError::not_found(
            "No recent articles available for summary generation.",
        ));
    }
    let batch_budget = fetch_context_budget(state, &state.models.batch).await?;
    let final_budget = if state.models.batch == state.models.final_summary {
        batch_budget
    } else {
        fetch_context_budget(state, &state.models.final_summary).await?
    };
    let summary = {
        let mut batch_summaries = Vec::new();
        let prompts = budgeted_prompts(
            &articles,
            build_batch_prompt,
            batch_budget,
            BATCH_OUTPUT_TOKENS,
            ARTICLES_PER_BATCH,
        )?;
        info!(
            articles = articles.len(),
            batches = prompts.len(),
            "summary generation plan"
        );
        for (index, prompt) in prompts.into_iter().enumerate() {
            info!(batch = index + 1, "generating summary article batch");
            batch_summaries.push(
                generate(
                    state,
                    &state.models.batch,
                    batch_budget,
                    prompt,
                    BATCH_OUTPUT_TOKENS,
                    &mut usage,
                )
                .await?,
            );
        }
        while batch_summaries.len() > 16
            || !final_budget.fits(&build_final_prompt(&batch_summaries), FINAL_OUTPUT_TOKENS)
        {
            let mut merged = Vec::new();
            let prompts = budgeted_prompts(
                &batch_summaries,
                build_merge_prompt,
                batch_budget,
                BATCH_OUTPUT_TOKENS,
                8,
            )?;
            if prompts.len() >= batch_summaries.len() {
                return Err(HandlerError::internal(
                    "Summary context budget too small to merge summaries",
                ));
            }
            for prompt in prompts {
                merged.push(
                    generate(
                        state,
                        &state.models.batch,
                        batch_budget,
                        prompt,
                        BATCH_OUTPUT_TOKENS,
                        &mut usage,
                    )
                    .await?,
                );
            }
            batch_summaries = merged;
        }
        info!(
            batches = batch_summaries.len(),
            "generating final news summary"
        );
        generate(
            state,
            &state.models.final_summary,
            final_budget,
            build_final_prompt(&batch_summaries),
            FINAL_OUTPUT_TOKENS,
            &mut usage,
        )
        .await?
    };

    let mut feed_ids: Vec<i64> = articles.iter().map(|article| article.feed_id).collect();
    feed_ids.sort_unstable();
    feed_ids.dedup();
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

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use chrono::{DateTime, Utc};
    use tokio::sync::Semaphore;

    use crate::summary::LEASE_SECONDS;

    use super::*;

    #[test]
    fn model_selection_defaults_to_gemma_for_both_stages_and_preserves_overrides() {
        for (legacy, batch, final_summary, expected_batch, expected_final) in [
            (None, None, None, "gemma4:e4b", "gemma4:e4b"),
            (Some("single"), None, None, "single", "single"),
            (None, Some("fast"), None, "fast", "gemma4:e4b"),
            (None, None, Some("quality"), "gemma4:e4b", "quality"),
            (Some("single"), Some("fast"), None, "fast", "single"),
            (Some("single"), None, Some("quality"), "single", "quality"),
            (
                Some("single"),
                Some("fast"),
                Some("quality"),
                "fast",
                "quality",
            ),
        ] {
            let models = SummaryModels::from_overrides(
                legacy.map(str::to_string),
                batch.map(str::to_string),
                final_summary.map(str::to_string),
            );
            assert_eq!(models.batch, expected_batch);
            assert_eq!(models.final_summary, expected_final);
            assert_eq!(
                models.label(),
                if expected_batch == expected_final {
                    expected_final.to_string()
                } else {
                    format!("{expected_batch} → {expected_final}")
                }
            );
        }
    }

    #[test]
    fn generation_deadlines_allow_slow_batches_and_finish_before_the_lease() {
        assert!(OLLAMA_REQUEST_TIMEOUT > Duration::from_secs(120));
        // The observed workload is 312 articles: 13 batches and a final merge.
        let calls = 312_usize.div_ceil(ARTICLES_PER_BATCH) + 1;
        assert!(GENERATION_TIMEOUT > Duration::from_secs(120) * calls as u32);
        assert!(GENERATION_TIMEOUT.as_secs() < LEASE_SECONDS as u64);
    }

    async fn mock_ollama(
        metadata: serde_json::Value,
        generated: serde_json::Value,
        num_predict: u32,
    ) -> (SummaryState, tokio::task::JoinHandle<()>) {
        mock_ollama_responses(
            metadata,
            vec![
                (generated.clone(), num_predict),
                (generated, num_predict * 2),
            ],
        )
        .await
    }

    async fn mock_ollama_responses(
        metadata: serde_json::Value,
        generated: Vec<(serde_json::Value, u32)>,
    ) -> (SummaryState, tokio::task::JoinHandle<()>) {
        let responses = Arc::new(Mutex::new(VecDeque::from(generated)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route(
                "/api/show",
                axum::routing::post(move |Json(request): Json<serde_json::Value>| {
                    let metadata = metadata.clone();
                    async move {
                        assert_eq!(request["model"], "test-model");
                        Json(metadata)
                    }
                }),
            )
            .route(
                "/api/generate",
                axum::routing::post(move |Json(request): Json<serde_json::Value>| {
                    let (generated, num_predict) = responses
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("unexpected generation attempt");
                    async move {
                        assert_eq!(request["model"], "test-model");
                        assert_eq!(request["options"]["num_ctx"], SUMMARY_CONTEXT_TOKENS);
                        assert_eq!(request["options"]["num_predict"], num_predict);
                        assert_eq!(request["think"], false);
                        Json(generated)
                    }
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/rss_centr")
            .unwrap();
        let mut state = SummaryState::new(pool);
        state.ollama_url = format!("http://{address}");
        (state, server)
    }

    #[tokio::test]
    async fn model_metadata_caps_context_and_requires_a_limit() {
        for (metadata, expected) in [
            (
                serde_json::json!({"model_info": {"gemma4.context_length": 262144}}),
                Some(16384),
            ),
            (
                serde_json::json!({"model_info": {"gemma4.context_length": 32768}}),
                Some(16384),
            ),
            (
                serde_json::json!({"model_info": {"gemma4.context_length": 4096}}),
                Some(4096),
            ),
            (serde_json::json!({"model_info": {}}), None),
            (
                serde_json::json!({"model_info": {"gemma4.context_length": 0}}),
                None,
            ),
        ] {
            let (state, server) = mock_ollama(metadata, serde_json::json!({}), 700).await;
            let result = fetch_context_budget(&state, "test-model").await;
            assert_eq!(result.ok().map(|budget| budget.num_ctx), expected);
            server.abort();
        }
    }

    #[tokio::test]
    async fn generation_checks_actual_usage_and_rejects_incomplete_responses() {
        for (response, success) in [
            (
                serde_json::json!({"response": " News ", "prompt_eval_count": 100, "eval_count": 20, "done_reason": "stop"}),
                true,
            ),
            (
                serde_json::json!({"response": "News", "prompt_eval_count": 100, "eval_count": 700, "done_reason": "length"}),
                false,
            ),
            (
                serde_json::json!({"response": "News", "prompt_eval_count": 15685, "eval_count": 20, "done_reason": "stop"}),
                false,
            ),
            (
                serde_json::json!({"response": "News", "prompt_eval_count": 15616, "eval_count": 800, "done_reason": "stop"}),
                false,
            ),
            (
                serde_json::json!({"response": "News", "prompt_eval_count": 15684, "eval_count": 700, "done_reason": "stop"}),
                true,
            ),
            (
                serde_json::json!({"response": " ", "prompt_eval_count": 100, "eval_count": 20, "done_reason": "stop"}),
                false,
            ),
            (serde_json::json!({"response": "News"}), false),
        ] {
            let (state, server) = mock_ollama(serde_json::json!({}), response, 700).await;
            let result = generate(
                &state,
                "test-model",
                ContextBudget {
                    num_ctx: SUMMARY_CONTEXT_TOKENS,
                },
                "News".to_string(),
                700,
                &mut SummaryUsage::default(),
            )
            .await;
            assert_eq!(result.is_ok(), success);
            if success {
                assert_eq!(result.unwrap(), "News");
            }
            // Rejected before making any request, independently of Ollama's response.
            assert!(
                generate(
                    &state,
                    "test-model",
                    ContextBudget {
                        num_ctx: SUMMARY_CONTEXT_TOKENS,
                    },
                    "oversized ".repeat(SUMMARY_CONTEXT_TOKENS as usize),
                    700,
                    &mut SummaryUsage::default()
                )
                .await
                .is_err()
            );
            server.abort();
        }
    }

    #[tokio::test]
    async fn truncated_generation_retries_once_with_more_headroom() {
        for num_predict in [BATCH_OUTPUT_TOKENS, FINAL_OUTPUT_TOKENS] {
            let (state, server) = mock_ollama_responses(
                serde_json::json!({}),
                vec![
                    (
                        serde_json::json!({
                            "response": "Partial notes", "prompt_eval_count": 2169,
                            "eval_count": num_predict, "done_reason": "length",
                        }),
                        num_predict,
                    ),
                    (
                        serde_json::json!({
                            "response": " Complete notes (Source). ", "prompt_eval_count": 2169,
                            "eval_count": num_predict + 100, "done_reason": "stop",
                        }),
                        num_predict * 2,
                    ),
                ],
            )
            .await;
            let result = generate(
                &state,
                "test-model",
                ContextBudget {
                    num_ctx: SUMMARY_CONTEXT_TOKENS,
                },
                "News".to_string(),
                num_predict,
                &mut SummaryUsage::default(),
            )
            .await
            .unwrap();
            assert_eq!(result, "Complete notes (Source).");
            server.abort();
        }
    }

    #[tokio::test]
    async fn repeated_truncation_rejects_partial_output() {
        let num_predict = BATCH_OUTPUT_TOKENS;
        let (state, server) = mock_ollama_responses(
            serde_json::json!({}),
            [num_predict, num_predict * 2]
                .into_iter()
                .map(|allowance| {
                    (
                        serde_json::json!({
                            "response": "Partial notes", "prompt_eval_count": 2169,
                            "eval_count": allowance, "done_reason": "length",
                        }),
                        allowance,
                    )
                })
                .collect(),
        )
        .await;
        let err = generate(
            &state,
            "test-model",
            ContextBudget {
                num_ctx: SUMMARY_CONTEXT_TOKENS,
            },
            "News".to_string(),
            num_predict,
            &mut SummaryUsage::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.message, "Summary generation truncated");
        server.abort();
    }

    #[tokio::test]
    async fn truncation_retry_requires_estimated_and_actual_context_headroom() {
        let num_predict = BATCH_OUTPUT_TOKENS;
        for (prompt, prompt_tokens) in [
            ("x".repeat(38000), 100),
            ("News".to_string(), SUMMARY_CONTEXT_TOKENS - num_predict),
        ] {
            let (state, server) = mock_ollama_responses(
                serde_json::json!({}),
                vec![(
                    serde_json::json!({
                        "response": "Partial notes", "prompt_eval_count": prompt_tokens,
                        "eval_count": num_predict, "done_reason": "length",
                    }),
                    num_predict,
                )],
            )
            .await;
            let err = generate(
                &state,
                "test-model",
                ContextBudget {
                    num_ctx: SUMMARY_CONTEXT_TOKENS,
                },
                prompt,
                num_predict,
                &mut SummaryUsage::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(err.message, "Summary generation truncated");
            server.abort();
        }
    }

    #[tokio::test]
    async fn context_overflow_and_other_incomplete_responses_do_not_retry() {
        for (prompt_tokens, done_reason, message) in [
            (
                32000,
                "length",
                "Summary generation exceeded context budget",
            ),
            (100, "unexpected", "Summary generation incomplete"),
        ] {
            let (state, server) = mock_ollama_responses(
                serde_json::json!({}),
                vec![(
                    serde_json::json!({
                        "response": "Partial notes", "prompt_eval_count": prompt_tokens,
                        "eval_count": BATCH_OUTPUT_TOKENS, "done_reason": done_reason,
                    }),
                    BATCH_OUTPUT_TOKENS,
                )],
            )
            .await;
            let err = generate(
                &state,
                "test-model",
                ContextBudget {
                    num_ctx: SUMMARY_CONTEXT_TOKENS,
                },
                "News".to_string(),
                BATCH_OUTPUT_TOKENS,
                &mut SummaryUsage::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(err.message, message);
            server.abort();
        }
    }

    #[tokio::test]
    async fn longer_summaries_have_headroom_but_truncated_output_still_fails() {
        for num_predict in [BATCH_OUTPUT_TOKENS, FINAL_OUTPUT_TOKENS] {
            for done_reason in ["stop", "length"] {
                let (state, server) = mock_ollama(
                    serde_json::json!({}),
                    serde_json::json!({
                        "response": "Complete news overview (Source).",
                        "prompt_eval_count": 2653,
                        "eval_count": 800,
                        "done_reason": done_reason,
                    }),
                    num_predict,
                )
                .await;
                let result = generate(
                    &state,
                    "test-model",
                    ContextBudget {
                        num_ctx: SUMMARY_CONTEXT_TOKENS,
                    },
                    "News".to_string(),
                    num_predict,
                    &mut SummaryUsage::default(),
                )
                .await;
                assert_eq!(result.is_ok(), done_reason == "stop");
                server.abort();
            }
        }
    }

    async fn wait_for_refresh(pool: &PgPool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let released: bool = sqlx::query_scalar(
                    "SELECT lease_token IS NULL FROM summary_refresh_queue WHERE id = TRUE",
                )
                .fetch_one(pool)
                .await
                .unwrap();
                if released {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("background refresh should release its lease");
    }

    #[sqlx::test]
    async fn worker_refresh_generates_and_persists_source_ids(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let responses = Arc::new(Semaphore::new(0));
        let response_gate = responses.clone();
        let app = axum::Router::new().route(
            "/api/show",
            axum::routing::post(|Json(request): Json<serde_json::Value>| async move {
                let context = match request["model"].as_str().unwrap() {
                    "batch-model" => 4096,
                    "final-model" => 262144,
                    model => panic!("unexpected model metadata request: {model}"),
                };
                Json(serde_json::json!({"model_info": {"gemma4.context_length": context}}))
            }),
        ).route(
            "/api/generate",
            axum::routing::post(move |Json(request): Json<serde_json::Value>| {
                let response_gate = response_gate.clone();
                async move {
                    let prompt = request["prompt"].as_str().unwrap();
                    let expected_output = if prompt.starts_with("Trekk ut saksnotater") {
                        assert!(prompt.contains("Tema: ... | Hendelse: ... | Nøkkelfakta: ... | Usikkerhet: ... | Kilder: ..."));
                        assert_eq!(request["model"], "batch-model");
                        assert_eq!(request["options"]["num_ctx"], 4096);
                        BATCH_OUTPUT_TOKENS
                    } else {
                        assert!(prompt.starts_with("Skriv en samlet nyhetsoversikt"));
                        assert!(prompt.contains("250–350 ord"));
                        assert_eq!(request["model"], "final-model");
                        assert_eq!(request["options"]["num_ctx"], SUMMARY_CONTEXT_TOKENS);
                        FINAL_OUTPUT_TOKENS
                    };
                    assert_eq!(request["options"]["num_predict"], expected_output);
                    response_gate.acquire().await.unwrap().forget();
                    Json(serde_json::json!({"response": "News overview (Example).", "prompt_eval_count": 100, "eval_count": 20, "done_reason": "stop"}))
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let feed_id: i64 = sqlx::query_scalar(
            "INSERT INTO feeds (url, title) VALUES ('https://example.com/rss', 'Example') RETURNING id",
        ).fetch_one(&pool).await.unwrap();
        sqlx::query(
            "WITH items AS (
                INSERT INTO feed_items (feed_id, external_id, title, url)
                SELECT $1, n::text, 'News', 'https://example.com/news' FROM generate_series(1, 2) n
                RETURNING id
             ) INSERT INTO feed_item_details (feed_item_id, summary, content, author, published_at)
               SELECT id, 'Description', '', '', NOW() FROM items",
        )
        .bind(feed_id)
        .execute(&pool)
        .await
        .unwrap();
        let mut state = SummaryState::new(pool.clone());
        let expected_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM feed_items WHERE feed_id = $1 ORDER BY inserted_at DESC, id DESC",
        )
        .bind(feed_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        state.ollama_url = format!("http://{address}");
        state.models = SummaryModels {
            batch: "batch-model".to_string(),
            final_summary: "final-model".to_string(),
        };
        // Independent state and pool represent separate backend instances.
        let other_pool = sqlx::postgres::PgPoolOptions::new()
            .connect_with((*pool.connect_options()).clone())
            .await
            .unwrap();
        let mut other_state = SummaryState::new(other_pool.clone());
        other_state.ollama_url = state.ollama_url.clone();
        other_state.models = state.models.clone();
        for instance in [state.clone(), other_state.clone()] {
            let (status, _) = tokio::time::timeout(
                Duration::from_secs(1),
                refresh_daily_summary(State(instance)),
            )
            .await
            .expect("refresh should return without waiting for generation")
            .unwrap();
            assert_eq!(status, StatusCode::ACCEPTED);
        }
        assert!(repository.latest_summary().await.unwrap().is_none());
        responses.add_permits(2);
        wait_for_refresh(&pool).await;
        // A staggered worker must not generate again immediately after completion.
        let _ = refresh_daily_summary(State(other_state)).await.unwrap();
        assert!(repository.claim_refresh(false).await.unwrap().is_none());
        let stored = fetch_daily_summary(State(SummaryState::new(pool.clone())))
            .await
            .unwrap()
            .0;
        assert_eq!(stored.feed_ids, vec![feed_id]);
        assert_eq!(stored.feed_item_ids, expected_ids);
        assert_eq!(stored.model, "batch-model → final-model");
        assert_eq!(stored.summary, "News overview (Example).");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ai_summaries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        server.abort();
        other_pool.close().await;
    }

    #[sqlx::test]
    async fn model_budgets_control_merging_and_shared_models_reuse_metadata(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let feed_id: i64 = sqlx::query_scalar(
            "INSERT INTO feeds (url, title) VALUES ('https://example.com/rss', 'Example') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "WITH items AS (
                INSERT INTO feed_items (feed_id, external_id, title, url)
                SELECT $1, n::text, 'News', 'https://example.com/news' FROM generate_series(1, 26) n
                RETURNING id
             ) INSERT INTO feed_item_details (feed_item_id, summary, content, author, published_at)
               SELECT id, 'Description', '', '', NOW() FROM items",
        )
        .bind(feed_id)
        .execute(&pool)
        .await
        .unwrap();

        for shared_model in [false, true] {
            let final_model = if shared_model {
                "batch-model"
            } else {
                "final-model"
            };
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            let metadata_requests = requests.clone();
            let generation_requests = requests.clone();
            let app = axum::Router::new()
                .route("/api/show", axum::routing::post(move |Json(request): Json<serde_json::Value>| {
                    let requests = metadata_requests.clone();
                    async move {
                        let model = request["model"].as_str().unwrap();
                        requests.lock().unwrap().push(format!("show:{model}"));
                        let num_ctx = if model == "batch-model" { 8192 } else {
                            assert_eq!(model, "final-model");
                            4096
                        };
                        Json(serde_json::json!({"model_info": {"test.context_length": num_ctx}}))
                    }
                }))
                .route("/api/generate", axum::routing::post(move |Json(request): Json<serde_json::Value>| {
                    let requests = generation_requests.clone();
                    async move {
                        let prompt = request["prompt"].as_str().unwrap();
                        let model = request["model"].as_str().unwrap();
                        let (stage, response) = if prompt.starts_with("Trekk ut saksnotater") {
                            ("batch", "x".repeat(4200))
                        } else if prompt.starts_with("Slå sammen saksnotatene") {
                            assert!(!shared_model);
                            assert_eq!(prompt.matches(&"x".repeat(4200)).count(), 2);
                            ("merge", "Merged fact (Example).".to_string())
                        } else {
                            assert!(prompt.starts_with("Skriv en samlet nyhetsoversikt"));
                            if !shared_model {
                                assert!(prompt.contains("Merged fact (Example)."));
                            }
                            ("final", "Final overview (Example).".to_string())
                        };
                        let (expected_model, expected_context, expected_output) = if stage == "final" {
                            (final_model, if shared_model { 8192 } else { 4096 }, FINAL_OUTPUT_TOKENS)
                        } else {
                            ("batch-model", 8192, BATCH_OUTPUT_TOKENS)
                        };
                        assert_eq!(model, expected_model);
                        assert_eq!(request["options"]["num_ctx"], expected_context);
                        assert_eq!(request["options"]["num_predict"], expected_output);
                        assert!(ContextBudget { num_ctx: expected_context }.fits(prompt, expected_output));
                        requests.lock().unwrap().push(format!("{stage}:{model}"));
                        Json(serde_json::json!({"response": response, "prompt_eval_count": 100, "eval_count": 500, "done_reason": "stop"}))
                    }
                }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let mut state = SummaryState::new(pool.clone());
            state.ollama_url = format!("http://{address}");
            state.models = SummaryModels {
                batch: "batch-model".to_string(),
                final_summary: final_model.to_string(),
            };
            sqlx::query("UPDATE summary_refresh_queue SET due_at = NOW()")
                .execute(&pool)
                .await
                .unwrap();
            let token = repository.claim_refresh(false).await.unwrap().unwrap();
            let summary = generate_summary(&state, &token).await.unwrap();
            assert_eq!(summary.summary, "Final overview (Example).");
            assert_eq!(summary.feed_item_ids.len(), 26);
            assert_eq!(summary.model, state.models.label());
            let expected = if shared_model {
                vec![
                    "show:batch-model",
                    "batch:batch-model",
                    "batch:batch-model",
                    "final:batch-model",
                ]
            } else {
                vec![
                    "show:batch-model",
                    "show:final-model",
                    "batch:batch-model",
                    "batch:batch-model",
                    "merge:batch-model",
                    "final:final-model",
                ]
            };
            assert_eq!(*requests.lock().unwrap(), expected);
            server.abort();
        }
    }

    #[sqlx::test]
    async fn failed_background_refresh_releases_lease_and_backs_off(pool: PgPool) {
        let state = SummaryState::new(pool.clone());
        let repository = PostgresSummaryRepository::new(pool.clone());
        for _ in 0..2 {
            assert_eq!(
                refresh_daily_summary(State(state.clone())).await.unwrap().0,
                StatusCode::ACCEPTED
            );
            wait_for_refresh(&pool).await;
            assert!(repository.latest_summary().await.unwrap().is_none());
            assert!(repository.claim_refresh(false).await.unwrap().is_none());
            sqlx::query("UPDATE summary_refresh_queue SET due_at = NOW()")
                .execute(&pool)
                .await
                .unwrap();
        }
    }

    #[sqlx::test]
    async fn concurrent_claims_have_one_winner_and_expired_leases_are_recoverable(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let (first, second) = tokio::join!(
            repository.claim_refresh(false),
            repository.claim_refresh(false)
        );
        let winners: Vec<_> = [first.unwrap(), second.unwrap()]
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(winners.len(), 1);
        assert!(repository.claim_refresh(false).await.unwrap().is_none());
        sqlx::query(
            "UPDATE summary_refresh_queue SET lease_expires_at = NOW() - INTERVAL '1 second'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let replacement = repository.claim_refresh(false).await.unwrap().unwrap();
        assert_ne!(replacement, winners[0]);
        assert!(repository.claim_refresh(false).await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn refresh_reports_next_due_at_when_not_due(pool: PgPool) {
        let due_at: DateTime<Utc> = sqlx::query_scalar(
            "UPDATE summary_refresh_queue SET due_at = NOW() + INTERVAL '1 hour' RETURNING due_at",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let (status, Json(body)) = refresh_daily_summary(State(SummaryState::new(pool)))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body.next_due_at, due_at);
    }

    #[sqlx::test]
    async fn refresh_reports_lease_expiry_while_in_progress(pool: PgPool) {
        let lease_expires_at: DateTime<Utc> = sqlx::query_scalar(
            r#"UPDATE summary_refresh_queue
               SET due_at = NOW() - INTERVAL '1 minute',
                   lease_token = 'busy',
                   lease_expires_at = NOW() + INTERVAL '30 minutes'
               RETURNING lease_expires_at"#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let (_, Json(body)) = refresh_daily_summary(State(SummaryState::new(pool)))
            .await
            .unwrap();
        assert_eq!(body.next_due_at, lease_expires_at);
    }

    #[sqlx::test]
    async fn forced_claim_ignores_schedule_but_respects_active_lease(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        sqlx::query("UPDATE summary_refresh_queue SET due_at = NOW() + INTERVAL '1 hour'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(repository.claim_refresh(false).await.unwrap().is_none());
        assert!(repository.claim_refresh(true).await.unwrap().is_some());
        assert!(repository.claim_refresh(true).await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn get_reads_latest_persisted_summary_and_handles_empty_database(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let state = SummaryState::new(pool.clone());
        let Err(error) = fetch_daily_summary(State(state)).await else {
            panic!("expected missing summary");
        };
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(error.message, "No summary is available yet.");
        let token = repository.claim_refresh(true).await.unwrap().unwrap();
        let first = repository
            .complete_refresh(
                &token,
                &GeneratedSummary {
                    summary: "First".into(),
                    feed_ids: vec![1, 2],
                    feed_item_ids: vec![10, 20],
                    model: "model-a".into(),
                    generation_ms: 1,
                    usage: SummaryUsage::default(),
                },
            )
            .await
            .unwrap();
        let token = repository.claim_refresh(true).await.unwrap().unwrap();
        let second = repository
            .complete_refresh(
                &token,
                &GeneratedSummary {
                    summary: "Second".into(),
                    feed_ids: vec![2, 3],
                    feed_item_ids: vec![20, 30],
                    model: "model-b".into(),
                    generation_ms: 1,
                    usage: SummaryUsage::default(),
                },
            )
            .await
            .unwrap();
        let result = fetch_daily_summary(State(SummaryState::new(pool.clone())))
            .await
            .unwrap()
            .0;
        assert!(second.id > first.id);
        assert_eq!(result.id, second.id);
        assert_eq!(result.summary, "Second");
        assert_eq!(result.feed_ids, vec![2, 3]);
        assert_eq!(result.feed_item_ids, vec![20, 30]);
        assert_eq!(result.model, "model-b");
        assert_eq!(result.generated_at, second.generated_at);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ai_summaries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[sqlx::test]
    async fn stale_refresh_cannot_replace_previous_summary(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let feed_id: i64 = sqlx::query_scalar(
            "INSERT INTO feeds (url, title) VALUES ('https://example.com/rss', 'Example') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "WITH item AS (
                INSERT INTO feed_items (feed_id, external_id, title, url)
                VALUES ($1, 'news', 'News', 'https://example.com/news') RETURNING id
             ) INSERT INTO feed_item_details (feed_item_id, summary, content, author, published_at)
               SELECT id, 'Description', '', '', NOW() FROM item",
        )
        .bind(feed_id)
        .execute(&pool)
        .await
        .unwrap();
        let token = repository.claim_refresh(true).await.unwrap().unwrap();
        let previous = repository
            .complete_refresh(
                &token,
                &GeneratedSummary {
                    summary: "Previous overview".into(),
                    feed_ids: vec![feed_id],
                    feed_item_ids: vec![],
                    model: "old-model".into(),
                    generation_ms: 1,
                    usage: SummaryUsage::default(),
                },
            )
            .await
            .unwrap();

        for expired in [false, true] {
            let (mut state, server) = mock_ollama_responses(
                serde_json::json!({"model_info": {"test.context_length": SUMMARY_CONTEXT_TOKENS}}),
                vec![
                    (serde_json::json!({"response": "Notes", "prompt_eval_count": 100, "eval_count": 20, "done_reason": "stop"}), BATCH_OUTPUT_TOKENS),
                    (serde_json::json!({"response": "New overview", "prompt_eval_count": 100, "eval_count": 20, "done_reason": "stop"}), FINAL_OUTPUT_TOKENS),
                ],
            )
            .await;
            state.repository = repository.clone();
            state.models = SummaryModels {
                batch: "test-model".into(),
                final_summary: "test-model".into(),
            };
            sqlx::query(
                "UPDATE summary_refresh_queue SET due_at = NOW(), lease_token = 'current',
                 lease_expires_at = NOW() + INTERVAL '1 hour'",
            )
            .execute(&pool)
            .await
            .unwrap();
            if expired {
                sqlx::query(
                    "UPDATE summary_refresh_queue SET lease_expires_at = NOW() - INTERVAL '1 second'",
                )
                .execute(&pool)
                .await
                .unwrap();
            }
            let schedule_before = repository.next_due_at().await.unwrap();
            let token = if expired { "current" } else { "stale" };
            let error = generate_summary(&state, token).await.err().unwrap();
            assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(error.message, "Summary refresh lease conflict");
            assert_eq!(
                repository.latest_summary().await.unwrap().unwrap().id,
                previous.id
            );
            assert_eq!(repository.next_due_at().await.unwrap(), schedule_before);
            let token: Option<String> =
                sqlx::query_scalar("SELECT lease_token FROM summary_refresh_queue WHERE id = TRUE")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(token.as_deref(), Some("current"));
            server.abort();
        }
    }

    #[sqlx::test]
    async fn recent_articles_include_all_nonempty_descriptions(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let feed_id: i64 = sqlx::query_scalar(
            "INSERT INTO feeds (url, title) VALUES ('https://example.com/rss', 'Example') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        for (index, (age, summary, content)) in [
            (1, "summary 1", ""),
            (1, "", "fallback content"),
            (1, "summary 3", ""),
            (1, "summary 4", ""),
            (1, "summary 5", ""),
            (3, "summary earlier", ""),
            (25, "too old", ""),
            (3, "", ""),
        ]
        .into_iter()
        .enumerate()
        {
            let item_id: i64 = sqlx::query_scalar(
                "INSERT INTO feed_items (feed_id, external_id, title, url, inserted_at) \
                 VALUES ($1, $2, $2, 'https://example.com/item', NOW() - ($3 * INTERVAL '1 hour')) RETURNING id",
            )
            .bind(feed_id)
            .bind(format!("item {index}"))
            .bind(age)
            .fetch_one(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO feed_item_details (feed_item_id, summary, content, author, published_at) \
                 VALUES ($1, $2, $3, '', NOW())",
            )
            .bind(item_id)
            .bind(summary)
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();
        }

        let articles = repository.fetch_articles().await.unwrap();
        assert_eq!(articles.len(), 6);
        assert!(
            articles
                .iter()
                .any(|article| article.description == "summary 1")
        );
        assert!(
            articles
                .iter()
                .any(|article| article.description == "fallback content")
        );
        assert!(
            articles
                .iter()
                .any(|article| article.description == "summary earlier")
        );
        assert!(
            articles
                .iter()
                .all(|article| article.description != "too old")
        );

        sqlx::query(
            "INSERT INTO feed_items (feed_id, external_id, title, url, inserted_at) \
             SELECT $1, 'extra-' || n, 'extra-' || n, 'https://example.com/item', \
                    NOW() - INTERVAL '1 hour' FROM generate_series(1, 101) n",
        )
        .bind(feed_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO feed_item_details (feed_item_id, summary, content, author, published_at) \
             SELECT id, 'extra', '', '', NOW() FROM feed_items WHERE external_id LIKE 'extra-%'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let articles = repository.fetch_articles().await.unwrap();
        assert_eq!(articles.len(), 107);
        assert!(
            articles
                .iter()
                .any(|article| article.description == "summary earlier")
        );
    }
}
