use std::{sync::Arc, time::Duration};

use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use tokio::sync::{RwLock, Semaphore};
use tracing::{error, instrument};
use utoipa::ToSchema;

use super::error::HandlerError;

const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const ARTICLES_PER_BATCH: usize = 25;
const SUMMARY_SYSTEM_PROMPT: &str = "Du er nyhetsredaktør. Bruk bare opplysninger fra de oppgitte artiklene eller deloppsummeringene. Oppgi navnet på nyhetskilden i parentes etter hver omtalt sak. Ikke tilskriv en sak en kilde som ikke er oppgitt i grunnlaget, og ikke finn på fakta eller kilder.";

#[derive(Clone)]
pub struct SummaryState {
    pool: PgPool,
    http: reqwest::Client,
    ollama_url: String,
    ollama_model: String,
    cache: Arc<RwLock<Option<(tokio::time::Instant, DailySummary)>>>,
    generation_slot: Arc<Semaphore>,
}

impl SummaryState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            http: reqwest::Client::new(),
            ollama_url: std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://desktop:11434".to_string()),
            ollama_model: std::env::var("OLLAMA_MODEL")
                .unwrap_or_else(|_| "gemma4:e2b".to_string()),
            cache: Arc::new(RwLock::new(None)),
            generation_slot: Arc::new(Semaphore::new(1)),
        }
    }
}

#[derive(Clone, Serialize, ToSchema)]
pub struct DailySummary {
    pub summary: String,
    pub articles_sampled: usize,
    pub articles_available: i64,
    pub generated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct SummaryArticle {
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
        SELECT i.title, COALESCE(f.title, f.url) AS feed,
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
        (status = 500, description = "Database error"),
        (status = 502, description = "Ollama unavailable"),
    )
)]
#[instrument(skip(state))]
pub async fn fetch_daily_summary(
    State(state): State<SummaryState>,
) -> Result<Json<DailySummary>, HandlerError> {
    get_summary(&state, false).await
}

#[utoipa::path(
    post,
    path = "/internal/items/summary/refresh",
    tag = "items",
    responses(
        (status = 200, description = "Refresh the cached daily overview", body = DailySummary),
        (status = 500, description = "Database error"),
        (status = 502, description = "Ollama unavailable"),
    )
)]
#[instrument(skip(state))]
pub async fn refresh_daily_summary(
    State(state): State<SummaryState>,
) -> Result<Json<DailySummary>, HandlerError> {
    get_summary(&state, true).await
}

async fn get_summary(
    state: &SummaryState,
    refresh: bool,
) -> Result<Json<DailySummary>, HandlerError> {
    let started = tokio::time::Instant::now();
    if !refresh
        && let Some((generated, summary)) = &*state.cache.read().await
        && generated.elapsed() < CACHE_TTL
    {
        return Ok(Json(summary.clone()));
    }

    // Concurrent landing-page requests share the same generation rather than
    // each starting another full set of model calls.
    let _slot = state.generation_slot.acquire().await.map_err(|err| {
        error!(%err, "summary generation slot closed");
        HandlerError::internal("Summary service unavailable")
    })?;
    if let Some((generated, summary)) = &*state.cache.read().await
        && (*generated >= started || (!refresh && generated.elapsed() < CACHE_TTL))
    {
        return Ok(Json(summary.clone()));
    }

    let articles = fetch_articles(&state.pool).await.map_err(|err| {
        error!(%err, "failed to fetch summary articles");
        HandlerError::internal("Failed to fetch recent articles")
    })?;

    let summary = if articles.is_empty() {
        "Ingen nyheter med beskrivelser de siste 24 timene.".to_string()
    } else {
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

    let result = DailySummary {
        summary,
        articles_sampled: articles.len(),
        articles_available: articles.len() as i64,
        generated_at: Utc::now(),
    };
    *state.cache.write().await = Some((tokio::time::Instant::now(), result.clone()));
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn refresh_replaces_cached_summary_while_get_reuses_it(pool: PgPool) {
        let state = SummaryState::new(pool);
        let first = fetch_daily_summary(State(state.clone())).await.unwrap().0;
        let cached = fetch_daily_summary(State(state.clone())).await.unwrap().0;
        assert_eq!(first.generated_at, cached.generated_at);

        let refreshed = refresh_daily_summary(State(state.clone())).await.unwrap().0;
        assert!(refreshed.generated_at > first.generated_at);
        let cached = fetch_daily_summary(State(state)).await.unwrap().0;
        assert_eq!(refreshed.generated_at, cached.generated_at);
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
