//! Pure view logic shared by server-rendered pages and islands.

mod filters;
mod format;
mod items;
mod timeline;

pub use filters::{NewsFilters, Order, SearchMode, topic_url};
pub use format::{
    absolute_date, compact_text, hostname, poll_interval, preview, time_ago, truncate,
};
pub use items::{
    FeedNames, PREVIEW_CHARS, append_page, display_order, effective_date, feed_name, feed_names,
    merge_items, slim_item, sort_newest, timestamp,
};
pub use timeline::{ReceiveOutcome, ReplayOutcome, TimelineState, resume_cursor};

pub const PAGE_SIZE: usize = 50;
pub const CURSOR_KEY: &str = "rss:last-event-id";
