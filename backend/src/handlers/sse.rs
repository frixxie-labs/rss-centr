use std::{convert::Infallible, time::Duration};

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::sse::{Event, Sse},
};
use serde::Deserialize;
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio::time::interval;
use tokio_stream::{StreamExt, wrappers::BroadcastStream, wrappers::IntervalStream};
use tracing::warn;

use crate::events::NewFeedItemEvent;
use crate::feed::feed_item::{FeedItemsAfterId, read_feed_items_after_id};

const REPLAY_LIMIT: i64 = 500;

#[derive(Debug, Deserialize)]
pub struct SseQuery {
    pub last_event_id: Option<i64>,
}

pub async fn stream_new_items(
    State((pool, tx)): State<(PgPool, broadcast::Sender<NewFeedItemEvent>)>,
    headers: HeaderMap,
    Query(query): Query<SseQuery>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = tx.subscribe();

    let header_last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok());

    let last_event_id = header_last_event_id.or(query.last_event_id);

    if headers.contains_key("last-event-id") && header_last_event_id.is_none() {
        warn!("ignoring invalid Last-Event-ID header");
    }

    let replay = match last_event_id {
        Some(id) => match read_feed_items_after_id(&pool, id, REPLAY_LIMIT).await {
            Ok(replay) => replay,
            Err(e) => {
                warn!("failed to read replay items after Last-Event-ID={id}: {e:#}");
                FeedItemsAfterId {
                    items: Vec::new(),
                    skipped_older: false,
                }
            }
        },
        None => FeedItemsAfterId {
            items: Vec::new(),
            skipped_older: false,
        },
    };

    let replayed = replay.items.len();
    let replay_limited = replay.skipped_older;
    let replay_last_event_id = replay.items.last().map(|item| item.id).or(last_event_id);

    let replay_stream = tokio_stream::iter(replay.items.into_iter().filter_map(|item| {
        let payload = NewFeedItemEvent::from(&item);
        Event::default()
            .id(item.id.to_string())
            .event("feed_item")
            .json_data(payload)
            .ok()
            .map(Ok)
    }));

    let replay_done_data = serde_json::json!({
        "replayed": replayed,
        "limited": replay_limited,
        "last_event_id": replay_last_event_id,
    })
    .to_string();
    let replay_done_stream = tokio_stream::once(Ok(Event::default()
        .event("replay_done")
        .data(replay_done_data)));

    let item_stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(item) => match Event::default()
            .id(item.id.to_string())
            .event("feed_item")
            .json_data(item)
        {
            Ok(event) => Some(Ok(event)),
            Err(_) => None,
        },
        Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(skipped)) => {
            let event = Event::default().event("lagged").data(skipped.to_string());
            Some(Ok(event))
        }
    });

    let keep_alive_stream = IntervalStream::new(interval(Duration::from_secs(15)))
        .map(|_| Ok(Event::default().event("keep_alive").data("keep-alive")));

    Sse::new(
        replay_stream
            .chain(replay_done_stream)
            .chain(item_stream)
            .merge(keep_alive_stream),
    )
}
