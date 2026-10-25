use std::collections::BTreeSet;

use leptos::{prelude::*, task::spawn_local};

use crate::{api::Api, components::*, hooks::*, logic::*, models::*};

#[island]
pub fn TimelineView(
    initial: Result<Vec<FeedItem>, String>,
    feeds: FeedNames,
    now: i64,
) -> impl IntoView {
    // Stays false until a list request succeeds, so a failed load is not shown as "no news".
    let loaded = RwSignal::new(initial.is_ok());
    let initial = initial.unwrap_or_default();
    let has_more = RwSignal::new(initial.len() >= PAGE_SIZE);
    let loading_more = RwSignal::new(false);
    let state = RwSignal::new(TimelineState::new(initial));
    let connected = RwSignal::new(false);
    let pulse = RwSignal::new(false);
    let new_ids = RwSignal::new(BTreeSet::<i64>::new());
    let notice = RwSignal::new(None::<String>);
    let now = use_clock(now);
    let feeds = StoredValue::new(feeds);

    // Cancelled when the island is disposed, which drops the stream and closes the connection.
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local_scoped_with_cancellation(async move {
        use crate::browser::{Stream, StreamEventKind as Kind, save_cursor, stored_cursor};
        use futures_util::StreamExt;

        // The cursor and replay buffer are not rendered, so they change without notifying.
        let Some(cursor) = state.try_with_untracked(|s| resume_cursor(&s.items, stored_cursor()))
        else {
            return;
        };
        state.try_update_untracked(|s| s.cursor = cursor.clone());
        save_cursor(cursor.as_deref());
        let mut stream = match Stream::connect(cursor.as_deref()) {
            Ok(stream) => stream,
            Err(error) => {
                notice.try_set(Some(error));
                return;
            }
        };
        while let Some(event) = stream.receiver.next().await {
            match event.kind {
                Kind::Open => {
                    connected.try_set(true);
                }
                Kind::Error => {
                    connected.try_set(false);
                    state.try_update(TimelineState::begin_replay);
                }
                Kind::FeedItem => {
                    let Ok(item) = serde_json::from_str::<FeedItem>(&event.data) else {
                        notice.try_set(Some("An invalid live update was ignored.".into()));
                        continue;
                    };
                    let id = item.id;
                    let event_id = if event.id.is_empty() {
                        id.to_string()
                    } else {
                        event.id
                    };
                    let outcome = state.try_maybe_update(|s| {
                        let outcome = s.receive(item, event_id);
                        (outcome == ReceiveOutcome::Live, outcome)
                    });
                    if outcome == Some(ReceiveOutcome::Live) {
                        state.try_with_untracked(|s| save_cursor(s.cursor.as_deref()));
                        new_ids.try_update(|ids| {
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
                Kind::ReplayDone => {
                    let marker =
                        serde_json::from_str::<ReplayDone>(&event.data).unwrap_or_default();
                    let outcome = state.try_update(|s| s.finish_replay(marker));
                    state.try_with_untracked(|s| save_cursor(s.cursor.as_deref()));
                    // Also retries the first page when the server could not load it.
                    let Some(was_loaded) = loaded.try_get_untracked() else {
                        return;
                    };
                    if outcome == Some(ReplayOutcome::Refresh) || !was_loaded {
                        spawn_local(async move {
                            if let Ok(items) =
                                Api::default().items(&NewsFilters::default(), 0).await
                            {
                                if !was_loaded {
                                    has_more.try_set(items.len() >= PAGE_SIZE);
                                }
                                state.try_update(|s| merge_items(&mut s.items, items));
                                loaded.try_set(true);
                            }
                        });
                    }
                }
                Kind::Lagged => {
                    notice.try_set(Some(
                        "Some live updates were missed; reconnecting will replay them.".into(),
                    ));
                }
                Kind::KeepAlive => pulse_once(pulse),
                Kind::Message if event.data == "keep-alive" => pulse_once(pulse),
                Kind::Message => {}
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
                        .try_update(|s| append_page(&mut s.items, page, Order::Newest))
                        .unwrap_or(false);
                    has_more.try_set(more);
                }
                Err(_) => {
                    has_more.try_set(false);
                    notice.try_set(Some("Could not load older items.".into()));
                }
            }
            loading_more.try_set(false);
        });
    });

    view! {
        <div class="px-4 py-3 border-b border-sumi-ink3 flex flex-wrap items-center justify-between gap-2 text-xs text-fuji-gray">
            <span role="status">
                <span
                    class="inline-block w-2 h-2 mr-2 rounded-full"
                    class=("bg-spring-green", move || connected.get())
                    class=("bg-autumn-red", move || !connected.get())
                    style:opacity=move || if pulse.get() { "0.3" } else { "1" }
                ></span>
                {move || if connected.get() { "Live" } else { "Connecting…" }}
                {move || state.with(TimelineState::is_replaying).then_some(" · catching up")}
            </span>
            <span>{move || format!("{} items", state.with(|s| s.items.len()))}</span>
        </div>
        <Show when=move || !loaded.get()>
            <ErrorNotice message="Could not load timeline." />
        </Show>
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
        // Cards are static; live items gain details after a follow-up fetch, and including
        // that in the key re-renders them once instead of making every card reactive.
        <For
            each=move || state.with(|s| s.items.clone())
            key=|item| (item.id, item.summary.is_some(), item.author.is_some(), item.published_at.is_some())
            let:item
        >
            {
                let id = item.id;
                let name = feeds.with_value(|names| feed_name(names, item.feed_id));
                let is_new = Signal::derive(move || new_ids.with(|ids| ids.contains(&id)));
                view! { <ItemCard item feed_name=name now is_new /> }
            }
        </For>
        <Show when=move || loaded.get() && state.with(|s| s.items.is_empty())>
            <p class="px-4 py-12 text-center text-katana-gray">
                "No news yet. Add sources to start collecting news."
            </p>
        </Show>
        <LoadMore has_more=has_more loading=loading_more onload=load_more />
    }
}

#[cfg(target_arch = "wasm32")]
fn pulse_once(pulse: RwSignal<bool>) {
    pulse.try_set(true);
    spawn_local(async move {
        sleep_ms(450).await;
        pulse.try_set(false);
    });
}
