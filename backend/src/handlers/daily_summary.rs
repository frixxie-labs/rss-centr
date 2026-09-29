use std::time::Duration;

use axum::{Json, extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use tracing::{Instrument, error, info, instrument};
use utoipa::ToSchema;

use super::error::HandlerError;

const ARTICLES_PER_BATCH: usize = 25;
// Bound the whole job below its lease, including all sequential Ollama calls.
const GENERATION_TIMEOUT: Duration = Duration::from_secs(14 * 60);
const LEASE_SECONDS: i64 = 15 * 60;
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
                .unwrap_or_else(|_| "gemma4:e4b".to_string()),
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
    prompt: String,
    num_predict: u32,
) -> Result<String, HandlerError> {
    let request = GenerateRequest {
        model: &state.ollama_model,
        system: SUMMARY_SYSTEM_PROMPT,
        prompt,
        stream: false,
        think: false,
        options: GenerateOptions {
            num_ctx: 8192,
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
        .timeout(Duration::from_secs(120))
        .json(&request)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| {
            error!(%err, "Ollama request failed");
            HandlerError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "Summary service unavailable".to_string(),
            )
        })?;
    let generated: GenerateResponse = response.json().await.map_err(|err| {
        error!(%err, "invalid Ollama response");
        HandlerError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "Invalid summary response".to_string(),
        )
    })?;
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
    let summary = {
        let mut batch_summaries = Vec::new();
        for batch in articles.chunks(ARTICLES_PER_BATCH) {
            batch_summaries.push(generate(state, build_batch_prompt(batch), 320).await?);
        }
        while batch_summaries.len() > 16 {
            let mut merged = Vec::new();
            for group in batch_summaries.chunks(8) {
                merged.push(generate(state, build_final_prompt(group), 320).await?);
            }
            batch_summaries = merged;
        }
        generate(state, build_final_prompt(&batch_summaries), 700).await?
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
            "/api/generate",
            axum::routing::post(move || {
                let response_gate = response_gate.clone();
                async move {
                    response_gate.acquire().await.unwrap().forget();
                    Json(serde_json::json!({"response": "News overview (Example)."}))
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
