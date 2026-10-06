use std::time::Duration;

use axum::{Json, extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use tracing::{Instrument, error, info, instrument};
use utoipa::ToSchema;

use super::error::HandlerError;

const ARTICLES_PER_BATCH: usize = 25;
const SUMMARY_CONTEXT_TOKENS: u32 = 32 * 1024;
// Word limits in the prompts are guidance, not tokenizer limits. Leave enough
// headroom for Norwegian text and source names without accepting cut-off output.
const BATCH_OUTPUT_TOKENS: u32 = 1024;
const FINAL_OUTPUT_TOKENS: u32 = 1536;
const OLLAMA_REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);
// Conservative allowance for the model's chat template and special tokens.
const TEMPLATE_TOKEN_RESERVE: usize = 256;
// Bound the whole job below its lease, including all sequential Ollama calls.
// A day's articles require multiple sequential calls. The 12B model can take
// over two minutes per batch; a 14-minute job deadline cannot cover them all.
const GENERATION_TIMEOUT: Duration = Duration::from_secs(59 * 60);
const LEASE_SECONDS: i64 = 60 * 60;
const SUMMARY_SYSTEM_PROMPT: &str = "Du er nyhetsredaktør. Bruk bare opplysninger fra de oppgitte artiklene eller deloppsummeringene. Oppgi navnet på nyhetskilden i parentes etter hver omtalt sak. Ikke tilskriv en sak en kilde som ikke er oppgitt i grunnlaget, og ikke finn på fakta eller kilder.";

#[derive(Clone)]
pub struct SummaryState {
    pool: PgPool,
    http: reqwest::Client,
    ollama_url: String,
    ollama_model: String,
}

impl SummaryState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            http: reqwest::Client::new(),
            ollama_url: std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://desktop:11434".to_string()),
            ollama_model: std::env::var("OLLAMA_MODEL")
                .unwrap_or_else(|_| "gemma4:12b".to_string()),
        }
    }
}

#[derive(Clone, Serialize, FromRow, ToSchema)]
pub struct DailySummary {
    pub id: i64,
    pub summary: String,
    pub feed_ids: Vec<i64>,
    pub feed_item_ids: Vec<i64>,
    pub generated_at: DateTime<Utc>,
    pub model: String,
}

#[derive(FromRow)]
struct SummaryArticle {
    id: i64,
    feed_id: i64,
    title: String,
    feed: String,
    description: String,
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    system: &'static str,
    prompt: String,
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
}

#[derive(Deserialize)]
struct ShowResponse {
    model_info: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Copy)]
struct ContextBudget {
    num_ctx: u32,
}

impl ContextBudget {
    fn estimated_input_tokens(self, prompt: &str) -> usize {
        // One token per UTF-8 byte deliberately overestimates ordinary news text.
        // This is a sizing heuristic, not the model's tokenizer; actual usage is
        // checked and logged from Ollama's response as well.
        SUMMARY_SYSTEM_PROMPT.len() + prompt.len() + TEMPLATE_TOKEN_RESERVE
    }

    fn fits(self, prompt: &str, num_predict: u32) -> bool {
        self.estimated_input_tokens(prompt)
            .saturating_add(num_predict as usize)
            <= self.num_ctx as usize
    }
}

