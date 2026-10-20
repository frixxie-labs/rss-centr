use dioxus::prelude::*;

use crate::{api::Api, components::*, logic::*, models::*};

#[component]
pub fn News(query: String) -> Element {
    let initial = NewsFilters::parse(&query);
    let filters = initial.clone();
    let response = use_server_future(use_reactive((&query,), move |(query,)| async move {
        let api = Api::default();
        let filters = NewsFilters::parse(&query);
        let (items, feeds) = futures_util::join!(api.items(&filters, 0), api.feeds());
        (items, feeds)
    }))?;
    let data = response.read();
    let Some((items, feeds)) = data.as_ref() else {
        return rsx! { "Loading…" };
    };
    rsx! {
        NewsView { key: "{query}", initial: filters, initial_items: items.clone(), feeds: feeds.clone().unwrap_or_default() }
    }
}

#[component]
fn NewsView(
    initial: NewsFilters,
    initial_items: Result<Vec<FeedItem>, String>,
    feeds: Vec<Feed>,
) -> Element {
    let mut query = use_signal(|| initial.query.clone());
    let mut source = use_signal(|| initial.feed_id.map(|id| id.to_string()).unwrap_or_default());
    let mut mode = use_signal(|| initial.mode.clone());
    let mut items = use_signal(|| {
        display_order(
            initial_items.clone().unwrap_or_default(),
            &initial.mode,
            &initial.query,
        )
    });
    let mut error = use_signal(|| initial_items.is_err());
    let mut loading = use_signal(|| false);
    let mut has_more = use_signal(|| {
        initial_items
            .as_ref()
            .is_ok_and(|items| items.len() >= PAGE_SIZE)
    });
    let mut loading_more = use_signal(|| false);
    // Bumped on every new search so an in-flight page for old filters is discarded.
    let mut generation = use_signal(|| 0_u64);
    let first = use_hook(|| std::rc::Rc::new(std::cell::Cell::new(true)));
    let now = use_clock();
    // A reactive resource cancels its previous task on filter changes. Its request's
    // dropped future cannot publish stale results after a newer search starts.
    use_resource(move || {
        let filters = NewsFilters {
            query: query().trim().into(),
            feed_id: source().parse().ok(),
            mode: mode(),
        };
        let skip = first.replace(false) && initial_items.is_ok();
        async move {
            if skip {
                return;
            }
            #[cfg(all(feature = "web", target_arch = "wasm32"))]
            gloo_timers::future::TimeoutFuture::new(250).await;
            *generation.write() += 1;
            loading.set(true);
            error.set(false);
            match Api::default().items(&filters, 0).await {
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
        }
    });
    let load_more = move |_| {
        if *loading.peek() || *loading_more.peek() || !*has_more.peek() {
            return;
        }
        let filters = NewsFilters {
            query: query.peek().trim().into(),
            feed_id: source.peek().parse().ok(),
            mode: mode.peek().clone(),
        };
        let current = *generation.peek();
        let offset = items.peek().len();
        loading_more.set(true);
        spawn(async move {
            let result = Api::default().items(&filters, offset).await;
            if *generation.peek() == current {
                match result {
                    Ok(page) => has_more.set(append_page(
                        &mut items.write(),
                        page,
                        &filters.mode,
                        &filters.query,
                    )),
                    Err(_) => error.set(true),
                }
            }
            loading_more.set(false);
        });
    };
    let mut feeds = feeds;
    feeds.sort_by_key(|f| f.name().to_lowercase());
    rsx! {
        document::Title { "RSS Centr - All News" }
        div { class: PANEL,
            div { class: "flex flex-wrap items-center justify-between gap-2",
                h2 { class: "text-sm font-semibold", "Latest News" }
                span { role: "status", class: "text-xs text-katana-gray", if loading() { "Loading..." } else { "{items.read().len()} shown" } }
            }
            div { class: "grid gap-2 sm:grid-cols-[1fr_120px_180px]",
                input { r#type: "search", class: INPUT, aria_label: "Search news", value: "{query}",
                    placeholder: if mode() == "keyword" { "Filter by title, summary, author, URL, or source" } else { "Search by topic or meaning" },
                    oninput: move |event| query.set(event.value())
                }
                select { class: INPUT, aria_label: "Search mode", value: "{mode}", onchange: move |event| mode.set(event.value()),
                    option { value: "hybrid", selected: mode() == "hybrid", "Hybrid" }
                    option { value: "semantic", selected: mode() == "semantic", "Semantic" }
                    option { value: "keyword", selected: mode() == "keyword", "Keyword" }
                }
                select { class: INPUT, aria_label: "Source", value: "{source}", onchange: move |event| source.set(event.value()),
                    option { value: "", selected: source().is_empty(), "All sources" }
                    for feed in &feeds { option { value: "{feed.id}", selected: source() == feed.id.to_string(), "{feed.name()}" } }
                }
            }
        }
        if error() { ErrorNotice { message: "Could not load matching news." } }
        div { class: "border-y border-sumi-ink3",
            for item in items.read().iter() {
                ItemCard { key: "{item.id}", item: item.clone(), feed_name: feed_name(&feeds, item.feed_id), now: now() }
            }
            if items.read().is_empty() && !loading() { p { class: "px-4 py-12 text-center text-katana-gray", "No news matches the current filters." } }
        }
        LoadMore { has_more: has_more() && !loading(), loading: loading_more(), onload: load_more }
    }
}
