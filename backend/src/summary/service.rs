use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tracing::{Instrument, error, info};

use super::{
    ARTICLES_PER_BATCH, BATCH_OUTPUT_TOKENS, DailySummary, FINAL_OUTPUT_TOKENS, GeneratedSummary,
    OllamaSummaryGenerator, PostgresSummaryRepository, SummaryError, SummaryGenerator,
    SummaryModels, SummaryRepository, SummaryStoreError, SummaryUsage, budgeted_prompts,
    build_batch_prompt, build_final_prompt, build_merge_prompt,
};

// Bound the whole job below its lease, including all sequential Ollama calls.
// A day's articles require multiple sequential calls. The 12B model can take
// over two minutes per batch; a 14-minute job deadline cannot cover them all.
const GENERATION_TIMEOUT: Duration = Duration::from_secs(59 * 60);

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
                state
                    .generator
                    .generate(
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
                return Err(SummaryError::MergeBudgetTooSmall);
            }
            for prompt in prompts {
                merged.push(
                    state
                        .generator
                        .generate(
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
        state
            .generator
            .generate(
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

    use axum::{Json, extract::State, http::StatusCode};
    use chrono::{DateTime, Utc};
    use tokio::sync::Semaphore;

    use crate::handlers::daily_summary::{fetch_daily_summary, refresh_daily_summary};
    use crate::summary::ollama::OLLAMA_REQUEST_TIMEOUT;
    use crate::summary::{ContextBudget, LEASE_SECONDS, SUMMARY_CONTEXT_TOKENS};

    use super::*;

    fn summary_state(pool: PgPool) -> SummaryState {
        SummaryState::new(
            pool,
            reqwest::Client::new(),
            "http://127.0.0.1:1".into(),
            SummaryModels::default(),
        )
    }

    #[tokio::test]
    async fn state_uses_the_injected_client_endpoint_and_summary_models() {
        let app = axum::Router::new()
            .route("/configured/api/show", axum::routing::post(|headers: axum::http::HeaderMap, Json(request): Json<serde_json::Value>| async move {
                assert_eq!(headers["x-configured-client"], "summary");
                assert_eq!(request["model"], "configured-batch");
                Json(serde_json::json!({"model_info": {"test.context_length": SUMMARY_CONTEXT_TOKENS}}))
            }))
            .route("/configured/api/generate", axum::routing::post(|headers: axum::http::HeaderMap, Json(request): Json<serde_json::Value>| async move {
                assert_eq!(headers["x-configured-client"], "summary");
                assert_eq!(request["model"], "configured-final");
                Json(serde_json::json!({"response": "Configured overview", "prompt_eval_count": 100, "eval_count": 20, "done_reason": "stop"}))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-configured-client", "summary".parse().unwrap());
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/rss_centr")
            .unwrap();

        let state = SummaryState::new(
            pool,
            http,
            format!("http://{address}/configured/"),
            SummaryModels {
                batch: "configured-batch".into(),
                final_summary: "configured-final".into(),
            },
        );

        assert_eq!(state.models.label(), "configured-batch → configured-final");
        let budget = state
            .generator
            .context_budget(&state.models.batch)
            .await
            .unwrap();
        let summary = state
            .generator
            .generate(
                &state.models.final_summary,
                budget,
                "News".into(),
                FINAL_OUTPUT_TOKENS,
                &mut SummaryUsage::default(),
            )
            .await
            .unwrap();
        assert_eq!(summary, "Configured overview");
        server.abort();
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
        let state = SummaryState::new(
            pool,
            reqwest::Client::new(),
            format!("http://{address}"),
            SummaryModels::default(),
        );
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
            let result = state.generator.context_budget("test-model").await;
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
            let result = state
                .generator
                .generate(
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
                state
                    .generator
                    .generate(
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
            let result = state
                .generator
                .generate(
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
        let err = state
            .generator
            .generate(
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
        assert!(matches!(err, SummaryError::Truncated));
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
            let err = state
                .generator
                .generate(
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
            assert!(matches!(err, SummaryError::Truncated));
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
            let err = state
                .generator
                .generate(
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
            assert_eq!(err.to_string(), message);
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
                let result = state
                    .generator
                    .generate(
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
        let mut state = summary_state(pool.clone());
        let expected_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM feed_items WHERE feed_id = $1 ORDER BY inserted_at DESC, id DESC",
        )
        .bind(feed_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        state.generator =
            OllamaSummaryGenerator::new(reqwest::Client::new(), format!("http://{address}"));
        state.models = SummaryModels {
            batch: "batch-model".to_string(),
            final_summary: "final-model".to_string(),
        };
        // Independent state and pool represent separate backend instances.
        let other_pool = sqlx::postgres::PgPoolOptions::new()
            .connect_with((*pool.connect_options()).clone())
            .await
            .unwrap();
        let mut other_state = summary_state(other_pool.clone());
        other_state.generator = state.generator.clone();
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
        let stored = fetch_daily_summary(State(summary_state(pool.clone())))
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
            let mut state = summary_state(pool.clone());
            state.generator =
                OllamaSummaryGenerator::new(reqwest::Client::new(), format!("http://{address}"));
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
        let state = summary_state(pool.clone());
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
        let (status, Json(body)) = refresh_daily_summary(State(summary_state(pool)))
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
        let (_, Json(body)) = refresh_daily_summary(State(summary_state(pool)))
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
        let state = summary_state(pool.clone());
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
        let result = fetch_daily_summary(State(summary_state(pool.clone())))
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
            assert!(matches!(
                error,
                SummaryError::Store(SummaryStoreError::LeaseConflict)
            ));
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
