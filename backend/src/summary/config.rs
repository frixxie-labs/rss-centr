const DEFAULT_MODEL: &str = "gemma4:e4b";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SummaryModels {
    pub(crate) batch: String,
    pub(crate) final_summary: String,
}

impl Default for SummaryModels {
    fn default() -> Self {
        Self::from_overrides(None, None, None)
    }
}

impl SummaryModels {
    pub(crate) fn from_overrides(
        legacy: Option<String>,
        batch: Option<String>,
        final_summary: Option<String>,
    ) -> Self {
        Self {
            batch: batch
                .or_else(|| legacy.clone())
                .unwrap_or_else(|| DEFAULT_MODEL.into()),
            final_summary: final_summary
                .or(legacy)
                .unwrap_or_else(|| DEFAULT_MODEL.into()),
        }
    }

    pub(crate) fn label(&self) -> String {
        if self.batch == self.final_summary {
            self.final_summary.clone()
        } else {
            format!("{} → {}", self.batch, self.final_summary)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_selection_defaults_to_gemma_for_both_stages_and_preserves_overrides() {
        for (legacy, batch, final_summary, expected_batch, expected_final) in [
            (None, None, None, "gemma4:e4b", "gemma4:e4b"),
            (Some("single"), None, None, "single", "single"),
            (None, Some("fast"), None, "fast", "gemma4:e4b"),
            (None, None, Some("quality"), "gemma4:e4b", "quality"),
            (Some("single"), Some("fast"), None, "fast", "single"),
            (Some("single"), None, Some("quality"), "single", "quality"),
            (
                Some("single"),
                Some("fast"),
                Some("quality"),
                "fast",
                "quality",
            ),
        ] {
            let models = SummaryModels::from_overrides(
                legacy.map(str::to_string),
                batch.map(str::to_string),
                final_summary.map(str::to_string),
            );
            assert_eq!(models.batch, expected_batch);
            assert_eq!(models.final_summary, expected_final);
            assert_eq!(
                models.label(),
                if expected_batch == expected_final {
                    expected_final.to_string()
                } else {
                    format!("{expected_batch} → {expected_final}")
                }
            );
        }
    }
}
