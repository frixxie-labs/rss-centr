use leptos::prelude::*;

use crate::{logic::*, models::*};

pub const INPUT: &str = "min-w-0 rounded-md border border-sumi-ink4 bg-sumi-ink0 px-3 py-2 text-sm text-old-white outline-none transition focus:border-carp-yellow";
pub const BUTTON: &str = "rounded-md border border-sumi-ink4 px-3 py-2 text-sm text-old-white transition hover:bg-sumi-ink3 disabled:opacity-50 disabled:cursor-not-allowed";
pub const PANEL: &str =
    "mx-4 my-5 rounded-lg border border-sumi-ink3 bg-sumi-ink2/60 p-4 space-y-3";
pub const LOADING: &str = "Loading…";

#[component]
pub fn ErrorNotice(#[prop(into)] message: String) -> impl IntoView {
    view! {
        <p
            role="alert"
            class="mx-4 my-3 rounded-md border border-autumn-red/70 bg-winter-red/40 px-3 py-2 text-sm text-wave-red"
        >
            {message}
        </p>
    }
}

#[component]
pub fn ItemCard(
    item: FeedItem,
    feed_name: String,
    #[prop(into)] now: Signal<i64>,
    #[prop(into, optional)] is_new: Signal<bool>,
) -> impl IntoView {
    let text = truncate(&preview(&item), PREVIEW_CHARS);
    let host = hostname(&item.url);
    let author = item.author.clone().filter(|a| !a.is_empty());
    let published = item.published_at.clone().map(|date| {
        let title = absolute_date(Some(&date));
        let label = date.clone();
        view! {
            <span>"·"</span>
            <time datetime=date title=title class="text-fuji-gray">
                {move || format!("published {}", time_ago(&label, now.get()))}
            </time>
        }
    });
    let inserted = item.inserted_at.clone();
    let inserted_label = inserted.clone();
    view! {
        <a
            href=item.url.clone()
            target="_blank"
            rel="noopener noreferrer"
            class=move || {
                if is_new.get() {
                    "block min-w-0 px-4 py-3 border-b border-sumi-ink3 hover:bg-sumi-ink2 transition-colors bg-wave-blue1/50 border-l-2 border-l-carp-yellow"
                } else {
                    "block min-w-0 px-4 py-3 border-b border-sumi-ink3 hover:bg-sumi-ink2 transition-colors"
                }
            }
        >
            <h3 class="break-words text-sm font-medium text-fuji-white leading-snug">
                {item.title.clone()}
            </h3>
            {(!text.is_empty())
                .then(|| {
                    view! {
                        <p class="mt-2 line-clamp-3 break-words text-sm leading-relaxed text-fuji-gray">
                            {text}
                        </p>
                    }
                })}
            <div class="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-katana-gray">
                <span class="break-all">{host}</span>
                {author
                    .map(|author| {
                        view! {
                            <span>"·"</span>
                            <span class="break-words text-fuji-gray">{author}</span>
                        }
                    })}
                {published}
                {(!feed_name.is_empty())
                    .then(|| {
                        view! {
                            <span>"·"</span>
                            <span class="break-words text-fuji-gray">{feed_name}</span>
                        }
                    })}
                <span>"·"</span>
                <time datetime=inserted>{move || time_ago(&inserted_label, now.get())}</time>
            </div>
        </a>
    }
}

/// Loads pages while the sentinel stays near the viewport; the button covers no-observer cases.
#[component]
pub fn LoadMore(
    #[prop(into)] has_more: Signal<bool>,
    #[prop(into)] loading: Signal<bool>,
    onload: Callback<()>,
) -> impl IntoView {
    let node = NodeRef::<leptos::html::Div>::new();
    #[cfg(target_arch = "wasm32")]
    {
        let in_view = RwSignal::new(false);
        Effect::new(move |started: Option<bool>| {
            if started == Some(true) {
                return true;
            }
            let Some(element) = node.get() else {
                return false;
            };
            leptos::task::spawn_local(async move {
                use futures_util::StreamExt;
                let Some(mut visible) = crate::browser::Visible::observe(&element) else {
                    return;
                };
                while let Some(intersecting) = visible.receiver.next().await {
                    if in_view.try_set(intersecting).is_some() {
                        break;
                    }
                }
            });
            true
        });
        // Re-runs after each page, so a sentinel that never leaves the viewport keeps loading.
        Effect::new(move |_| {
            if in_view.get() && has_more.get() && !loading.get() {
                onload.run(());
            }
        });
    }
    view! {
        <div class="px-4 py-4 text-center" node_ref=node>
            <Show when=move || has_more.get()>
                <button class=BUTTON disabled=move || loading.get() on:click=move |_| onload.run(())>
                    {move || if loading.get() { LOADING } else { "Load more" }}
                </button>
            </Show>
        </div>
    }
}

pub fn feed_name(feeds: &[Feed], id: i64) -> String {
    feeds
        .iter()
        .find(|f| f.id == id)
        .map(|f| f.name().to_owned())
        .unwrap_or_default()
}

/// Starts at the server's render time so hydration matches, then ticks every minute.
pub fn use_clock(initial: i64) -> Signal<i64> {
    let now = RwSignal::new(initial);
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(60_000).await;
            if now.try_set(chrono::Utc::now().timestamp_millis()).is_some() {
                break;
            }
        }
    });
    now.into()
}

pub fn sleep_ms(_ms: u32) -> impl std::future::Future<Output = ()> {
    #[cfg(target_arch = "wasm32")]
    return gloo_timers::future::TimeoutFuture::new(_ms);
    #[cfg(not(target_arch = "wasm32"))]
    std::future::ready(())
}
