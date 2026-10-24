use std::{error::Error, fmt};

use super::{InputTooLarge, SummaryStoreError};

#[derive(Debug)]
pub(crate) enum SummaryError {
    Store(SummaryStoreError),
    InputTooLarge(InputTooLarge),
    NoArticles,
    MergeBudgetTooSmall,
    ModelUnavailable,
    InvalidModelMetadata,
    MissingContextLimit,
    ServiceUnavailable,
    InvalidResponse,
    ContextOverflow,
    Truncated,
    Incomplete,
    EmptyResponse,
}

impl fmt::Display for SummaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(f),
            Self::InputTooLarge(error) => error.fmt(f),
            Self::NoArticles => f.write_str("No recent articles available for summary generation."),
            Self::MergeBudgetTooSmall => {
                f.write_str("Summary context budget too small to merge summaries")
            }
            Self::ModelUnavailable => f.write_str("Summary model unavailable"),
            Self::InvalidModelMetadata => f.write_str("Invalid summary model metadata"),
            Self::MissingContextLimit => f.write_str("Summary model context limit missing"),
            Self::ServiceUnavailable => f.write_str("Summary service unavailable"),
            Self::InvalidResponse => f.write_str("Invalid summary response"),
            Self::ContextOverflow => f.write_str("Summary generation exceeded context budget"),
            Self::Truncated => f.write_str("Summary generation truncated"),
            Self::Incomplete => f.write_str("Summary generation incomplete"),
            Self::EmptyResponse => f.write_str("Empty summary response"),
        }
    }
}

impl Error for SummaryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::InputTooLarge(error) => Some(error),
            _ => None,
        }
    }
}

impl From<SummaryStoreError> for SummaryError {
    fn from(error: SummaryStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<InputTooLarge> for SummaryError {
    fn from(error: InputTooLarge) -> Self {
        Self::InputTooLarge(error)
    }
}