async fn fetch_context_budget(state: &SummaryState) -> Result<ContextBudget, HandlerError> {
    let response = state
        .http
        .post(format!(
            "{}/api/show",
            state.ollama_url.trim_end_matches('/')
        ))
        .timeout(Duration::from_secs(30))
        .json(&serde_json::json!({"model": state.ollama_model}))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| {
            error!(error = ?err, "failed to fetch Ollama model metadata");
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
    info!(model = %state.ollama_model, model_context, num_ctx, "summary model context budget");
    Ok(ContextBudget { num_ctx })
}

fn budgeted_prompts<T>(
    items: &[T],
    build: impl Fn(&[T]) -> String,
    budget: ContextBudget,
    num_predict: u32,
    max_items: usize,
) -> Result<Vec<String>, HandlerError> {
    let mut prompts = Vec::new();
    let mut start = 0;
    while start < items.len() {
        let mut end = start;
        let mut accepted = None;
        while end < items.len() && end - start < max_items {
            let candidate = build(&items[start..=end]);
            if !budget.fits(&candidate, num_predict) {
                break;
            }
            accepted = Some(candidate);
            end += 1;
        }
        let prompt = accepted.ok_or_else(|| {
            error!(
                num_ctx = budget.num_ctx,
                num_predict, "single summary input exceeds context budget"
            );
            HandlerError::internal("Summary input exceeds model context budget")
        })?;
        prompts.push(prompt);
        start = end;
    }
    Ok(prompts)
}

fn build_batch_prompt(articles: &[SummaryArticle]) -> String {
    let mut prompt = String::from(
        "Oppsummer hovedsakene i disse nyhetsartiklene på norsk i 3–4 korte setninger (maks 80 ord), uten punktliste eller markdown. \
         Behold kildenavn ved hver sak slik at den samlede oversikten kan vise hvor nyhetene kommer fra. \
         Grupper relaterte saker og bruk bare opplysninger som står i artiklene. \
         Ikke finn på datoer, tall eller hendelser. Denne gruppen er del av en større nyhetsoversikt.\n\n",
    );
    for article in articles {
        prompt.push_str(&format!(
            "Kilde: {}\nTittel: {}\nBeskrivelse: {}\n\n",
            article.feed, article.title, article.description
        ));
    }
    prompt
}

fn build_final_prompt(batch_summaries: &[String]) -> String {
    let mut prompt = String::from(
        "Skriv en kort samlet nyhetsoversikt på norsk for de siste 24 timene i 5–7 setninger (maks 160 ord) ut fra deloppsummeringene nedenfor. \
         Ta med kildenavn i parentes ved hver omtalt sak. \
         Grupper relaterte saker, unngå gjentakelser og prioriter de viktigste sakene. \
         Bruk bare opplysninger fra deloppsummeringene; ikke finn på fakta. \
         Skriv vanlig tekst uten markdown. Ikke påstå at alle saker er omtalt.\n\n",
    );
    for (index, summary) in batch_summaries.iter().enumerate() {
        prompt.push_str(&format!("Del {}: {}\n\n", index + 1, summary));
    }
    prompt
}

async fn fetch_articles(pool: &PgPool) -> Result<Vec<SummaryArticle>, sqlx::Error> {
    sqlx::query_as::<_, SummaryArticle>(
        r#"
        SELECT i.id, i.feed_id, i.title, COALESCE(f.title, f.url) AS feed,
               LEFT(COALESCE(NULLIF(BTRIM(d.summary), ''),
                             NULLIF(BTRIM(d.content), '')), 300) AS description
        FROM feed_items i
        JOIN feeds f ON f.id = i.feed_id
        JOIN feed_item_details d ON d.feed_item_id = i.id
        WHERE i.inserted_at >= NOW() - INTERVAL '24 hours'
          AND i.inserted_at <= NOW()
          AND COALESCE(NULLIF(BTRIM(d.summary), ''),
                       NULLIF(BTRIM(d.content), '')) IS NOT NULL
        ORDER BY i.inserted_at DESC, i.id DESC
        "#,
    )
    .fetch_all(pool)
    .await
}

async fn generate(
    state: &SummaryState,
    budget: ContextBudget,
    prompt: String,
    num_predict: u32,
) -> Result<String, HandlerError> {
    if !budget.fits(&prompt, num_predict) {
        return Err(HandlerError::internal(
            "Summary input exceeds model context budget",
        ));
    }
    info!(model = %state.ollama_model, estimated_input_tokens = budget.estimated_input_tokens(&prompt),
        num_ctx = budget.num_ctx, num_predict, "generating summary within context budget");
    let request = GenerateRequest {
        model: &state.ollama_model,
        system: SUMMARY_SYSTEM_PROMPT,
        prompt,
        stream: false,
        think: false,
        options: GenerateOptions {
            num_ctx: budget.num_ctx,
            num_predict,
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
            error!(error = ?err, timeout = err.is_timeout(), timeout_seconds = OLLAMA_REQUEST_TIMEOUT.as_secs(), "Ollama request failed");
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
    info!(model = %state.ollama_model, prompt_tokens = generated.prompt_eval_count,
        output_tokens = generated.eval_count, num_ctx = budget.num_ctx,
        done_reason = %generated.done_reason, "summary token usage");
    if generated.done_reason != "stop"
        || generated
            .prompt_eval_count
            .saturating_add(generated.eval_count)
            > budget.num_ctx
        || generated.prompt_eval_count.saturating_add(num_predict) > budget.num_ctx
    {
        return Err(HandlerError::new(
            StatusCode::BAD_GATEWAY,
            "Summary generation truncated or exceeded context budget".to_string(),
        ));
    }
    if generated.response.trim().is_empty() {
        return Err(HandlerError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "Empty summary response".to_string(),
        ));
    }
    Ok(generated.response.trim().to_string())
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
    latest_summary(&state.pool)
        .await?
        .map(Json)
        .ok_or_else(|| HandlerError::not_found("No summary is available yet."))
}

async fn latest_summary(pool: &PgPool) -> Result<Option<DailySummary>, HandlerError> {
    sqlx::query_as::<_, DailySummary>(
        r#"SELECT id, summary, feed_ids, feed_item_ids, generated_at, model
           FROM ai_summaries ORDER BY generated_at DESC, id DESC LIMIT 1"#,
    )
    .fetch_optional(pool)
    .await
    .map_err(|err| {
        error!(%err, "failed to fetch latest summary");
        HandlerError::internal("Failed to fetch latest summary")
    })
}

async fn save_summary<'a>(
    executor: impl sqlx::Executor<'a, Database = sqlx::Postgres>,
    summary: &str,
    feed_ids: &[i64],
    feed_item_ids: &[i64],
    model: &str,
) -> Result<DailySummary, HandlerError> {
    sqlx::query_as::<_, DailySummary>(
        r#"INSERT INTO ai_summaries (summary, feed_ids, feed_item_ids, model)
           VALUES ($1, $2, $3, $4)
           RETURNING id, summary, feed_ids, feed_item_ids, generated_at, model"#,
    )
    .bind(summary)
    .bind(feed_ids)
    .bind(feed_item_ids)
    .bind(model)
    .fetch_one(executor)
    .await
    .map_err(|err| {
        error!(%err, "failed to persist summary");
        HandlerError::internal("Failed to save summary")
    })
}

#[utoipa::path(
    post,
    path = "/internal/items/summary/refresh",
    tag = "items",
    responses(
        (status = 202, description = "Summary refresh accepted, already in progress, or not due yet"),
        (status = 500, description = "Summary service unavailable"),
    )
)]
#[instrument(skip(state))]
pub async fn refresh_daily_summary(
    State(state): State<SummaryState>,
) -> Result<StatusCode, HandlerError> {
    let Some(token) = claim_summary_refresh(&state.pool).await? else {
        return Ok(StatusCode::ACCEPTED);
    };
    info!(model = %state.ollama_model, timeout_seconds = GENERATION_TIMEOUT.as_secs(), "daily news summary refresh started");
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
            if let Err(err) = sqlx::query(
                r#"UPDATE summary_refresh_queue
                   SET lease_token = NULL, lease_expires_at = NULL,
                       due_at = NOW() + INTERVAL '1 minute'
                   WHERE id = TRUE AND lease_token = $1"#,
            )
            .bind(&token)
            .execute(&state.pool)
            .await
            {
                error!(%err, "failed to release summary refresh lease");
            }
        }
        .in_current_span(),
    );
    Ok(StatusCode::ACCEPTED)
}

