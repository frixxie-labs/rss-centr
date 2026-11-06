use std::collections::HashMap;

use super::SummaryArticle;

// Unrelated texts score about 0.55-0.65 with embeddinggemma; reports of the
// same event from different outlets score well above that.
const DUPLICATE_SIMILARITY: f32 = 0.88;
// Keep one busy feed from crowding out the others, and bound the model calls.
const MAX_STORIES_PER_FEED: usize = 40;
const MAX_STORIES: usize = 400;

/// One event to summarize: an article and the later near-duplicates of it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Story {
    /// The first article's id; it identifies the story.
    pub(crate) id: i64,
    pub(crate) item_ids: Vec<i64>,
    pub(crate) feed_ids: Vec<i64>,
    pub(crate) sources: Vec<String>,
    pub(crate) title: String,
    pub(crate) description: String,
}

/// Groups near-duplicate articles into stories and keeps the newest stories
/// within the per-feed and total limits. Articles are oldest first, and so
/// are the stories.
pub(crate) fn select_stories(articles: Vec<SummaryArticle>) -> Vec<Story> {
    cap_stories(
        group_duplicates(articles),
        MAX_STORIES_PER_FEED,
        MAX_STORIES,
    )
}

fn group_duplicates(articles: Vec<SummaryArticle>) -> Vec<Story> {
    let mut stories: Vec<Story> = Vec::new();
    let mut embeddings: Vec<Option<pgvector::Vector>> = Vec::new();
    for article in articles {
        let duplicate_of = article.embedding.as_ref().and_then(|embedding| {
            embeddings.iter().position(|other| {
                other.as_ref().is_some_and(|other| {
                    similarity(embedding.as_slice(), other.as_slice()) >= DUPLICATE_SIMILARITY
                })
            })
        });
        match duplicate_of {
            Some(index) => {
                let story = &mut stories[index];
                story.item_ids.push(article.id);
                if !story.feed_ids.contains(&article.feed_id) {
                    story.feed_ids.push(article.feed_id);
                }
                if !story.sources.contains(&article.feed) {
                    story.sources.push(article.feed);
                }
            }
            None => {
                embeddings.push(article.embedding);
                stories.push(Story {
                    id: article.id,
                    item_ids: vec![article.id],
                    feed_ids: vec![article.feed_id],
                    sources: vec![article.feed],
                    title: article.title,
                    description: article.description,
                });
            }
        }
    }
    stories
}

/// Embeddings are normalized, so their inner product is the cosine.
fn similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

/// The newest stories, at most `per_feed` from each first feed and `total`
/// overall, in their original order.
fn cap_stories(stories: Vec<Story>, per_feed: usize, total: usize) -> Vec<Story> {
    let mut per_feed_count: HashMap<i64, usize> = HashMap::new();
    let mut kept: Vec<Story> = stories
        .into_iter()
        .rev()
        .filter(|story| {
            let count = per_feed_count.entry(story.feed_ids[0]).or_default();
            *count += 1;
            *count <= per_feed
        })
        .take(total)
        .collect();
    kept.reverse();
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article(id: i64, feed_id: i64, embedding: Option<Vec<f32>>) -> SummaryArticle {
        SummaryArticle {
            id,
            feed_id,
            title: format!("title {id}"),
            feed: format!("feed {feed_id}"),
            description: format!("description {id}"),
            embedding: embedding.map(pgvector::Vector::from),
        }
    }

    #[test]
    fn near_duplicates_join_the_first_story_with_their_sources() {
        let stories = select_stories(vec![
            article(1, 1, Some(vec![1.0, 0.0])),
            article(2, 2, Some(vec![0.0, 1.0])),
            article(3, 2, Some(vec![0.95, 0.312])),
            article(4, 1, Some(vec![0.999, 0.045])),
            article(5, 3, None),
        ]);

        let groups: Vec<_> = stories
            .iter()
            .map(|story| (story.id, story.item_ids.clone(), story.sources.clone()))
            .collect();
        assert_eq!(
            groups,
            [
                (1, vec![1, 3, 4], vec!["feed 1".into(), "feed 2".into()]),
                (2, vec![2], vec!["feed 2".into()]),
                (5, vec![5], vec!["feed 3".into()]),
            ]
        );
        assert_eq!(stories[0].feed_ids, [1, 2]);
        assert_eq!(stories[0].title, "title 1");
    }

    #[test]
    fn articles_without_comparable_embeddings_stay_separate() {
        let stories = select_stories(vec![
            article(1, 1, None),
            article(2, 1, None),
            article(3, 1, Some(vec![1.0, 0.0])),
            article(4, 1, Some(vec![1.0, 0.0, 0.0])),
        ]);
        assert_eq!(stories.len(), 4);
    }

    #[test]
    fn caps_keep_the_newest_stories_per_feed_and_overall_in_order() {
        let stories = group_duplicates(
            [(1, 1), (2, 1), (3, 2), (4, 1), (5, 2), (6, 3)]
                .into_iter()
                .map(|(id, feed)| article(id, feed, None))
                .collect(),
        );
        let ids = |stories: Vec<Story>| stories.iter().map(|s| s.id).collect::<Vec<_>>();

        assert_eq!(ids(cap_stories(stories.clone(), 2, 10)), [2, 3, 4, 5, 6]);
        assert_eq!(ids(cap_stories(stories, 2, 3)), [4, 5, 6]);
    }

    quickcheck::quickcheck! {
        fn prop_every_kept_article_is_in_exactly_one_story(feeds: Vec<u8>) -> bool {
            let articles: Vec<_> = feeds
                .iter()
                .enumerate()
                .map(|(id, feed)| {
                    let angle = f32::from(*feed % 4);
                    article(id as i64, i64::from(*feed % 3), Some(vec![angle.cos(), angle.sin()]))
                })
                .collect();
            let stories = group_duplicates(articles);
            let mut ids: Vec<i64> = stories.iter().flat_map(|s| s.item_ids.clone()).collect();
            ids.sort_unstable();
            ids == (0..feeds.len() as i64).collect::<Vec<_>>()
                && stories.windows(2).all(|pair| pair[0].id < pair[1].id)
        }
    }
}
