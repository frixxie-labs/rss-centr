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
use crate::feed::feed_item::read_feed_items_after_id;

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

    let replay_items = match last_event_id {
        Some(id) => match read_feed_items_after_id(&pool, id).await {
            Ok(items) => items,
            Err(e) => {
                warn!("failed to read replay items after Last-Event-ID={id}: {e:#}");
                Vec::new()
            }
        },
        None => Vec::new(),
    };

    let replay_stream = tokio_stream::iter(replay_items.into_iter().filter_map(|item| {
        let payload = NewFeedItemEvent::from(&item);
        Event::default()
            .id(item.id.to_string())
            .event("feed_item")
            .json_data(payload)
            .ok()
            .map(Ok)
    }));

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

    Sse::new(replay_stream.chain(item_stream).merge(keep_alive_stream))
}