async fn claim_summary_refresh(pool: &PgPool) -> Result<Option<String>, HandlerError> {
    sqlx::query_scalar(
        r#"WITH due AS (
               SELECT id FROM summary_refresh_queue
               WHERE due_at <= NOW()
                 AND (lease_expires_at IS NULL OR lease_expires_at <= NOW())
               FOR UPDATE SKIP LOCKED
           )
           UPDATE summary_refresh_queue q
           SET lease_token = md5(random()::TEXT || clock_timestamp()::TEXT),
               lease_expires_at = NOW() + ($1::BIGINT * INTERVAL '1 second')
           FROM due WHERE q.id = due.id
           RETURNING q.lease_token"#,
    )
    .bind(LEASE_SECONDS)
    .fetch_optional(pool)
    .await
    .map_err(|err| {
        error!(%err, "failed to claim summary refresh lease");
        HandlerError::internal("Failed to claim summary refresh")
    })
}

async fn generate_summary(state: &SummaryState, token: &str) -> Result<DailySummary, HandlerError> {
    let articles = fetch_articles(&state.pool).await.map_err(|err| {
        error!(%err, "failed to fetch summary articles");
        HandlerError::internal("Failed to fetch recent articles")
    })?;

    if articles.is_empty() {
        return Err(HandlerError::not_found(
            "No recent articles available for summary generation.",
        ));
    }
    let budget = fetch_context_budget(state).await?;
    let summary = {
        let mut batch_summaries = Vec::new();
        let prompts = budgeted_prompts(
            &articles,
            build_batch_prompt,
            budget,
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
            batch_summaries.push(generate(state, budget, prompt, BATCH_OUTPUT_TOKENS).await?);
        }
        while batch_summaries.len() > 16
            || !budget.fits(&build_final_prompt(&batch_summaries), FINAL_OUTPUT_TOKENS)
        {
            let mut merged = Vec::new();
            let prompts = budgeted_prompts(
                &batch_summaries,
                build_final_prompt,
                budget,
                BATCH_OUTPUT_TOKENS,
                8,
            )?;
            if prompts.len() >= batch_summaries.len() {
                return Err(HandlerError::internal(
                    "Summary context budget too small to merge summaries",
                ));
            }
            for prompt in prompts {
                merged.push(generate(state, budget, prompt, BATCH_OUTPUT_TOKENS).await?);
            }
            batch_summaries = merged;
        }
        info!(
            batches = batch_summaries.len(),
            "generating final news summary"
        );
        generate(
            state,
            budget,
            build_final_prompt(&batch_summaries),
            FINAL_OUTPUT_TOKENS,
        )
        .await?
    };

    let mut feed_ids: Vec<i64> = articles.iter().map(|article| article.feed_id).collect();
    feed_ids.sort_unstable();
    feed_ids.dedup();
    let feed_item_ids: Vec<i64> = articles.iter().map(|article| article.id).collect();
    let mut tx = state.pool.begin().await.map_err(|err| {
        error!(%err, "failed to begin summary completion");
        HandlerError::internal("Failed to save summary")
    })?;
    // Fence stale jobs and reschedule atomically with persistence, like feed completion.
    let completed = sqlx::query(
        r#"UPDATE summary_refresh_queue
           SET lease_token = NULL, lease_expires_at = NULL,
               due_at = NOW() + INTERVAL '1 hour'
           WHERE id = TRUE AND lease_token = $1 AND lease_expires_at > NOW()"#,
    )
    .bind(token)
    .execute(&mut *tx)
    .await
    .map_err(|err| {
        error!(%err, "failed to complete summary refresh lease");
        HandlerError::internal("Failed to complete summary refresh")
    })?;
    if completed.rows_affected() == 0 {
        return Err(HandlerError::internal("Summary refresh lease conflict"));
    }
    let result = save_summary(
        &mut *tx,
        &summary,
        &feed_ids,
        &feed_item_ids,
        &state.ollama_model,
    )
    .await?;
    tx.commit().await.map_err(|err| {
        error!(%err, "failed to commit summary completion");
        HandlerError::internal("Failed to save summary")
    })?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    use super::*;

    #[test]
    fn generation_deadlines_allow_slow_batches_and_finish_before_the_lease() {
        assert!(OLLAMA_REQUEST_TIMEOUT > Duration::from_secs(120));
        // The observed workload is 312 articles: 13 batches and a final merge.
        let calls = 312_usize.div_ceil(ARTICLES_PER_BATCH) + 1;
        assert!(GENERATION_TIMEOUT > Duration::from_secs(120) * calls as u32);
        assert!(GENERATION_TIMEOUT.as_secs() < LEASE_SECONDS as u64);
    }

    #[test]
    fn summary_batches_reserve_the_full_generation_allowance() {
        let articles: Vec<_> = (0..26)
            .map(|id| SummaryArticle {
                id,
                feed_id: 1,
                title: "Title".to_string(),
                feed: "Source".to_string(),
                description: "æ".repeat(300),
            })
            .collect();
        let budget = ContextBudget { num_ctx: 8192 };
        let prompts = budgeted_prompts(
            &articles,
            build_batch_prompt,
            budget,
            BATCH_OUTPUT_TOKENS,
            ARTICLES_PER_BATCH,
        )
        .unwrap();
        assert!(prompts.len() > 1);
        assert!(
            prompts
                .iter()
                .all(|prompt| budget.fits(prompt, BATCH_OUTPUT_TOKENS))
        );
        assert_eq!(
            prompts
                .iter()
                .map(|prompt| prompt.matches("Kilde:").count())
                .sum::<usize>(),
            articles.len()
        );
    }

    #[test]
    fn context_budget_reserves_system_template_and_output_tokens() {
        let prompt = "Nyheter: blåbær 世界";
        let num_predict = 700;
        let required = SUMMARY_SYSTEM_PROMPT.len()
            + prompt.len()
            + TEMPLATE_TOKEN_RESERVE
            + num_predict as usize;
        assert!(
            ContextBudget {
                num_ctx: required as u32
            }
            .fits(prompt, num_predict)
        );
        assert!(
            !ContextBudget {
                num_ctx: required as u32 - 1
            }
            .fits(prompt, num_predict)
        );
    }

    #[test]
    fn batches_split_by_context_and_preserve_every_input() {
        let items = vec!["a".repeat(2000), "b".repeat(2000), "c".repeat(2000)];
        let budget = ContextBudget { num_ctx: 6000 };
        let prompts = budgeted_prompts(&items, |group| group.concat(), budget, 700, 25).unwrap();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts.concat(), items.concat());
        assert!(prompts.iter().all(|prompt| budget.fits(prompt, 700)));
        let limited = budgeted_prompts(&items, |group| group.concat(), budget, 700, 1).unwrap();
        assert_eq!(limited.len(), 3);
    }

    #[test]
    fn configured_context_is_32k_and_batches_reserve_output_space() {
        assert_eq!(SUMMARY_CONTEXT_TOKENS, 32768);
        let budget = ContextBudget {
            num_ctx: SUMMARY_CONTEXT_TOKENS,
        };
        let num_predict = 700;
        let max_prompt_bytes = budget.num_ctx as usize
            - SUMMARY_SYSTEM_PROMPT.len()
            - TEMPLATE_TOKEN_RESERVE
            - num_predict as usize;
        assert!(budget.fits(&"x".repeat(max_prompt_bytes), num_predict));
        assert!(!budget.fits(&"x".repeat(max_prompt_bytes + 1), num_predict));

        let items = vec!["a".repeat(16000), "b".repeat(16000), "c".repeat(16000)];
        let prompts =
            budgeted_prompts(&items, |group| group.concat(), budget, num_predict, 25).unwrap();
        assert!(prompts.len() > 1);
        assert_eq!(prompts.concat(), items.concat());
        assert!(
            prompts
                .iter()
                .all(|prompt| budget.fits(prompt, num_predict))
        );
    }

    #[test]
    fn oversized_single_input_fails_instead_of_being_silently_truncated() {
        let items = vec!["x".repeat(8192)];
        assert!(
            budgeted_prompts(
                &items,
                build_final_prompt,
                ContextBudget { num_ctx: 8192 },
                700,
                8
            )
            .is_err()
        );
    }

    #[test]
    fn final_merge_is_batched_by_size_not_just_summary_count() {
        let summaries = vec!["æ".repeat(1200); 4];
        let budget = ContextBudget { num_ctx: 8192 };
        assert!(!budget.fits(&build_final_prompt(&summaries), 700));
        let prompts = budgeted_prompts(&summaries, build_final_prompt, budget, 320, 8).unwrap();
        assert!(prompts.len() > 1 && prompts.len() < summaries.len());
        assert!(prompts.iter().all(|prompt| budget.fits(prompt, 320)));
    }

    async fn mock_ollama(
        metadata: serde_json::Value,
        generated: serde_json::Value,
        num_predict: u32,
    ) -> (SummaryState, tokio::task::JoinHandle<()>) {
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
                    let generated = generated.clone();
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
        state.ollama_model = "test-model".to_string();
        (state, server)
    }

    #[tokio::test]
    async fn model_metadata_caps_context_and_requires_a_limit() {
        for (metadata, expected) in [
            (
                serde_json::json!({"model_info": {"gemma4.context_length": 262144}}),
                Some(32768),
            ),
            (
                serde_json::json!({"model_info": {"gemma4.context_length": 32768}}),
                Some(32768),
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
            let result = fetch_context_budget(&state).await;
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
                serde_json::json!({"response": "News", "prompt_eval_count": 32069, "eval_count": 20, "done_reason": "stop"}),
                false,
            ),
            (
                serde_json::json!({"response": "News", "prompt_eval_count": 32000, "eval_count": 800, "done_reason": "stop"}),
                false,
            ),
            (
                serde_json::json!({"response": "News", "prompt_eval_count": 32068, "eval_count": 700, "done_reason": "stop"}),
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
                ContextBudget {
                    num_ctx: SUMMARY_CONTEXT_TOKENS,
                },
                "News".to_string(),
                700,
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
                    ContextBudget {
                        num_ctx: SUMMARY_CONTEXT_TOKENS,
                    },
                    "x".repeat(SUMMARY_CONTEXT_TOKENS as usize),
                    700
                )
                .await
                .is_err()
            );
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
                    ContextBudget {
                        num_ctx: SUMMARY_CONTEXT_TOKENS,
                    },
                    "News".to_string(),
                    num_predict,
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let responses = Arc::new(Semaphore::new(0));
        let response_gate = responses.clone();
        let app = axum::Router::new().route(
            "/api/show",
            axum::routing::post(|| async {
                Json(serde_json::json!({"model_info": {"gemma4.context_length": 262144}}))
            }),
        ).route(
            "/api/generate",
            axum::routing::post(move |Json(request): Json<serde_json::Value>| {
                let response_gate = response_gate.clone();
                async move {
                    let prompt = request["prompt"].as_str().unwrap();
                    let expected_output = if prompt.starts_with("Oppsummer hovedsakene") {
                        BATCH_OUTPUT_TOKENS
                    } else {
                        assert!(prompt.starts_with("Skriv en kort samlet nyhetsoversikt"));
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
        state.ollama_model = "test-model".to_string();
        // Independent state and pool represent separate backend instances.
        let other_pool = sqlx::postgres::PgPoolOptions::new()
            .connect_with((*pool.connect_options()).clone())
            .await
            .unwrap();
        let mut other_state = SummaryState::new(other_pool.clone());
        other_state.ollama_url = state.ollama_url.clone();
        other_state.ollama_model = state.ollama_model.clone();
        for instance in [state.clone(), other_state.clone()] {
            let status = tokio::time::timeout(
                Duration::from_secs(1),
                refresh_daily_summary(State(instance)),
            )
            .await
            .expect("refresh should return without waiting for generation")
            .unwrap();
            assert_eq!(status, StatusCode::ACCEPTED);
        }
        assert!(latest_summary(&pool).await.unwrap().is_none());
        responses.add_permits(2);
        wait_for_refresh(&pool).await;
        // A staggered worker must not generate again immediately after completion.
        refresh_daily_summary(State(other_state)).await.unwrap();
        assert!(claim_summary_refresh(&pool).await.unwrap().is_none());
        let stored = fetch_daily_summary(State(SummaryState::new(pool)))
            .await
            .unwrap()
            .0;
        assert_eq!(stored.feed_ids, vec![feed_id]);
        assert_eq!(stored.feed_item_ids, expected_ids);
        assert_eq!(stored.model, "test-model");
        assert_eq!(stored.summary, "News overview (Example).");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ai_summaries")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        server.abort();
        other_pool.close().await;
    }

    #[sqlx::test]
    async fn failed_background_refresh_releases_lease_and_backs_off(pool: PgPool) {
        let state = SummaryState::new(pool);
        for _ in 0..2 {
            assert_eq!(
                refresh_daily_summary(State(state.clone())).await.unwrap(),
                StatusCode::ACCEPTED
            );
            wait_for_refresh(&state.pool).await;
            assert!(latest_summary(&state.pool).await.unwrap().is_none());
            assert!(claim_summary_refresh(&state.pool).await.unwrap().is_none());
            sqlx::query("UPDATE summary_refresh_queue SET due_at = NOW()")
                .execute(&state.pool)
                .await
                .unwrap();
        }
    }

    #[sqlx::test]
    async fn concurrent_claims_have_one_winner_and_expired_leases_are_recoverable(pool: PgPool) {
        let (first, second) =
            tokio::join!(claim_summary_refresh(&pool), claim_summary_refresh(&pool));
        let winners: Vec<_> = [first.unwrap(), second.unwrap()]
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(winners.len(), 1);
        assert!(claim_summary_refresh(&pool).await.unwrap().is_none());
        sqlx::query(
            "UPDATE summary_refresh_queue SET lease_expires_at = NOW() - INTERVAL '1 second'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let replacement = claim_summary_refresh(&pool).await.unwrap().unwrap();
        assert_ne!(replacement, winners[0]);
        assert!(claim_summary_refresh(&pool).await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn get_reads_latest_persisted_summary_and_handles_empty_database(pool: PgPool) {
        let state = SummaryState::new(pool.clone());
        let Err(error) = fetch_daily_summary(State(state)).await else {
            panic!("expected missing summary");
        };
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(error.message, "No summary is available yet.");
        let first = save_summary(&pool, "First", &[1, 2], &[10, 20], "model-a")
            .await
            .unwrap();
        let second = save_summary(&pool, "Second", &[2, 3], &[20, 30], "model-b")
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
    async fn recent_articles_include_all_nonempty_descriptions(pool: PgPool) {
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

        let articles = fetch_articles(&pool).await.unwrap();
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

        let articles = fetch_articles(&pool).await.unwrap();
        assert_eq!(articles.len(), 107);
        assert!(
            articles
                .iter()
                .any(|article| article.description == "summary earlier")
        );
    }
}
