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
