use std::sync::{Arc, Mutex};

use super::*;
use crate::summary::{ContextBudget, Generated, SummaryArticle};

type Events = Arc<Mutex<Vec<&'static str>>>;

#[derive(Clone)]
struct RecordingRepository {
    events: Events,
    has_articles: bool,
    saved: Arc<Mutex<Option<DailySummary>>>,
}

impl SummaryRepository for RecordingRepository {
    async fn latest_summary(&self) -> Result<Option<DailySummary>, SummaryStoreError> {
        Ok(self.saved.lock().unwrap().clone())
    }

    async fn fetch_articles(&self) -> Result<Vec<SummaryArticle>, SummaryStoreError> {
        self.events.lock().unwrap().push("articles");
        Ok(if self.has_articles {
            vec![SummaryArticle {
                id: 10,
                feed_id: 1,
                title: "News".into(),
                feed: "Example".into(),
                description: "Description".into(),
            }]
        } else {
            Vec::new()
        })
    }

    async fn claim_refresh(&self, _force: bool) -> Result<Option<String>, SummaryStoreError> {
        panic!("the job must use its existing lease")
    }

    async fn next_due_at(&self) -> Result<DateTime<Utc>, SummaryStoreError> {
        panic!("the job must not read the schedule")
    }

    async fn release_refresh(&self, token: &str) -> Result<(), SummaryStoreError> {
        assert_eq!(token, "lease");
        self.events.lock().unwrap().push("release");
        Ok(())
    }

    async fn complete_refresh(
        &self,
        token: &str,
        summary: &GeneratedSummary,
    ) -> Result<DailySummary, SummaryStoreError> {
        assert_eq!(token, "lease");
        self.events.lock().unwrap().push("save");
        let saved = DailySummary {
            id: 1,
            summary: summary.summary.clone(),
            feed_ids: summary.feed_ids.clone(),
            feed_item_ids: summary.feed_item_ids.clone(),
            generated_at: DateTime::UNIX_EPOCH,
            model: summary.model.clone(),
            generation_ms: Some(summary.generation_ms),
            llm_calls: Some(summary.usage.llm_calls),
            prompt_tokens: Some(summary.usage.prompt_tokens),
            output_tokens: Some(summary.usage.output_tokens),
            llm_ms: Some(summary.usage.llm_ms),
            eval_ms: Some(summary.usage.eval_ms),
        };
        *self.saved.lock().unwrap() = Some(saved.clone());
        Ok(saved)
    }
}

#[derive(Clone, Copy)]
enum Scenario {
    Success,
    MissingFinalModel,
    FailedBatch,
    Pending,
}

struct RecordingGenerator {
    events: Events,
    scenario: Scenario,
}

impl SummaryGenerator for RecordingGenerator {
    async fn context_budget(&self, model: &str) -> Result<ContextBudget, SummaryError> {
        let stage = match model {
            "batch" => "batch budget",
            "final" => "final budget",
            _ => panic!("unexpected model: {model}"),
        };
        self.events.lock().unwrap().push(stage);
        if model == "final" && matches!(self.scenario, Scenario::MissingFinalModel) {
            return Err(SummaryError::ModelUnavailable);
        }
        Ok(ContextBudget { num_ctx: 16384 })
    }

    async fn generate(
        &self,
        model: &str,
        _budget: ContextBudget,
        _prompt: String,
        _num_predict: u32,
    ) -> Result<Generated, SummaryError> {
        let stage = match model {
            "batch" => "batch",
            "final" => "final",
            _ => panic!("unexpected model: {model}"),
        };
        self.events.lock().unwrap().push(stage);
        match self.scenario {
            Scenario::FailedBatch => Err(SummaryError::Truncated),
            Scenario::Pending => std::future::pending().await,
            Scenario::Success | Scenario::MissingFinalModel => Ok(Generated {
                text: if model == "batch" {
                    "Notes"
                } else {
                    "Overview"
                }
                .into(),
                usage: SummaryUsage {
                    llm_calls: 1,
                    ..SummaryUsage::default()
                },
            }),
        }
    }
}

#[tokio::test]
async fn successful_job_checks_models_then_generates_and_saves_without_releasing() {
    let events = Events::default();
    let state = SummaryState {
        repository: RecordingRepository {
            events: events.clone(),
            has_articles: true,
            saved: Default::default(),
        },
        generator: RecordingGenerator {
            events: events.clone(),
            scenario: Scenario::Success,
        },
        models: SummaryModels {
            batch: "batch".into(),
            final_summary: "final".into(),
        },
    };

    state.run_refresh("lease").await;

    assert_eq!(
        *events.lock().unwrap(),
        [
            "articles",
            "batch budget",
            "final budget",
            "batch",
            "final",
            "save"
        ]
    );
    let saved = state.latest_summary().await.unwrap().unwrap();
    assert_eq!(saved.summary, "Overview");
    assert_eq!(saved.feed_ids, [1]);
    assert_eq!(saved.feed_item_ids, [10]);
    assert_eq!(saved.model, "batch → final");
    assert_eq!(saved.llm_calls, Some(2));
}

#[tokio::test]
async fn failed_generation_releases_the_lease_without_saving_or_running_later_stages() {
    for (scenario, expected) in [
        (
            Scenario::MissingFinalModel,
            vec!["articles", "batch budget", "final budget", "release"],
        ),
        (
            Scenario::FailedBatch,
            vec![
                "articles",
                "batch budget",
                "final budget",
                "batch",
                "release",
            ],
        ),
    ] {
        let events = Events::default();
        let state = SummaryState {
            repository: RecordingRepository {
                events: events.clone(),
                has_articles: true,
                saved: Default::default(),
            },
            generator: RecordingGenerator {
                events: events.clone(),
                scenario,
            },
            models: SummaryModels {
                batch: "batch".into(),
                final_summary: "final".into(),
            },
        };

        state.run_refresh("lease").await;

        assert_eq!(*events.lock().unwrap(), expected);
        assert!(state.latest_summary().await.unwrap().is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn job_timeout_cancels_generation_and_releases_the_lease_without_saving() {
    let events = Events::default();
    let state = SummaryState {
        repository: RecordingRepository {
            events: events.clone(),
            has_articles: true,
            saved: Default::default(),
        },
        generator: RecordingGenerator {
            events: events.clone(),
            scenario: Scenario::Pending,
        },
        models: SummaryModels {
            batch: "batch".into(),
            final_summary: "final".into(),
        },
    };
    let started = tokio::time::Instant::now();

    state.run_refresh("lease").await;

    assert_eq!(started.elapsed(), GENERATION_TIMEOUT);
    assert_eq!(
        *events.lock().unwrap(),
        [
            "articles",
            "batch budget",
            "final budget",
            "batch",
            "release"
        ]
    );
    assert!(state.latest_summary().await.unwrap().is_none());
}

#[tokio::test]
async fn empty_articles_release_the_lease_without_calling_the_generator() {
    let events = Events::default();
    let state = SummaryState {
        repository: RecordingRepository {
            events: events.clone(),
            has_articles: false,
            saved: Default::default(),
        },
        generator: RecordingGenerator {
            events: events.clone(),
            scenario: Scenario::Success,
        },
        models: SummaryModels {
            batch: "batch".into(),
            final_summary: "final".into(),
        },
    };

    state.run_refresh("lease").await;

    assert_eq!(*events.lock().unwrap(), ["articles", "release"]);
    assert!(state.latest_summary().await.unwrap().is_none());
}
