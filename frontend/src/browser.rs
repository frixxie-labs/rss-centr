//! Browser-only SSE adapter. Dropping it closes the connection and releases every
//! JS callback, including when its owning island is disposed.
use futures_channel::mpsc::{UnboundedReceiver, unbounded};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{
    Element, Event, EventSource, IntersectionObserver, IntersectionObserverEntry,
    IntersectionObserverInit, MessageEvent,
};

use crate::logic::CURSOR_KEY;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamEventKind {
    Open,
    Error,
    FeedItem,
    ReplayDone,
    Lagged,
    KeepAlive,
    Message,
}

impl StreamEventKind {
    const ALL: [Self; 7] = [
        Self::Open,
        Self::Error,
        Self::FeedItem,
        Self::ReplayDone,
        Self::Lagged,
        Self::KeepAlive,
        Self::Message,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Error => "error",
            Self::FeedItem => "feed_item",
            Self::ReplayDone => "replay_done",
            Self::Lagged => "lagged",
            Self::KeepAlive => "keep_alive",
            Self::Message => "message",
        }
    }
}

pub struct StreamEvent {
    pub kind: StreamEventKind,
    pub data: String,
    pub id: String,
}

type Listener = (StreamEventKind, Closure<dyn FnMut(Event)>);

pub struct Stream {
    source: EventSource,
    listeners: Vec<Listener>,
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
        for kind in StreamEventKind::ALL {
            let sender = sender.clone();
            let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
                let message = event.dyn_ref::<MessageEvent>();
                let _ = sender.unbounded_send(StreamEvent {
                    kind,
                    data: message
                        .and_then(|m| m.data().as_string())
                        .unwrap_or_default(),
                    id: message.map(|m| m.last_event_id()).unwrap_or_default(),
                });
            });
            stream
                .source
                .add_event_listener_with_callback(kind.as_str(), callback.as_ref().unchecked_ref())
                .map_err(|_| "Could not register live update listener.".to_owned())?;
            stream.listeners.push((kind, callback));
        }
        Ok(stream)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.source.close();
        for (kind, listener) in &self.listeners {
            let _ = self.source.remove_event_listener_with_callback(
                kind.as_str(),
                listener.as_ref().unchecked_ref(),
            );
        }
    }
}

/// Calls `on_change` whenever an element moves near or away from the viewport.
/// Dropping it disconnects the observer.
pub struct Visible {
    observer: IntersectionObserver,
    _callback: Closure<dyn FnMut(js_sys::Array)>,
}

impl Visible {
    pub fn observe(element: &Element, mut on_change: impl FnMut(bool) + 'static) -> Option<Self> {
        let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            on_change(entries.iter().any(|entry| {
                entry
                    .unchecked_into::<IntersectionObserverEntry>()
                    .is_intersecting()
            }));
        });
        let options = IntersectionObserverInit::new();
        options.set_root_margin("600px 0px");
        let observer =
            IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options)
                .ok()?;
        observer.observe(element);
        Some(Self {
            observer,
            _callback: callback,
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
    if let Some(cursor) = cursor
        && let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten())
    {
        let _ = storage.set_item(CURSOR_KEY, cursor);
    }
}

/// Updates the address bar without adding a history entry or reloading.
pub fn replace_url(url: &str) {
    if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(url));
    }
}
