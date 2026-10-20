use dioxus::prelude::*;

use crate::{api::Api, components::*, logic::*, models::*};

#[component]
pub fn Timeline() -> Element {
    let response = use_server_future(|| async {
        let api = Api::default();
        let filters = NewsFilters::default();
        futures_util::join!(api.items(&filters, 0), api.feeds())
    })?;
    let data = response.read();
    let Some((items, feeds)) = data.as_ref() else {
        return rsx! { "Loading…" };
    };
    rsx! {
        document::Title { "RSS Centr - Live Timeline" }
        if items.is_err() { ErrorNotice { message: "Could not load timeline." } }
        TimelineView { initial: items.clone().unwrap_or_default(), feeds: feeds.clone().unwrap_or_default() }
    }
}

#[component]
fn TimelineView(initial: Vec<FeedItem>, feeds: Vec<Feed>) -> Element {
    let mut has_more = use_signal(|| initial.len() >= PAGE_SIZE);
    let mut loading_more = use_signal(|| false);
    #[allow(unused_mut)]
    let mut state = use_signal(|| TimelineState::new(initial));
    #[allow(unused_mut)]
    let mut connected = use_signal(|| false);
    #[allow(unused_mut)]
    let mut pulse = use_signal(|| false);
    #[allow(unused_mut)]
    let mut new_ids = use_signal(std::collections::BTreeSet::<i64>::new);
    #[allow(unused_mut)]
    let mut notice = use_signal(|| None::<String>);
    let now = use_clock();

    #[cfg(all(feature = "web", target_arch = "wasm32"))]
    use_future(move || async move {
        use crate::browser::{Stream, save_cursor, stored_cursor};
        use futures_util::StreamExt;

        let cursor = resume_cursor(&state.peek().items, stored_cursor());
        state.write().cursor = cursor.clone();
        save_cursor(cursor.as_deref());
        let mut stream = match Stream::connect(cursor.as_deref()) {
            Ok(stream) => stream,
            Err(error) => {
                notice.set(Some(error));
                return;
            }
        };
        while let Some(event) = stream.receiver.next().await {
            match event.kind.as_str() {
                "open" => connected.set(true),
                "error" => {
                    connected.set(false);
                    state.write().replaying = true;
                }
                "feed_item" => {
                    if let Ok(item) = serde_json::from_str::<FeedItem>(&event.data) {
                        let id = item.id;
                        let replaying = state.peek().replaying;
                        let event_id = if event.id.is_empty() {
                            id.to_string()
                        } else {
                            event.id
                        };
                        state.write().receive(item, event_id);
                        if !replaying {
                            save_cursor(state.peek().cursor.as_deref());
                            new_ids.write().insert(id);
                            spawn(async move {
                                gloo_timers::future::TimeoutFuture::new(30_000).await;
                                new_ids.write().remove(&id);
                            });
                            spawn(async move {
                                if let Ok(item) = Api::default().item(id).await {
                                    merge_items(&mut state.write().items, [item]);
                                }
                            });
                        }
                    } else {
                        notice.set(Some("An invalid live update was ignored.".into()));
                    }
                }
                "replay_done" => {
                    let marker =
                        serde_json::from_str::<ReplayDone>(&event.data).unwrap_or_default();
                    let refresh = state.write().finish_replay(marker);
                    save_cursor(state.peek().cursor.as_deref());
                    if refresh {
                        spawn(async move {
                            if let Ok(items) =
                                Api::default().items(&NewsFilters::default(), 0).await
                            {
                                merge_items(&mut state.write().items, items);
                            }
                        });
                    }
                }
                "lagged" => notice.set(Some(
                    "Some live updates were missed; reconnecting will replay them.".into(),
                )),
                "keep_alive" | "message"
                    if event.kind == "keep_alive" || event.data == "keep-alive" =>
                {
                    pulse.set(true);
                    spawn(async move {
                        gloo_timers::future::TimeoutFuture::new(450).await;
                        pulse.set(false);
                    });
                }
                _ => {}
            }
        }
    });

    let load_more = move |_| {
        if *loading_more.peek() || !*has_more.peek() {
            return;
        }
        // Live items shift the backend's offsets too, so the list length stays aligned;
        // any overlap is de-duplicated by id.
        let offset = state.peek().items.len();
        loading_more.set(true);
        spawn(async move {
            match Api::default().items(&NewsFilters::default(), offset).await {
                Ok(page) => {
                    has_more.set(append_page(&mut state.write().items, page, "keyword", ""))
                }
                Err(_) => notice.set(Some("Could not load older items.".into())),
            }
            loading_more.set(false);
        });
    };
    let state = state.read();
    rsx! {
        div { class: "px-4 py-3 border-b border-sumi-ink3 flex flex-wrap items-center justify-between gap-2 text-xs text-fuji-gray",
            span { role: "status",
                span { class: if connected() { "inline-block w-2 h-2 mr-2 rounded-full bg-spring-green" } else { "inline-block w-2 h-2 mr-2 rounded-full bg-autumn-red" }, style: if pulse() { "opacity: 0.3" } else { "opacity: 1" } }
                if connected() { "Live" } else { "Connecting…" }
                if state.replaying { " · catching up" }
            }
            span { "{state.items.len()} items" }
        }
        if state.limited { p { class: "px-4 py-2 text-xs text-ronin-yellow", "showing latest updates; older missed items skipped" } }
        if let Some(message) = notice() { p { role: "status", class: "px-4 py-2 text-xs text-ronin-yellow", "{message}" } }
        for item in &state.items {
            ItemCard { key: "{item.id}", item: item.clone(), feed_name: feed_name(&feeds, item.feed_id), now: now(), is_new: new_ids.read().contains(&item.id) }
        }
        if state.items.is_empty() { p { class: "px-4 py-12 text-center text-katana-gray", "No news yet. Add sources to start collecting news." } }
        LoadMore { has_more: has_more(), loading: loading_more(), onload: load_more }
    }
}
