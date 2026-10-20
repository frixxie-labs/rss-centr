mod news;
mod sources;
mod summary;
mod timeline;
mod topics;

#[cfg(not(target_arch = "wasm32"))]
pub use news::NewsView;
#[cfg(not(target_arch = "wasm32"))]
pub use sources::SourcesView;
#[cfg(not(target_arch = "wasm32"))]
pub use summary::SummaryControls;
#[cfg(not(target_arch = "wasm32"))]
pub use timeline::TimelineView;
#[cfg(not(target_arch = "wasm32"))]
pub use topics::TopicsView;
