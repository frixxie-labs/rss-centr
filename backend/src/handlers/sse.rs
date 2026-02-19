use std::{convert::Infallible, time::Duration};

use axum::{
    extract::State,
    response::sse::{Event, Sse},
};
use tokio::sync::broadcast;
use tokio::time::interval;
use tokio_stream::{StreamExt, wrappers::BroadcastStream, wrappers::IntervalStream};

use crate::events::NewFeedItemEvent;

pub async fn stream_new_items(
    State(tx): State<broadcast::Sender<NewFeedItemEvent>>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = tx.subscribe();

    let item_stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(item) => match Event::default().event("feed_item").json_data(item) {
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

    Sse::new(item_stream.merge(keep_alive_stream))
}
