use leptos::{prelude::*, task::spawn_local};

use crate::{api::Api, components::*, logic::*, models::*};

#[island]
pub fn NewsView(
    initial: NewsFilters,
    initial_items: Result<Vec<FeedItem>, String>,
    feeds: Vec<Feed>,
    now: i64,
) -> impl IntoView {
    let query = RwSignal::new(initial.query.clone());
    let source = RwSignal::new(initial.feed_id.map(|id| id.to_string()).unwrap_or_default());
    let mode = RwSignal::new(initial.mode.clone());
    let items = RwSignal::new(display_order(
        initial_items.clone().unwrap_or_default(),
        &initial.mode,
        &initial.query,
    ));
    let error = RwSignal::new(initial_items.is_err());
    let loading = RwSignal::new(false);
    let has_more = RwSignal::new(initial_items.as_ref().is_ok_and(|i| i.len() >= PAGE_SIZE));
    let loading_more = RwSignal::new(false);
    // Bumped on every new search; responses for an older generation are discarded,
    // which also covers in-flight "load more" pages and debounced searches.
    let generation = RwSignal::new(0_u64);
    let now = use_clock(now);
    let skip_initial = initial_items.is_ok();
    let current_filters = move || NewsFilters {
        query: query.get().trim().into(),
        feed_id: source.get().parse().ok(),
        mode: mode.get(),
    };

    Effect::new(move |previous: Option<NewsFilters>| {
        let filters = current_filters();
        let Some(previous) = previous.or_else(|| (!skip_initial).then(NewsFilters::default)) else {
            return filters;
        };
        if previous == filters && skip_initial {
            return filters;
        }
        // Only typing is debounced; dropdown changes search immediately.
        let debounce = previous.query != filters.query;
        generation.update(|g| *g += 1);
        let current = generation.get_untracked();
        let request = filters.clone();
        spawn_local(async move {
            let filters = request;
            if debounce {
                sleep_ms(250).await;
            }
            if generation.get_untracked() != current {
                return;
            }
            #[cfg(target_arch = "wasm32")]
            crate::browser::replace_url(&filters.page_url());
            loading.set(true);
            error.set(false);
            let result = Api::default().items(&filters, 0).await;
            if generation.get_untracked() != current {
                return;
            }
            match result {
                Ok(result) => {
                    has_more.set(result.len() >= PAGE_SIZE);
                    items.set(display_order(result, &filters.mode, &filters.query));
                }
                Err(_) => {
                    has_more.set(false);
                    error.set(true);
                }
            }
            loading.set(false);
        });
        filters
    });

    let load_more = Callback::new(move |()| {
        if loading.get_untracked() || loading_more.get_untracked() || !has_more.get_untracked() {
            return;
        }
        let filters = NewsFilters {
            query: query.get_untracked().trim().into(),
            feed_id: source.get_untracked().parse().ok(),
            mode: mode.get_untracked(),
        };
        let current = generation.get_untracked();
        let offset = items.with_untracked(Vec::len);
        loading_more.set(true);
        spawn_local(async move {
            let result = Api::default().items(&filters, offset).await;
            if generation.get_untracked() == current {
                match result {
                    Ok(page) => {
                        let more = items
                            .try_update(|items| {
                                append_page(items, page, &filters.mode, &filters.query)
                            })
                            .unwrap_or(false);
                        has_more.set(more);
                    }
                    // Stop auto-loading so a failing backend is not retried in a loop.
                    Err(_) => {
                        has_more.set(false);
                        error.set(true);
                    }
                }
            }
            loading_more.set(false);
        });
    });

    let mut feeds = feeds;
    feeds.sort_by_key(|f| f.name().to_lowercase());
    let feeds = StoredValue::new(feeds);
    let feed_options = feeds
        .get_value()
        .into_iter()
        .map(|feed| {
            let id = feed.id.to_string();
            let value = id.clone();
            view! {
                <option value=value selected=move || source.with(|s| *s == id)>
                    {feed.name().to_owned()}
                </option>
            }
        })
        .collect_view();
    view! {
        <div class=PANEL>
            <div class="flex flex-wrap items-center justify-between gap-2">
                <h2 class="text-sm font-semibold">"Latest News"</h2>
                <span role="status" class="text-xs text-katana-gray">
                    {move || {
                        if loading.get() {
                            "Loading...".to_owned()
                        } else {
                            format!("{} shown", items.with(Vec::len))
                        }
                    }}
                </span>
            </div>
            <div class="grid gap-2 sm:grid-cols-[1fr_120px_180px]">
                <input
                    type="search"
                    class=INPUT
                    aria-label="Search news"
                    prop:value=move || query.get()
                    placeholder=move || {
                        if mode.get() == "keyword" {
                            "Filter by title, summary, author, URL, or source"
                        } else {
                            "Search by topic or meaning"
                        }
                    }
                    on:input=move |event| query.set(event_target_value(&event))
                />
                <select
                    class=INPUT
                    aria-label="Search mode"
                    prop:value=move || mode.get()
                    on:change=move |event| mode.set(event_target_value(&event))
                >
                    <option value="hybrid" selected=move || mode.get() == "hybrid">"Hybrid"</option>
                    <option value="semantic" selected=move || mode.get() == "semantic">"Semantic"</option>
                    <option value="keyword" selected=move || mode.get() == "keyword">"Keyword"</option>
                </select>
                <select
                    class=INPUT
                    aria-label="Source"
                    prop:value=move || source.get()
                    on:change=move |event| source.set(event_target_value(&event))
                >
                    <option value="" selected=move || source.with(String::is_empty)>"All sources"</option>
                    {feed_options}
                </select>
            </div>
        </div>
        <Show when=move || error.get()>
            <ErrorNotice message="Could not load matching news." />
        </Show>
        <div class="border-y border-sumi-ink3">
            <For each=move || items.get() key=|item| item.id let:item>
                {
                    let name = feeds.with_value(|f| feed_name(f, item.feed_id));
                    view! { <ItemCard item feed_name=name now /> }
                }
            </For>
            <Show when=move || items.with(Vec::is_empty) && !loading.get()>
                <p class="px-4 py-12 text-center text-katana-gray">
                    "No news matches the current filters."
                </p>
            </Show>
        </div>
        <LoadMore
            has_more=Signal::derive(move || has_more.get() && !loading.get())
            loading=loading_more
            onload=load_more
        />
    }
}
