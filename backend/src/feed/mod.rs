pub mod cadence;
pub mod feed_item;
pub mod feed_item_detail;
pub mod feed_subscription;
pub mod feed_title_index;
pub mod feed_update_queue;
pub mod poll_interval;

/// A requested feed record does not exist.
#[derive(Debug)]
pub struct NotFound(String);

impl NotFound {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotFound {}
