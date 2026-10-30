use rss_centr_core::semantic::{ItemEmbedding, normalize};

use super::*;
use crate::feed::feed_item::insert_feed_item;
use crate::feed::feed_subscription::upsert_feed_by_url;
use crate::search::embedding_store::store_item_embeddings;

fn centroid(feed_id: i64, vectors: &[Vec<f32>]) -> FeedCentroid {
    let mut sum = vec![0.0; vectors[0].len()];
    for vector in vectors {
        sum.iter_mut().zip(vector).for_each(|(s, v)| *s += v);
    }
    FeedCentroid {
        feed_id,
        sum,
        count: vectors.len() as i64,
    }
}

#[test]
fn mean_similarity_filters_by_feed_and_dimension() {
    let centroids = Centroids::new(vec![
        centroid(1, &[vec![1.0, 0.0], vec![0.0, 1.0]]),
        centroid(2, &[vec![1.0, 0.0]]),
        centroid(2, &[vec![1.0, 0.0, 0.0]]),
    ]);
    let query = [1.0, 0.0];

    assert_eq!(centroids.mean_similarity(&query, Some(1)), Some(0.5));
    assert_eq!(centroids.mean_similarity(&query, Some(2)), Some(1.0));
    assert!((centroids.mean_similarity(&query, None).unwrap() - 2.0 / 3.0).abs() < 1e-6);
    assert_eq!(centroids.mean_similarity(&query, Some(3)), None);
    assert_eq!(centroids.mean_similarity(&[1.0], None), None);
    assert_eq!(centroids.count(), 4);
}

quickcheck::quickcheck! {
    fn prop_mean_similarity_is_the_mean_of_individual_similarities(
        query: (i8, i8, i8),
        items: Vec<(bool, i8, i8, i8)>
    ) -> bool {
        let unit = |(x, y, z): (i8, i8, i8)| normalize(&[f32::from(x), f32::from(y), f32::from(z)]);
        let Some(query) = unit(query) else { return true };
        let items: Vec<(i64, Vec<f32>)> = items
            .into_iter()
            .filter_map(|(feed, x, y, z)| unit((x, y, z)).map(|v| (i64::from(feed), v)))
            .collect();
        let centroids = Centroids::new(
            [0, 1]
                .into_iter()
                .filter_map(|feed| {
                    let vectors: Vec<Vec<f32>> = items
                        .iter()
                        .filter(|(f, _)| *f == feed)
                        .map(|(_, v)| v.clone())
                        .collect();
                    (!vectors.is_empty()).then(|| centroid(feed, &vectors))
                })
                .collect(),
        );
        let expected = rss_centr_core::semantic::mean_similarity(
            items.iter().map(|(_, v)| dot(&query, v)),
        );
        match (centroids.mean_similarity(&query, None), expected) {
            (Some(actual), Some(expected)) => (actual - expected).abs() < 1e-4,
            (actual, expected) => actual == expected,
        }
    }
}

#[sqlx::test]
async fn centroids_sum_each_feeds_embeddings_of_the_model(pool: PgPool) {
    let mut ids = Vec::new();
    for (url, key) in [("https://a", "1"), ("https://a", "2"), ("https://b", "3")] {
        let feed = upsert_feed_by_url(&pool, url).await.unwrap();
        let item = insert_feed_item(&pool, feed.id, key, "t", "https://x")
            .await
            .unwrap();
        ids.push((feed.id, item.id));
    }
    let embedding = |id, embedding: Vec<f32>| ItemEmbedding {
        feed_item_id: id,
        content_hash: "h".into(),
        embedding,
    };
    store_item_embeddings(
        &pool,
        "model",
        &[
            embedding(ids[0].1, vec![1.0, 0.0]),
            embedding(ids[1].1, vec![0.0, 1.0]),
        ],
    )
    .await
    .unwrap();
    store_item_embeddings(&pool, "other", &[embedding(ids[2].1, vec![1.0, 0.0])])
        .await
        .unwrap();

    let centroids = EmbeddingCentroids::new("model");
    let loaded = centroids.get(&pool).await.unwrap();

    assert_eq!(loaded.count(), 2);
    assert_eq!(
        loaded.mean_similarity(&[1.0, 0.0], Some(ids[0].0)),
        Some(0.5)
    );
    assert_eq!(loaded.mean_similarity(&[1.0, 0.0], Some(ids[2].0)), None);

    store_item_embeddings(&pool, "model", &[embedding(ids[2].1, vec![1.0, 0.0])])
        .await
        .unwrap();
    assert_eq!(
        centroids.get(&pool).await.unwrap().count(),
        2,
        "cached until reloaded"
    );
    assert_eq!(centroids.load(&pool).await.unwrap().count(), 3);
}
