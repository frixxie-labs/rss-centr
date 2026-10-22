use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Response of `POST /internal/items/summary/refresh`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SummaryRefreshResponse {
    /// Earliest time a refresh can be claimed again.
    pub next_due_at: DateTime<Utc>,
}
