use std::collections::BTreeSet;

use leptos::{prelude::*, task::spawn_local};

use crate::{api::Api, components::*, logic::*, models::*};

#[island]
pub fn TimelineView(initial: Vec<FeedItem>, feeds: Vec<Feed>, now: i64) -> impl IntoView {
    let has_more = RwSignal::new(initial.len() >= PAGE_SIZE);
    let loading_more = RwSignal::new(false);
    let state = RwSignal::new(TimelineState::new(initial));
    let connected = RwSignal::new(false);
    let pulse = RwSignal::new(false);
    let new_ids = RwSignal::new(BTreeSet::<i64>::new());
    let notice = RwSignal::new(None::<String>);
    let now = use_clock(now);
    let feeds = StoredValue::new(feeds);

    #[cfg(target_arch = "wasm32")]
    spawn_local(async move {
        use crate::browser::{Stream, save_cursor, stored_cursor};
        use futures_util::StreamExt;

        let cursor = state.with_untracked(|s| resume_cursor(&s.items, stored_cursor()));
        state.update(|s| s.cursor = cursor.clone());
        save_cursor(cursor.as_deref());
        let mut stream = match Stream::connect(cursor.as_deref()) {
            Ok(stream) => stream,
            Err(error) => {
                notice.set(Some(error));
                return;
            }
        };
        while let Some(event) = stream.receiver.next().await {
            // Stop (and drop the stream, closing the connection) once the island is disposed.
            if state.is_disposed() {
                break;
            }
            match event.kind.as_str() {
                "open" => connected.set(true),
                "error" => {
                    connected.set(false);
                    state.update(|s| s.replaying = true);
                }
                "feed_item" => {
                    let Ok(item) = serde_json::from_str::<FeedItem>(&event.data) else {
                        notice.set(Some("An invalid live update was ignored.".into()));
                        continue;
                    };
                    let id = item.id;
                    let replaying = state.with_untracked(|s| s.replaying);
                    let event_id = if event.id.is_empty() {
                        id.to_string()
                    } else {
                        event.id
                    };
                    state.update(|s| s.receive(item, event_id));
                    if !replaying {
                        state.with_untracked(|s| save_cursor(s.cursor.as_deref()));
                        new_ids.update(|ids| {
                            ids.insert(id);
                        });
                        spawn_local(async move {
                            sleep_ms(30_000).await;
                            new_ids.try_update(|ids| ids.remove(&id));
                        });
                        spawn_local(async move {
                            if let Ok(item) = Api::default().item(id).await {
                                state.try_update(|s| merge_items(&mut s.items, [item]));
                            }
                        });
                    }
                }
                "replay_done" => {
                    let marker =
                        serde_json::from_str::<ReplayDone>(&event.data).unwrap_or_default();
                    let refresh = state
                        .try_update(|s| s.finish_replay(marker))
                        .unwrap_or(false);
                    state.with_untracked(|s| save_cursor(s.cursor.as_deref()));
                    if refresh {
                        spawn_local(async move {
                            if let Ok(items) =
                                Api::default().items(&NewsFilters::default(), 0).await
                            {
                                state.try_update(|s| merge_items(&mut s.items, items));
                            }
                        });
                    }
                }
                "lagged" => notice.set(Some(
                    "Some live updates were missed; reconnecting will replay them.".into(),
                )),
                "keep_alive" => pulse_once(pulse),
                "message" if event.data == "keep-alive" => pulse_once(pulse),
                _ => {}
            }
        }
    });

    let load_more = Callback::new(move |()| {
        if loading_more.get_untracked() || !has_more.get_untracked() {
            return;
        }
        // Live items shift the backend's offsets too, so the list length stays aligned;
        // any overlap is de-duplicated by id.
        let offset = state.with_untracked(|s| s.items.len());
        loading_more.set(true);
        spawn_local(async move {
            match Api::default().items(&NewsFilters::default(), offset).await {
                Ok(page) => {
                    let more = state
                        .try_update(|s| append_page(&mut s.items, page, "keyword", ""))
                        .unwrap_or(false);
                    has_more.set(more);
                }
                Err(_) => {
                    has_more.set(false);
                    notice.set(Some("Could not load older items.".into()));
                }
            }
            loading_more.set(false);
        });
    });

    view! {
        <div class="px-4 py-3 border-b border-sumi-ink3 flex flex-wrap items-center justify-between gap-2 text-xs text-fuji-gray">
            <span role="status">
                <span
                    class=move || {
                        if connected.get() {
                            "inline-block w-2 h-2 mr-2 rounded-full bg-spring-green"
                        } else {
                            "inline-block w-2 h-2 mr-2 rounded-full bg-autumn-red"
                        }
                    }
                    style=move || if pulse.get() { "opacity: 0.3" } else { "opacity: 1" }
                ></span>
                {move || if connected.get() { "Live" } else { "Connecting…" }}
                {move || state.with(|s| s.replaying).then_some(" · catching up")}
            </span>
            <span>{move || format!("{} items", state.with(|s| s.items.len()))}</span>
        </div>
        <Show when=move || state.with(|s| s.limited)>
            <p class="px-4 py-2 text-xs text-ronin-yellow">
                "showing latest updates; older missed items skipped"
            </p>
        </Show>
        {move || {
            notice
                .get()
                .map(|message| {
                    view! {
                        <p role="status" class="px-4 py-2 text-xs text-ronin-yellow">
                            {message}
                        </p>
                    }
                })
        }}
        // Live items gain details after a follow-up fetch; including that in the key re-renders them once.
        <For
            each=move || state.with(|s| s.items.clone())
            key=|item| (item.id, item.summary.is_some(), item.author.is_some(), item.published_at.is_some())
            let:item
        >
            {
                let id = item.id;
                let name = feeds.with_value(|f| feed_name(f, item.feed_id));
                let is_new = Signal::derive(move || new_ids.with(|ids| ids.contains(&id)));
                view! { <ItemCard item feed_name=name now is_new /> }
            }
        </For>
        <Show when=move || state.with(|s| s.items.is_empty())>
            <p class="px-4 py-12 text-center text-katana-gray">
                "No news yet. Add sources to start collecting news."
            </p>
        </Show>
        <LoadMore has_more=has_more loading=loading_more onload=load_more />
    }
}

#[cfg(target_arch = "wasm32")]
fn pulse_once(pulse: RwSignal<bool>) {
    pulse.set(true);
    spawn_local(async move {
        sleep_ms(450).await;
        pulse.try_set(false);
    });
}
