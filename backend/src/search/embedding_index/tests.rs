use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;

enum ReadStep {
    Embeddings {
        since: Option<DateTime<Utc>>,
        result: Result<Vec<StoredEmbedding>>,
    },
    Versions(Result<Vec<EmbeddingVersion>>),
    ByIds {
        ids: Vec<i64>,
        result: Result<Vec<StoredEmbedding>>,
    },
    Pending {
        since: Option<DateTime<Utc>>,
    },
}

struct ScriptedSource {
    steps: Mutex<VecDeque<ReadStep>>,
}

impl ScriptedSource {
    fn new(steps: Vec<ReadStep>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
        }
    }

    fn next(&self, model: &str) -> ReadStep {
        assert_eq!(model, "test-model");
        self.steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected source read")
    }
}

impl EmbeddingSource for ScriptedSource {
    async fn read_embeddings(
        &self,
        model: &str,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<StoredEmbedding>> {
        match self.next(model) {
            ReadStep::Embeddings {
                since: expected,
                result,
            } => {
                assert_eq!(since, expected);
                result
            }
            ReadStep::Pending { since: expected } => {
                assert_eq!(since, expected);
                std::future::pending().await
            }
            _ => panic!("expected a different kind of source read"),
        }
    }

    async fn read_embeddings_by_ids(
        &self,
        model: &str,
        ids: &[i64],
    ) -> Result<Vec<StoredEmbedding>> {
        let ReadStep::ByIds {
            ids: mut expected,
            result,
        } = self.next(model)
        else {
            panic!("expected a different kind of source read");
        };
        let mut actual = ids.to_vec();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
        result
    }

    async fn read_embedding_versions(&self, model: &str) -> Result<Vec<EmbeddingVersion>> {
        let ReadStep::Versions(result) = self.next(model) else {
            panic!("expected a different kind of source read");
        };
        result
    }
}

fn row(id: i64, vector: &[f32]) -> StoredEmbedding {
    StoredEmbedding {
        id,
        feed_id: 1,
        date: DateTime::UNIX_EPOCH,
        embedding: vector.to_vec(),
        embedded_at: DateTime::UNIX_EPOCH,
    }
}

fn version(id: i64) -> EmbeddingVersion {
    EmbeddingVersion {
        id,
        feed_id: 1,
        date: DateTime::UNIX_EPOCH,
        embedded_at: DateTime::UNIX_EPOCH,
    }
}

#[tokio::test]
async fn full_reload_drops_embeddings_of_deleted_items() {
    let source = ScriptedSource::new(vec![
        ReadStep::Embeddings {
            since: None,
            result: Ok(vec![row(1, &[1.0, 0.0]), row(2, &[1.0, 0.0])]),
        },
        ReadStep::Embeddings {
            since: Some(DateTime::UNIX_EPOCH - chrono::Duration::minutes(5)),
            result: Ok(Vec::new()),
        },
        ReadStep::Embeddings {
            since: None,
            result: Ok(vec![row(1, &[1.0, 0.0])]),
        },
    ]);
    let index = EmbeddingIndex::new("test-model");
    index.refresh(&source).await.unwrap();

    index.refresh(&source).await.unwrap();
    assert_eq!(
        index.item_count().await,
        2,
        "incremental refresh keeps stale items"
    );
    index.load_all(&source).await.unwrap();

    assert_eq!(index.similarities(&[1.0, 0.0], None).await, vec![(1, 1.0)]);
    assert_eq!(index.item_count().await, 1);
    assert!(source.steps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn refresh_uses_overlap_and_replaces_rows_without_duplicates_or_a_regressing_watermark() {
    let newer = DateTime::UNIX_EPOCH + chrono::Duration::hours(1);
    let source = ScriptedSource::new(vec![
        ReadStep::Embeddings {
            since: None,
            result: Ok(vec![row(1, &[1.0, 0.0])]),
        },
        ReadStep::Embeddings {
            since: Some(DateTime::UNIX_EPOCH - chrono::Duration::minutes(5)),
            result: Ok(vec![
                StoredEmbedding {
                    embedded_at: newer,
                    ..row(1, &[0.0, 1.0])
                },
                row(2, &[1.0, 0.0]),
            ]),
        },
        ReadStep::Embeddings {
            since: Some(newer - chrono::Duration::minutes(5)),
            result: Ok(vec![row(2, &[1.0, 0.0])]),
        },
        ReadStep::Embeddings {
            since: Some(newer - chrono::Duration::minutes(5)),
            result: Ok(Vec::new()),
        },
    ]);
    let index = EmbeddingIndex::new("test-model");

    for _ in 0..4 {
        index.refresh(&source).await.unwrap();
    }

    let mut scores = index.similarities(&[1.0, 0.0], None).await;
    scores.sort_by_key(|(id, _)| *id);
    assert_eq!(scores, vec![(1, 0.0), (2, 1.0)]);
    assert_eq!(index.item_count().await, 2);
    assert!(source.steps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_reconciliation_does_not_partially_apply_metadata_updates_or_deletions() {
    let changed_date = DateTime::UNIX_EPOCH + chrono::Duration::days(1);
    let versions = || {
        vec![
            EmbeddingVersion {
                date: changed_date,
                feed_id: 2,
                ..version(1)
            },
            version(3),
        ]
    };
    let source = ScriptedSource::new(vec![
        ReadStep::Embeddings {
            since: None,
            result: Ok(vec![row(1, &[1.0, 0.0]), row(2, &[1.0, 0.0])]),
        },
        ReadStep::Versions(Ok(versions())),
        ReadStep::ByIds {
            ids: vec![3],
            result: Err(anyhow::anyhow!("source unavailable")),
        },
        ReadStep::Versions(Ok(versions())),
        ReadStep::ByIds {
            ids: vec![3],
            result: Ok(vec![row(3, &[0.0, 1.0])]),
        },
    ]);
    let index = EmbeddingIndex::new("test-model");
    index.refresh(&source).await.unwrap();

    assert!(index.reconcile(&source).await.is_err());
    assert_eq!(index.similarities(&[1.0, 0.0], Some(1)).await.len(), 2);
    assert_eq!(index.dates([1, 2]).await[&1], DateTime::UNIX_EPOCH);

    let stats = index.reconcile(&source).await.unwrap();
    assert_eq!(
        stats,
        ReconcileStats {
            removed: 1,
            fetched: 1
        }
    );
    assert_eq!(
        index.dates([1, 2, 3]).await,
        HashMap::from([(1, changed_date), (3, DateTime::UNIX_EPOCH)])
    );
    assert_eq!(
        index.similarities(&[1.0, 0.0], Some(2)).await,
        vec![(1, 1.0)]
    );
    assert_eq!(
        index.similarities(&[1.0, 0.0], Some(1)).await,
        vec![(3, 0.0)]
    );
    assert!(source.steps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn degenerate_vectors_and_dimension_mismatches_are_skipped_when_scoring() {
    let source = ScriptedSource::new(vec![ReadStep::Embeddings {
        since: None,
        result: Ok(vec![
            row(1, &[1.0, 0.0]),
            row(2, &[1.0, 0.0, 0.0]),
            row(3, &[0.0, 0.0]),
            row(4, &[f32::NAN, 0.0]),
        ]),
    }]);
    let index = EmbeddingIndex::new("test-model");

    index.refresh(&source).await.unwrap();

    assert_eq!(index.item_count().await, 2);
    assert_eq!(index.similarities(&[1.0, 0.0], None).await, vec![(1, 1.0)]);
    assert!(index.similarities(&[0.0, 0.0], None).await.is_empty());
    assert!(index.similarities(&[f32::NAN, 0.0], None).await.is_empty());
    assert_eq!(
        index.dates([1, 3, i64::MAX]).await,
        HashMap::from([(1, DateTime::UNIX_EPOCH)])
    );
}

#[tokio::test(start_paused = true)]
async fn loader_retries_failed_loads_and_reconciliations_without_clearing_the_index() {
    let source = ScriptedSource::new(vec![
        ReadStep::Embeddings {
            since: None,
            result: Err(anyhow::anyhow!("initial read failed")),
        },
        ReadStep::Embeddings {
            since: None,
            result: Ok(vec![row(1, &[1.0, 0.0])]),
        },
        ReadStep::Versions(Err(anyhow::anyhow!("reconciliation failed"))),
        ReadStep::Versions(Ok(vec![version(1)])),
    ]);
    let index = EmbeddingIndex::new("test-model");
    let deadline = LOAD_RETRY_INTERVAL * 2 + RECONCILE_INTERVAL + Duration::from_secs(1);

    let result = tokio::time::timeout(deadline, index.run_loader(&source)).await;

    assert!(result.is_err(), "the loader keeps running until cancelled");
    assert!(source.steps.lock().unwrap().is_empty());
    assert_eq!(index.similarities(&[1.0, 0.0], None).await, vec![(1, 1.0)]);
}

#[tokio::test(start_paused = true)]
async fn cancelled_refresh_preserves_the_index_and_releases_the_refresh_lock() {
    let since = Some(DateTime::UNIX_EPOCH - chrono::Duration::minutes(5));
    let source = ScriptedSource::new(vec![
        ReadStep::Embeddings {
            since: None,
            result: Ok(vec![row(1, &[1.0, 0.0])]),
        },
        ReadStep::Pending { since },
        ReadStep::Embeddings {
            since,
            result: Ok(vec![row(2, &[0.0, 1.0])]),
        },
    ]);
    let index = EmbeddingIndex::new("test-model");
    index.refresh(&source).await.unwrap();

    assert!(
        tokio::time::timeout(Duration::from_secs(1), index.refresh(&source))
            .await
            .is_err()
    );
    assert_eq!(index.similarities(&[1.0, 0.0], None).await, vec![(1, 1.0)]);

    tokio::time::timeout(Duration::from_secs(1), index.refresh(&source))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(index.item_count().await, 2);
    assert!(source.steps.lock().unwrap().is_empty());
}

/// An operation on [`IndexedItems`]; small ids make repeats likely.
#[derive(Debug, Clone)]
enum Op {
    Upsert(u8, i8),
    Drop(u8),
}

impl quickcheck::Arbitrary for Op {
    fn arbitrary(g: &mut quickcheck::Gen) -> Self {
        if bool::arbitrary(g) {
            Self::Upsert(u8::arbitrary(g) % 16, i8::arbitrary(g))
        } else {
            Self::Drop(u8::arbitrary(g) % 16)
        }
    }
}

fn apply(ops: &[Op]) -> IndexedItems {
    let mut items = IndexedItems::default();
    for op in ops {
        match *op {
            Op::Upsert(id, x) => items.upsert(row(i64::from(id), &[f32::from(x), 1.0])),
            Op::Drop(id) => items.retain(|item| item.id != i64::from(id)),
        }
    }
    items
}

quickcheck::quickcheck! {
    fn prop_positions_always_point_at_their_item(ops: Vec<Op>) -> bool {
        let items = apply(&ops);
        items.positions.len() == items.items.len()
            && items.positions.iter().all(|(id, position)| items.items[*position].id == *id)
    }

    fn prop_reconcile_converges_to_the_stored_versions(ops: Vec<Op>, stored: Vec<(u8, bool)>) -> bool {
        let mut items = apply(&ops);
        let versions: HashMap<i64, EmbeddingVersion> = stored
            .iter()
            .map(|(id, newer)| {
                let id = i64::from(id % 16);
                let embedded_at = if *newer { DateTime::UNIX_EPOCH + chrono::Duration::hours(1) } else { DateTime::UNIX_EPOCH };
                (id, EmbeddingVersion { feed_id: 2, embedded_at, ..version(id) })
            })
            .collect();

        let outdated = items.outdated(&versions);
        items.apply_versions(&versions);
        items.extend(outdated.iter().map(|id| StoredEmbedding {
            feed_id: versions[id].feed_id,
            embedded_at: versions[id].embedded_at,
            ..row(*id, &[1.0])
        }));

        let mut ids: Vec<i64> = items.iter().map(|item| item.id).collect();
        let mut expected: Vec<i64> = versions.keys().copied().collect();
        ids.sort_unstable();
        expected.sort_unstable();
        ids == expected
            && items.iter().all(|item| item.feed_id == 2 && item.embedded_at == versions[&item.id].embedded_at)
            && items.outdated(&versions).is_empty()
    }
}
