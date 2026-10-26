//! Server-only route components. They fetch on the server and hand the data to islands.
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;

use crate::{api::Api, components::*, islands::*, logic::*, models::*};

fn use_api() -> Api {
    use_context::<Api>().unwrap_or_default()
}

fn render_time() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn loading() -> impl IntoView {
    view! { <p class="p-4 text-fuji-gray" role="status">{LOADING}</p> }
}

#[component]
pub fn Summary() -> impl IntoView {
    let api = use_api();
    view! {
        <Title text="RSS Centr - AI Summary" />
        <section class=PANEL aria-labelledby="daily-summary-title">
            <SummaryControls />
            <Suspense fallback=|| view! { <p role="status">"Loading latest overview…"</p> }>
                {Suspend::new(async move {
                    match api.summary().await {
                        Ok(Some(summary)) => {
                            view! {
                                <p class="mt-2 whitespace-pre-line text-sm leading-relaxed text-fuji-gray">
                                    {summary.summary.clone()}
                                </p>
                                <p class="mt-3 text-xs text-katana-gray">
                                    {format!(
                                        "Generated {} using {} · {} sources",
                                        absolute_date(Some(summary.generated_at)),
                                        summary.model,
                                        summary.feed_ids.len(),
                                    )}
                                </p>
                            }
                                .into_any()
                        }
                        Ok(None) => {
                            view! {
                                <p role="status" class="text-sm text-fuji-gray">
                                    "No summary is available yet. The next scheduled generation will appear here."
                                </p>
                            }
                                .into_any()
                        }
                        Err(_) => view! { <ErrorNotice message="Could not load the overview." /> }.into_any(),
                    }
                })}
            </Suspense>
        </section>
    }
}

#[component]
pub fn Timeline() -> impl IntoView {
    let api = use_api();
    view! {
        <Title text="RSS Centr - Live Timeline" />
        <Suspense fallback=loading>
            {Suspend::new(async move {
                let filters = NewsFilters::default();
                let (items, feeds) = futures_util::join!(api.items(&filters, 0), api.feeds());
                view! {
                    <TimelineView
                        initial=items
                        feeds=feed_names(&feeds.unwrap_or_default())
                        now=render_time()
                    />
                }
            })}
        </Suspense>
    }
}

#[component]
pub fn News() -> impl IntoView {
    let api = use_api();
    let filters = use_query_map().with_untracked(|params| {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(params.clone())
            .finish();
        NewsFilters::parse(&query)
    });
    view! {
        <Title text="RSS Centr - All News" />
        <Suspense fallback=loading>
            {Suspend::new(async move {
                let (items, feeds) = futures_util::join!(api.items(&filters, 0), api.feeds());
                view! {
                    <NewsView
                        initial=filters
                        initial_items=items
                        feeds=feed_names(&feeds.unwrap_or_default())
                        now=render_time()
                    />
                }
            })}
        </Suspense>
    }
}

#[component]
pub fn Sources() -> impl IntoView {
    let api = use_api();
    view! {
        <Title text="RSS Centr - Sources" />
        <Suspense fallback=loading>
            {Suspend::new(async move { view! { <SourcesView initial=api.feeds().await /> } })}
        </Suspense>
    }
}

#[component]
pub fn Topics() -> impl IntoView {
    let api = use_api();
    view! {
        <Title text="RSS Centr - Topics" />
        <Suspense fallback=loading>
            {Suspend::new(async move {
                let (topics, feeds): (Result<Vec<Topic>, String>, _) = futures_util::join!(
                    api.topics(), api.feeds()
                );
                view! { <TopicsView initial=topics feeds=feed_names(&feeds.unwrap_or_default()) /> }
            })}
        </Suspense>
    }
}
