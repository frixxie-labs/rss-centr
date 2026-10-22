use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use rss_centr_core::feed_update_queue::{
    CompleteFeedUpdateRequest, CompleteFeedUpdateResult, DequeuedFeedUpdate,
    FailedFeedUpdateRequest, FailedFeedUpdateResult,
};
use tokio::sync::Notify;

use crate::http_util::send_json;

/// Backend feed update queue, abstracted so the runner can be tested.
pub(crate) trait FeedQueue: Send + Sync + 'static {
    fn dequeue(
        &self,
        limit: u32,
        lease_seconds: u32,
    ) -> impl Future<Output = Result<Vec<DequeuedFeedUpdate>>> + Send;

    fn complete(
        &self,
        feed_id: i64,
        request: CompleteFeedUpdateRequest,
    ) -> impl Future<Output = Result<CompleteFeedUpdateResult>> + Send;

    fn failed(
        &self,
        feed_id: i64,
        request: FailedFeedUpdateRequest,
    ) -> impl Future<Output = Result<FailedFeedUpdateResult>> + Send;
}

#[derive(Clone)]
pub(crate) struct QueueClient {
    base_url: String,
    http: reqwest::Client,
}

impl QueueClient {
    pub(crate) fn new(base_url: &str, http: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        }
    }
}

impl FeedQueue for QueueClient {
    async fn dequeue(&self, limit: u32, lease_seconds: u32) -> Result<Vec<DequeuedFeedUpdate>> {
        let url = format!(
            "{}/internal/feed-update-queue/dequeue?limit={limit}&lease_seconds={lease_seconds}",
            self.base_url
        );
        send_json(self.http.post(url), "dequeue feed updates").await
    }

    async fn complete(
        &self,
        feed_id: i64,
        request: CompleteFeedUpdateRequest,
    ) -> Result<CompleteFeedUpdateResult> {
        let url = format!(
            "{}/internal/feed-update-queue/{feed_id}/complete",
            self.base_url
        );
        send_json(
            self.http.post(url).json(&request),
            &format!("complete feed update for feed_id={feed_id}"),
        )
        .await
    }

    async fn failed(
        &self,
        feed_id: i64,
        request: FailedFeedUpdateRequest,
    ) -> Result<FailedFeedUpdateResult> {
        let url = format!(
            "{}/internal/feed-update-queue/{feed_id}/failed",
            self.base_url
        );
        send_json(
            self.http.post(url).json(&request),
            &format!("record feed update failure for feed_id={feed_id}"),
        )
        .await
    }
}

/// Wraps a [`FeedQueue`] and signals `new_items` whenever a completed feed
/// update inserted items, so they can be embedded right away.
pub(crate) struct NotifyOnInsert<Q> {
    inner: Q,
    new_items: Arc<Notify>,
}

impl<Q> NotifyOnInsert<Q> {
    pub(crate) fn new(inner: Q, new_items: Arc<Notify>) -> Self {
        Self { inner, new_items }
    }
}

impl<Q: FeedQueue> FeedQueue for NotifyOnInsert<Q> {
    async fn dequeue(&self, limit: u32, lease_seconds: u32) -> Result<Vec<DequeuedFeedUpdate>> {
        self.inner.dequeue(limit, lease_seconds).await
    }

    async fn complete(
        &self,
        feed_id: i64,
        request: CompleteFeedUpdateRequest,
    ) -> Result<CompleteFeedUpdateResult> {
        let result = self.inner.complete(feed_id, request).await?;
        if result.inserted_items > 0 {
            self.new_items.notify_one();
        }
        Ok(result)
    }

    async fn failed(
        &self,
        feed_id: i64,
        request: FailedFeedUpdateRequest,
    ) -> Result<FailedFeedUpdateResult> {
        self.inner.failed(feed_id, request).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::Utc;

    use super::*;

    struct InsertingQueue {
        inserted_items: i64,
    }

    impl FeedQueue for InsertingQueue {
        async fn dequeue(&self, _: u32, _: u32) -> Result<Vec<DequeuedFeedUpdate>> {
            Ok(Vec::new())
        }

        async fn complete(
            &self,
            _: i64,
            _: CompleteFeedUpdateRequest,
        ) -> Result<CompleteFeedUpdateResult> {
            Ok(CompleteFeedUpdateResult {
                inserted_items: self.inserted_items,
                next_due_at: Utc::now(),
            })
        }

        async fn failed(
            &self,
            _: i64,
            _: FailedFeedUpdateRequest,
        ) -> Result<FailedFeedUpdateResult> {
            anyhow::bail!("unused")
        }
    }

    fn request() -> CompleteFeedUpdateRequest {
        CompleteFeedUpdateRequest {
            lease_token: "token".to_string(),
            fetched: true,
            title: None,
            site_url: None,
            etag: None,
            last_modified: None,
            items: Vec::new(),
        }
    }

    async fn notified_after_complete(inserted_items: i64) -> bool {
        let new_items = Arc::new(Notify::new());
        let queue = NotifyOnInsert::new(InsertingQueue { inserted_items }, new_items.clone());
        queue.complete(1, request()).await.unwrap();
        tokio::time::timeout(Duration::from_millis(10), new_items.notified())
            .await
            .is_ok()
    }

    #[tokio::test(start_paused = true)]
    async fn notifies_when_items_were_inserted() {
        assert!(notified_after_complete(2).await);
    }

    #[tokio::test(start_paused = true)]
    async fn stays_quiet_without_new_items() {
        assert!(!notified_after_complete(0).await);
    }
}
