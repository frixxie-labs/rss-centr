export interface FeedItem {
  id: number;
  feed_id: number;
  external_id: string;
  title: string;
  url: string;
  inserted_at: string;
  summary?: string | null;
  content?: string | null;
  author?: string | null;
  published_at?: string | null;
}

export interface FeedItemDetail {
  id: number;
  feed_item_id: number;
  summary: string;
  content: string;
  author: string;
  published_at: string;
}

export interface FeedSubscription {
  id: number;
  url: string;
  title: string | null;
  site_url: string | null;
  etag: string | null;
  last_modified: string | null;
  poll_interval_seconds: number;
  is_enabled: boolean;
  last_checked_at: string | null;
  last_success_at: string | null;
  last_inserted_at: string | null;
  failure_count: number;
}

export interface NewFeedItemEvent {
  id: number;
  feed_id: number;
  external_id: string;
  title: string;
  url: string;
  inserted_at: string;
}

export interface FeedTitleIndexItem {
  feed_src_id: number;
  occurrences: number;
}

export interface FeedTitleIndexEntry {
  word: string;
  total_occurrences: number;
  /** Number of distinct feed item titles containing this word at least once. */
  document_frequency: number;
  /** TF-IDF score: higher means more distinctive (concentrated in fewer titles). */
  tf_idf: number;
  items: FeedTitleIndexItem[];
}

export type AnalyticsEventType =
  | "page_view"
  | "item_open"
  | "search_performed"
  | "feed_added"
  | "feed_fetch_requested";

export interface AnalyticsTotals {
  page_views: number;
  unique_visitors: number;
  unique_sessions: number;
}

export interface DailyAnalyticsPoint {
  date: string;
  page_views: number;
  unique_visitors: number;
}

export interface TopPageStat {
  path: string;
  page_views: number;
  unique_visitors: number;
}

export interface EventBreakdownStat {
  event_type: AnalyticsEventType;
  count: number;
  unique_visitors: number;
}

export interface AnalyticsSummary {
  enabled: boolean;
  window_days: number;
  totals: AnalyticsTotals;
  daily_page_views: DailyAnalyticsPoint[];
  top_pages: TopPageStat[];
  event_breakdown: EventBreakdownStat[];
}
