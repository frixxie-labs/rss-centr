use rss_centr_core::semantic::RecencyBoost;

use crate::{search::SearchConfig, summary::SummaryModels};

const DEFAULT_OLLAMA_URL: &str = "http://desktop:11434";

#[derive(Debug)]
pub(crate) struct BackendConfig {
    pub(crate) ollama_url: String,
    pub(crate) search: SearchConfig,
    pub(crate) summary_models: SummaryModels,
}

impl BackendConfig {
    /// Startup supplies the environment lookup; tests supply immutable values.
    pub(crate) fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = SearchConfig::default();
        let number = |name: &str, fallback: f32| {
            lookup(name)
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(fallback)
        };
        Self {
            ollama_url: lookup("OLLAMA_URL").unwrap_or_else(|| DEFAULT_OLLAMA_URL.into()),
            search: SearchConfig {
                model: lookup("OLLAMA_EMBED_MODEL").unwrap_or(defaults.model),
                keep_alive: lookup("OLLAMA_EMBED_KEEP_ALIVE").unwrap_or(defaults.keep_alive),
                min_similarity: number("SEMANTIC_MIN_SIMILARITY", defaults.min_similarity),
                recency: RecencyBoost {
                    weight: number("SEMANTIC_RECENCY_WEIGHT", defaults.recency.weight),
                    half_life_hours: number(
                        "SEMANTIC_RECENCY_HALF_LIFE_HOURS",
                        defaults.recency.half_life_hours,
                    ),
                },
            },
            summary_models: SummaryModels::from_overrides(
                lookup("OLLAMA_MODEL"),
                lookup("OLLAMA_BATCH_MODEL"),
                lookup("OLLAMA_FINAL_MODEL"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn missing_settings_keep_existing_defaults() {
        let config = BackendConfig::from_lookup(|_| None);
        assert_eq!(config.ollama_url, "http://desktop:11434");
        assert_eq!(config.search.model, "embeddinggemma-2");
        assert_eq!(config.search.keep_alive, "1h");
        assert_eq!(config.search.min_similarity, 0.2);
        assert_eq!(
            config.search.recency,
            RecencyBoost {
                weight: 0.3,
                half_life_hours: 168.0
            }
        );
        assert_eq!(config.summary_models.batch, "gemma4:e4b");
        assert_eq!(config.summary_models.final_summary, "gemma4:e4b");
    }

    #[test]
    fn overrides_are_applied_independently_and_stage_models_override_the_legacy_model() {
        let values = HashMap::from([
            ("OLLAMA_URL", "http://ollama.example:11434/"),
            ("OLLAMA_EMBED_MODEL", "embed-model"),
            ("OLLAMA_EMBED_KEEP_ALIVE", "30m"),
            ("SEMANTIC_MIN_SIMILARITY", "0.4"),
            ("SEMANTIC_RECENCY_WEIGHT", "0.75"),
            ("SEMANTIC_RECENCY_HALF_LIFE_HOURS", "24"),
            ("OLLAMA_MODEL", "legacy"),
            ("OLLAMA_BATCH_MODEL", "fast"),
            ("OLLAMA_FINAL_MODEL", "quality"),
        ]);

        let config =
            BackendConfig::from_lookup(|name| values.get(name).map(|value| (*value).into()));

        assert_eq!(config.ollama_url, "http://ollama.example:11434/");
        assert_eq!(config.search.model, "embed-model");
        assert_eq!(config.search.keep_alive, "30m");
        assert_eq!(config.search.min_similarity, 0.4);
        assert_eq!(
            config.search.recency,
            RecencyBoost {
                weight: 0.75,
                half_life_hours: 24.0
            }
        );
        assert_eq!(config.summary_models.batch, "fast");
        assert_eq!(config.summary_models.final_summary, "quality");
    }

    #[test]
    fn invalid_numbers_fall_back_per_setting_without_discarding_valid_overrides() {
        let values = HashMap::from([
            ("SEMANTIC_MIN_SIMILARITY", "invalid"),
            ("SEMANTIC_RECENCY_WEIGHT", "0"),
            ("SEMANTIC_RECENCY_HALF_LIFE_HOURS", ""),
            ("OLLAMA_MODEL", "single-model"),
        ]);

        let config =
            BackendConfig::from_lookup(|name| values.get(name).map(|value| (*value).into()));

        assert_eq!(config.search.min_similarity, 0.2);
        assert_eq!(
            config.search.recency,
            RecencyBoost {
                weight: 0.0,
                half_life_hours: 168.0
            }
        );
        assert_eq!(config.summary_models.batch, "single-model");
        assert_eq!(config.summary_models.final_summary, "single-model");
    }

    #[test]
    fn empty_string_overrides_are_preserved_rather_than_replaced_with_defaults() {
        let config = BackendConfig::from_lookup(|name| match name {
            "OLLAMA_URL" | "OLLAMA_EMBED_MODEL" | "OLLAMA_EMBED_KEEP_ALIVE" | "OLLAMA_MODEL" => {
                Some(String::new())
            }
            _ => None,
        });

        assert_eq!(config.ollama_url, "");
        assert_eq!(config.search.model, "");
        assert_eq!(config.search.keep_alive, "");
        assert_eq!(config.summary_models.batch, "");
        assert_eq!(config.summary_models.final_summary, "");
    }
}
