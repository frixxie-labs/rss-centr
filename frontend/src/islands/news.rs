use leptos::{prelude::*, task::spawn_local};

use crate::{api::Api, components::*, hooks::*, logic::*, models::*};

#[island]
pub fn NewsView(
    initial: NewsFilters,
    initial_items: Result<Vec<FeedItem>, String>,
    feeds: FeedNames,
    now: i64,
) -> impl IntoView {
    let server_loaded = initial_items.is_ok();
    let initial_items = initial_items.unwrap_or_default();
    let query = RwSignal::new(initial.query.clone());
    let source = RwSignal::new(initial.feed_id.map(|id| id.to_string()).unwrap_or_default());
    let mode = RwSignal::new(initial.mode);
    let has_more = RwSignal::new(initial_items.len() >= PAGE_SIZE);
    let items = RwSignal::new(display_order(initial_items, initial.order()));
    let error = RwSignal::new(!server_loaded);
    let loading = RwSignal::new(false);
    let loading_more = RwSignal::new(false);
    // Bumped on every new search; responses for an older generation are discarded,
    // which also covers in-flight "load more" pages and debounced searches.
    let generation = StoredValue::new(0_u64);
    let now = use_clock(now);
    let filters = Memo::new(move |_| NewsFilters {
        query: query.get().trim().into(),
        feed_id: source.get().parse().ok(),
        mode: mode.get(),
    });

    let search = move |filters: NewsFilters, debounce: bool| {
        generation.update_value(|g| *g += 1);
        let current = generation.get_value();
        spawn_local(async move {
            if debounce {
                sleep_ms(250).await;
            }
            if generation.try_get_value() != Some(current) {
                return;
            }
            #[cfg(target_arch = "wasm32")]
            crate::browser::replace_url(&filters.page_url());
            loading.set(true);
            error.set(false);
            let result = Api::default().items(&filters, 0).await;
            if generation.try_get_value() != Some(current) {
                return;
            }
            match result {
                Ok(result) => {
                    has_more.set(result.len() >= PAGE_SIZE);
                    items.set(display_order(result, filters.order()));
                }
                Err(_) => {
                    has_more.set(false);
                    error.set(true);
                }
            }
            loading.set(false);
        });
    };

    Effect::new(move |previous: Option<NewsFilters>| {
        let current = filters.get();
        match &previous {
            // The server already rendered these filters; only retry if that failed.
            None if server_loaded => {}
            None => search(current.clone(), false),
            // Only typing is debounced; dropdown changes search immediately.
            Some(previous) => search(current.clone(), previous.query != current.query),
        }
        current
    });

    let load_more = Callback::new(move |()| {
        if loading.get_untracked() || loading_more.get_untracked() || !has_more.get_untracked() {
            return;
        }
        let filters = filters.get_untracked();
        let current = generation.get_value();
        let offset = items.with_untracked(Vec::len);
        loading_more.set(true);
        spawn_local(async move {
            let result = Api::default().items(&filters, offset).await;
            if generation.try_get_value() == Some(current) {
                match result {
                    Ok(page) => {
                        let more = items
                            .try_update(|items| append_page(items, page, filters.order()))
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

    let mut sources: Vec<_> = feeds.iter().map(|(id, name)| (*id, name.clone())).collect();
    sources.sort_by_key(|(_, name)| name.to_lowercase());
    let source_options = sources
        .into_iter()
        .map(|(id, name)| {
            let value = id.to_string();
            let selected = value.clone();
            view! {
                <option value=value selected=move || source.with(|s| *s == selected)>
                    {name}
                </option>
            }
        })
        .collect_view();
    let mode_options = SearchMode::ALL
        .into_iter()
        .map(|option| {
            view! {
                <option value=option.as_str() selected=move || mode.get() == option>
                    {option.label()}
                </option>
            }
        })
        .collect_view();
    let feeds = StoredValue::new(feeds);
    view! {
        <div class=PANEL>
            <div class="flex flex-wrap items-center justify-between gap-2">
                <h2 class="text-sm font-semibold">"Latest News"</h2>
                <span role="status" class="text-xs text-katana-gray">
                    {move || {
                        if loading.get() {
                            LOADING.to_owned()
                        } else {
                            format!("{} shown", items.with(Vec::len))
                        }
                    }}
                </span>
            </div>
            <div class="grid gap-2 sm:grid-cols-[1fr_120px_180px]">
                // `prop:value` is client-only; `value` puts the query into the server HTML.
                <input
                    type="search"
                    class=INPUT
                    aria-label="Search news"
                    value=initial.query
                    prop:value=move || query.get()
                    placeholder=move || {
                        if mode.get() == SearchMode::Keyword {
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
                    prop:value=move || mode.get().as_str()
                    on:change=move |event| {
                        if let Some(value) = SearchMode::parse(&event_target_value(&event)) {
                            mode.set(value);
                        }
                    }
                >
                    {mode_options}
                </select>
                <select
                    class=INPUT
                    aria-label="Source"
                    prop:value=move || source.get()
                    on:change=move |event| source.set(event_target_value(&event))
                >
                    <option value="" selected=move || source.with(String::is_empty)>"All sources"</option>
                    {source_options}
                </select>
            </div>
        </div>
        <Show when=move || error.get()>
            <ErrorNotice message="Could not load matching news." />
        </Show>
        <div class="border-y border-sumi-ink3">
            <For each=move || items.get() key=|item| item.id let:item>
                {
                    let name = feeds.with_value(|names| feed_name(names, item.feed_id));
                    view! { <ItemCard item feed_name=name now /> }
                }
            </For>
            <Show when=move || items.with(Vec::is_empty) && !loading.get() && !error.get()>
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
