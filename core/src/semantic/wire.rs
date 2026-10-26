//! Request and response payloads exchanged between the backend and worker.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EmbeddingJob {
    pub feed_item_id: i64,
    pub input: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ClaimEmbeddingsResponse {
    pub model: String,
    pub jobs: Vec<EmbeddingJob>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ItemEmbedding {
    pub feed_item_id: i64,
    pub content_hash: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StoreEmbeddingsRequest {
    pub model: String,
    pub embeddings: Vec<ItemEmbedding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StoreEmbeddingsResult {
    pub stored: u64,
}
