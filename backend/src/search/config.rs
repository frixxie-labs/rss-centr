use rss_centr_core::semantic::RecencyBoost;

pub const DEFAULT_EMBED_MODEL: &str = "embeddinggemma-2";
pub const DEFAULT_MIN_SIMILARITY: f32 = 0.2;
/// Keep the embedding model loaded between searches; loading it takes ~2 s.
pub const DEFAULT_EMBED_KEEP_ALIVE: &str = "1h";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchConfig {
    pub(crate) model: String,
    pub(crate) keep_alive: String,
    pub(crate) min_similarity: f32,
    pub(crate) recency: RecencyBoost,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_EMBED_MODEL.into(),
            keep_alive: DEFAULT_EMBED_KEEP_ALIVE.into(),
            min_similarity: DEFAULT_MIN_SIMILARITY,
            recency: RecencyBoost::default(),
        }
    }
}
