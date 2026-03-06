#[allow(clippy::module_inception)]
pub mod feed;
pub mod feed_item;
pub mod feed_subscription;
pub mod ingest;

pub use ingest::ingest_feed_url;
