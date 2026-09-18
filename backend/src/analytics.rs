use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use utoipa::ToSchema;

pub const DEFAULT_SUMMARY_DAYS: i64 = 7;
pub const MAX_SUMMARY_DAYS: i64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsEventType {
    PageView,
    ItemOpen,
    SearchPerformed,
    FeedAdded,
    FeedFetchRequested,
}

impl AnalyticsEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PageView => "page_view",
            Self::ItemOpen => "item_open",
            Self::SearchPerformed => "search_performed",
            Self::FeedAdded => "feed_added",
            Self::FeedFetchRequested => "feed_fetch_requested",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewAnalyticsEvent {
    pub event_type: AnalyticsEventType,
    pub path: String,
    pub referrer: Option<String>,
    pub visitor_id: Option<String>,
    pub session_id: Option<String>,
    pub feed_id: Option<i64>,
    pub item_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AnalyticsTotals {
    pub page_views: i64,
    pub unique_visitors: i64,
    pub unique_sessions: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DailyAnalyticsPoint {
    pub date: NaiveDate,
    pub page_views: i64,
    pub unique_visitors: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TopPageStat {
    pub path: String,
    pub page_views: i64,
    pub unique_visitors: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EventBreakdownStat {
    pub event_type: AnalyticsEventType,
    pub count: i64,
    pub unique_visitors: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AnalyticsSummary {
    pub enabled: bool,
    pub window_days: i64,
    pub totals: AnalyticsTotals,
    pub daily_page_views: Vec<DailyAnalyticsPoint>,
    pub top_pages: Vec<TopPageStat>,
    pub event_breakdown: Vec<EventBreakdownStat>,
}

#[derive(Debug, Clone, Copy)]
pub struct SummaryWindow {
    pub days: i64,
}

impl SummaryWindow {
    pub fn new(days: Option<i64>) -> Self {
        let days = days
            .unwrap_or(DEFAULT_SUMMARY_DAYS)
            .clamp(1, MAX_SUMMARY_DAYS);
        Self { days }
    }
}

pub async fn record_event(pool: &PgPool, event: &NewAnalyticsEvent) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO analytics_events (
            event_type,
            path,
            referrer,
            visitor_id,
            session_id,
            feed_id,
            item_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(event.event_type.as_str())
    .bind(&event.path)
    .bind(&event.referrer)
    .bind(&event.visitor_id)
    .bind(&event.session_id)
    .bind(event.feed_id)
    .bind(event.item_id)
    .execute(pool)
    .await
    .with_context(|| format!("failed to record analytics event {:?}", event.event_type))?;

    Ok(())
}

pub fn disabled_summary(window: SummaryWindow) -> AnalyticsSummary {
    AnalyticsSummary {
        enabled: false,
        window_days: window.days,
        totals: AnalyticsTotals {
            page_views: 0,
            unique_visitors: 0,
            unique_sessions: 0,
        },
        daily_page_views: Vec::new(),
        top_pages: Vec::new(),
        event_breakdown: Vec::new(),
    }
}

pub async fn fetch_summary(pool: &PgPool, window: SummaryWindow) -> Result<AnalyticsSummary> {
    let totals = fetch_totals(pool, window).await?;
    let daily_page_views = fetch_daily_page_views(pool, window).await?;
    let top_pages = fetch_top_pages(pool, window).await?;
    let event_breakdown = fetch_event_breakdown(pool, window).await?;

    Ok(AnalyticsSummary {
        enabled: true,
        window_days: window.days,
        totals,
        daily_page_views,
        top_pages,
        event_breakdown,
    })
}

async fn fetch_totals(pool: &PgPool, window: SummaryWindow) -> Result<AnalyticsTotals> {
    #[derive(FromRow)]
    struct TotalsRow {
        page_views: i64,
        unique_visitors: i64,
        unique_sessions: i64,
    }

    let row = sqlx::query_as::<_, TotalsRow>(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE event_type = 'page_view')::BIGINT AS page_views,
            COUNT(DISTINCT CASE WHEN event_type = 'page_view' THEN visitor_id END)::BIGINT AS unique_visitors,
            COUNT(DISTINCT CASE WHEN event_type = 'page_view' THEN session_id END)::BIGINT AS unique_sessions
        FROM analytics_events
        WHERE occurred_at >= NOW() - ($1 * INTERVAL '1 day')
        "#,
    )
    .bind(window.days)
    .fetch_one(pool)
    .await
    .context("failed to load analytics totals")?;

    Ok(AnalyticsTotals {
        page_views: row.page_views,
        unique_visitors: row.unique_visitors,
        unique_sessions: row.unique_sessions,
    })
}

async fn fetch_daily_page_views(
    pool: &PgPool,
    window: SummaryWindow,
) -> Result<Vec<DailyAnalyticsPoint>> {
    #[derive(FromRow)]
    struct DailyRow {
        date: NaiveDate,
        page_views: i64,
        unique_visitors: i64,
    }

    let rows = sqlx::query_as::<_, DailyRow>(
        r#"
        SELECT
            DATE(occurred_at AT TIME ZONE 'UTC') AS date,
            COUNT(*) FILTER (WHERE event_type = 'page_view')::BIGINT AS page_views,
            COUNT(DISTINCT CASE WHEN event_type = 'page_view' THEN visitor_id END)::BIGINT AS unique_visitors
        FROM analytics_events
        WHERE occurred_at >= NOW() - ($1 * INTERVAL '1 day')
        GROUP BY DATE(occurred_at AT TIME ZONE 'UTC')
        HAVING COUNT(*) FILTER (WHERE event_type = 'page_view') > 0
        ORDER BY date DESC
        "#,
    )
    .bind(window.days)
    .fetch_all(pool)
    .await
    .context("failed to load daily analytics")?;

    Ok(rows
        .into_iter()
        .map(|row| DailyAnalyticsPoint {
            date: row.date,
            page_views: row.page_views,
            unique_visitors: row.unique_visitors,
        })
        .collect())
}

async fn fetch_top_pages(pool: &PgPool, window: SummaryWindow) -> Result<Vec<TopPageStat>> {
    #[derive(FromRow)]
    struct TopPageRow {
        path: String,
        page_views: i64,
        unique_visitors: i64,
    }

    let rows = sqlx::query_as::<_, TopPageRow>(
        r#"
        SELECT
            path,
            COUNT(*) FILTER (WHERE event_type = 'page_view')::BIGINT AS page_views,
            COUNT(DISTINCT CASE WHEN event_type = 'page_view' THEN visitor_id END)::BIGINT AS unique_visitors
        FROM analytics_events
        WHERE occurred_at >= NOW() - ($1 * INTERVAL '1 day')
        GROUP BY path
        HAVING COUNT(*) FILTER (WHERE event_type = 'page_view') > 0
        ORDER BY page_views DESC, unique_visitors DESC, path ASC
        LIMIT 10
        "#,
    )
    .bind(window.days)
    .fetch_all(pool)
    .await
    .context("failed to load top pages")?;

    Ok(rows
        .into_iter()
        .map(|row| TopPageStat {
            path: row.path,
            page_views: row.page_views,
            unique_visitors: row.unique_visitors,
        })
        .collect())
}

async fn fetch_event_breakdown(
    pool: &PgPool,
    window: SummaryWindow,
) -> Result<Vec<EventBreakdownStat>> {
    #[derive(FromRow)]
    struct EventBreakdownRow {
        event_type: String,
        count: i64,
        unique_visitors: i64,
    }

    let rows = sqlx::query_as::<_, EventBreakdownRow>(
        r#"
        SELECT
            event_type,
            COUNT(*)::BIGINT AS count,
            COUNT(DISTINCT visitor_id)::BIGINT AS unique_visitors
        FROM analytics_events
        WHERE occurred_at >= NOW() - ($1 * INTERVAL '1 day')
          AND event_type IN (
              'page_view',
              'item_open',
              'search_performed',
              'feed_added',
              'feed_fetch_requested'
          )
        GROUP BY event_type
        ORDER BY count DESC, event_type ASC
        "#,
    )
    .bind(window.days)
    .fetch_all(pool)
    .await
    .context("failed to load analytics event breakdown")?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let event_type = match row.event_type.as_str() {
                "page_view" => AnalyticsEventType::PageView,
                "item_open" => AnalyticsEventType::ItemOpen,
                "search_performed" => AnalyticsEventType::SearchPerformed,
                "feed_added" => AnalyticsEventType::FeedAdded,
                "feed_fetch_requested" => AnalyticsEventType::FeedFetchRequested,
                _ => unreachable!("query filters to known analytics event types"),
            };

            EventBreakdownStat {
                event_type,
                count: row.count,
                unique_visitors: row.unique_visitors,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn test_fetch_summary_counts_page_views_and_uniques(pool: PgPool) {
        record_event(
            &pool,
            &NewAnalyticsEvent {
                event_type: AnalyticsEventType::PageView,
                path: "/".to_string(),
                referrer: None,
                visitor_id: Some("visitor-a".to_string()),
                session_id: Some("session-a".to_string()),
                feed_id: None,
                item_id: None,
            },
        )
        .await
        .unwrap();
        record_event(
            &pool,
            &NewAnalyticsEvent {
                event_type: AnalyticsEventType::PageView,
                path: "/".to_string(),
                referrer: Some("https://example.com".to_string()),
                visitor_id: Some("visitor-a".to_string()),
                session_id: Some("session-b".to_string()),
                feed_id: None,
                item_id: None,
            },
        )
        .await
        .unwrap();
        record_event(
            &pool,
            &NewAnalyticsEvent {
                event_type: AnalyticsEventType::PageView,
                path: "/news".to_string(),
                referrer: None,
                visitor_id: Some("visitor-b".to_string()),
                session_id: Some("session-c".to_string()),
                feed_id: None,
                item_id: None,
            },
        )
        .await
        .unwrap();
        record_event(
            &pool,
            &NewAnalyticsEvent {
                event_type: AnalyticsEventType::ItemOpen,
                path: "/news".to_string(),
                referrer: None,
                visitor_id: Some("visitor-b".to_string()),
                session_id: Some("session-c".to_string()),
                feed_id: None,
                item_id: None,
            },
        )
        .await
        .unwrap();

        let summary = fetch_summary(&pool, SummaryWindow::new(Some(7)))
            .await
            .unwrap();

        assert!(summary.enabled);
        assert_eq!(summary.window_days, 7);
        assert_eq!(summary.totals.page_views, 3);
        assert_eq!(summary.totals.unique_visitors, 2);
        assert_eq!(summary.totals.unique_sessions, 3);
        assert_eq!(summary.top_pages[0].path, "/");
        assert_eq!(summary.top_pages[0].page_views, 2);
        assert_eq!(summary.top_pages[1].path, "/news");
        assert_eq!(summary.daily_page_views[0].page_views, 3);
        assert_eq!(summary.daily_page_views[0].unique_visitors, 2);
        assert_eq!(summary.event_breakdown[0].count, 3);
    }
}
