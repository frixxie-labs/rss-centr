use dioxus::prelude::*;

use crate::{logic::*, models::*};

pub const INPUT: &str = "min-w-0 rounded-md border border-sumi-ink4 bg-sumi-ink0 px-3 py-2 text-sm text-old-white outline-none transition focus:border-carp-yellow";
pub const BUTTON: &str = "rounded-md border border-sumi-ink4 px-3 py-2 text-sm text-old-white transition hover:bg-sumi-ink3 disabled:opacity-50 disabled:cursor-not-allowed";
pub const PANEL: &str =
    "mx-4 my-5 rounded-lg border border-sumi-ink3 bg-sumi-ink2/60 p-4 space-y-3";

#[component]
pub fn ErrorNotice(message: String) -> Element {
    rsx! { p { role: "alert", class: "mx-4 my-3 rounded-md border border-autumn-red/70 bg-winter-red/40 px-3 py-2 text-sm text-wave-red", "{message}" } }
}

#[component]
pub fn ItemCard(
    item: FeedItem,
    feed_name: String,
    now: i64,
    #[props(default)] is_new: bool,
) -> Element {
    let text = truncate(&preview(&item), PREVIEW_CHARS);
    let host = hostname(&item.url);
    rsx! {
        a {
            href: "{item.url}", target: "_blank", rel: "noopener noreferrer",
            class: if is_new { "block min-w-0 px-4 py-3 border-b border-sumi-ink3 hover:bg-sumi-ink2 transition-colors bg-wave-blue1/50 border-l-2 border-l-carp-yellow" } else { "block min-w-0 px-4 py-3 border-b border-sumi-ink3 hover:bg-sumi-ink2 transition-colors" },
            h3 { class: "break-words text-sm font-medium text-fuji-white leading-snug", "{item.title}" }
            if !text.is_empty() { p { class: "mt-2 line-clamp-3 break-words text-sm leading-relaxed text-fuji-gray", "{text}" } }
            div { class: "mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-katana-gray",
                span { class: "break-all", "{host}" }
                if let Some(author) = &item.author {
                    if !author.is_empty() { span { "·" } span { class: "break-words text-fuji-gray", "{author}" } }
                }
                if let Some(date) = &item.published_at {
                    span { "·" }
                    time { datetime: "{date}", title: absolute_date(Some(date)), class: "text-fuji-gray", "published {time_ago(date, now)}" }
                }
                if !feed_name.is_empty() { span { "·" } span { class: "break-words text-fuji-gray", "{feed_name}" } }
                span { "·" }
                time { datetime: "{item.inserted_at}", "{time_ago(&item.inserted_at, now)}" }
            }
        }
    }
}

/// Loads the next page when scrolled into view; the button covers no-JS-observer cases.
#[component]
pub fn LoadMore(has_more: bool, loading: bool, onload: EventHandler<()>) -> Element {
    rsx! {
        div {
            class: "px-4 py-4 text-center",
            onmounted: move |_event| {
                #[cfg(all(feature = "web", target_arch = "wasm32"))]
                if let Some(element) = _event.data().downcast::<web_sys::Element>().cloned() {
                    spawn(async move {
                        use futures_util::StreamExt;
                        let Some(mut visible) = crate::browser::Visible::observe(&element) else {
                            return;
                        };
                        while visible.receiver.next().await.is_some() {
                            onload.call(());
                        }
                    });
                }
            },
            if has_more {
                button { class: BUTTON, disabled: loading, onclick: move |_| onload.call(()),
                    if loading { "Loading…" } else { "Load more" }
                }
            }
        }
    }
}

pub fn feed_name(feeds: &[Feed], id: i64) -> String {
    feeds
        .iter()
        .find(|f| f.id == id)
        .map(|f| f.name().to_owned())
        .unwrap_or_default()
}

pub fn use_clock() -> Signal<i64> {
    let initial = use_server_cached(|| chrono::Utc::now().timestamp_millis());
    #[allow(unused_mut)]
    let mut now = use_signal(|| initial);
    #[cfg(all(feature = "web", target_arch = "wasm32"))]
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(60_000).await;
            now.set(chrono::Utc::now().timestamp_millis());
        }
    });
    now
}
