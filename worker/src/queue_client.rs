use anyhow::{Context, Result, anyhow};
use rss_centr_core::feed_update_queue::{
    CompleteFeedUpdateRequest, CompleteFeedUpdateResult, DequeuedFeedUpdate,
    FailedFeedUpdateRequest, FailedFeedUpdateResult,
};

#[derive(Clone)]
pub(crate) struct QueueClient {
    base_url: String,
    http: reqwest::Client,
}

impl QueueClient {
    pub(crate) fn new(base_url: String, http: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    pub(crate) async fn dequeue(
        &self,
        limit: i64,
        lease_seconds: i64,
    ) -> Result<Vec<DequeuedFeedUpdate>> {
        let url = format!(
            "{}/internal/feed-update-queue/dequeue?limit={limit}&lease_seconds={lease_seconds}",
            self.base_url
        );
        let response = self
            .http
            .post(url)
            .send()
            .await
            .context("failed to call dequeue endpoint")?;
        if !response.status().is_success() {
            return Err(response_error("dequeue feed updates", response).await);
        }

        response
            .json::<Vec<DequeuedFeedUpdate>>()
            .await
            .context("failed to decode dequeue response")
    }

    pub(crate) async fn complete(
        &self,
        feed_id: i64,
        request: CompleteFeedUpdateRequest,
    ) -> Result<CompleteFeedUpdateResult> {
        let url = format!(
            "{}/internal/feed-update-queue/{feed_id}/complete",
            self.base_url
        );
        let response = self
            .http
            .post(url)
            .json(&request)
            .send()
            .await
            .with_context(|| format!("failed to call complete endpoint for feed_id={feed_id}"))?;
        if !response.status().is_success() {
            return Err(response_error("complete feed update", response).await);
        }

        response
            .json::<CompleteFeedUpdateResult>()
            .await
            .with_context(|| format!("failed to decode complete response for feed_id={feed_id}"))
    }

    pub(crate) async fn failed(
        &self,
        feed_id: i64,
        request: FailedFeedUpdateRequest,
    ) -> Result<FailedFeedUpdateResult> {
        let url = format!(
            "{}/internal/feed-update-queue/{feed_id}/failed",
            self.base_url
        );
        let response = self
            .http
            .post(url)
            .json(&request)
            .send()
            .await
            .with_context(|| format!("failed to call failed endpoint for feed_id={feed_id}"))?;
        if !response.status().is_success() {
            return Err(response_error("record feed update failure", response).await);
        }

        response
            .json::<FailedFeedUpdateResult>()
            .await
            .with_context(|| format!("failed to decode failed response for feed_id={feed_id}"))
    }
}

async fn response_error(action: &str, response: reqwest::Response) -> anyhow::Error {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    anyhow!("{action} failed with status {status}: {body}")
}
