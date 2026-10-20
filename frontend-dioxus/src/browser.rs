//! Browser-only SSE adapter. Dropping it closes the connection and releases every
//! JS callback, including when the Dioxus task is cancelled during navigation.
use futures_channel::mpsc::{UnboundedReceiver, unbounded};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{
    Element, Event, EventSource, IntersectionObserver, IntersectionObserverEntry,
    IntersectionObserverInit, MessageEvent,
};

use crate::logic::CURSOR_KEY;

pub struct StreamEvent {
    pub kind: String,
    pub data: String,
    pub id: String,
}

pub struct Stream {
    source: EventSource,
    listeners: Vec<(String, Closure<dyn FnMut(Event)>)>,
    pub receiver: UnboundedReceiver<StreamEvent>,
}

impl Stream {
    pub fn connect(cursor: Option<&str>) -> Result<Self, String> {
        let mut url = "/api/items/stream".to_owned();
        if let Some(cursor) = cursor {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("last_event_id", cursor)
                .finish();
            url.push('?');
            url.push_str(&query);
        }
        let source =
            EventSource::new(&url).map_err(|_| "Could not open live updates.".to_owned())?;
        let (sender, receiver) = unbounded();
        let mut stream = Self {
            source,
            listeners: Vec::new(),
            receiver,
        };
        for kind in [
            "open",
            "error",
            "feed_item",
            "replay_done",
            "lagged",
            "keep_alive",
            "message",
        ] {
            let sender = sender.clone();
            let callback = Closure::wrap(Box::new(move |event: Event| {
                let message = event.dyn_ref::<MessageEvent>();
                let _ = sender.unbounded_send(StreamEvent {
                    kind: kind.into(),
                    data: message
                        .and_then(|m| m.data().as_string())
                        .unwrap_or_default(),
                    id: message.map(|m| m.last_event_id()).unwrap_or_default(),
                });
            }) as Box<dyn FnMut(Event)>);
            stream
                .source
                .add_event_listener_with_callback(kind, callback.as_ref().unchecked_ref())
                .map_err(|_| "Could not register live update listener.".to_owned())?;
            stream.listeners.push((kind.into(), callback));
        }
        Ok(stream)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.source.close();
        for (kind, listener) in &self.listeners {
            let _ = self
                .source
                .remove_event_listener_with_callback(kind, listener.as_ref().unchecked_ref());
        }
    }
}

/// Signals whenever an element scrolls near the viewport. Dropping it disconnects the observer.
pub struct Visible {
    observer: IntersectionObserver,
    _callback: Closure<dyn FnMut(js_sys::Array)>,
    pub receiver: UnboundedReceiver<()>,
}

impl Visible {
    pub fn observe(element: &Element) -> Option<Self> {
        let (sender, receiver) = unbounded();
        let callback = Closure::wrap(Box::new(move |entries: js_sys::Array| {
            if entries.iter().any(|entry| {
                entry
                    .unchecked_into::<IntersectionObserverEntry>()
                    .is_intersecting()
            }) {
                let _ = sender.unbounded_send(());
            }
        }) as Box<dyn FnMut(js_sys::Array)>);
        let options = IntersectionObserverInit::new();
        options.set_root_margin("600px 0px");
        let observer =
            IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options)
                .ok()?;
        observer.observe(element);
        Some(Self {
            observer,
            _callback: callback,
            receiver,
        })
    }
}

impl Drop for Visible {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

pub fn stored_cursor() -> Option<String> {
    web_sys::window()?
        .local_storage()
        .ok()??
        .get_item(CURSOR_KEY)
        .ok()
        .flatten()
}

pub fn save_cursor(cursor: Option<&str>) {
    if let Some(cursor) = cursor {
        if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = storage.set_item(CURSOR_KEY, cursor);
        }
    }
}
