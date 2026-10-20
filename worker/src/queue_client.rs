use std::future::Future;

use anyhow::Result;
use rss_centr_core::feed_update_queue::{
    CompleteFeedUpdateRequest, CompleteFeedUpdateResult, DequeuedFeedUpdate,
    FailedFeedUpdateRequest, FailedFeedUpdateResult,
};

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
