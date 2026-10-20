use leptos::{prelude::*, task::spawn_local};

use crate::{
    api::Api,
    components::*,
    logic::{absolute_date, poll_interval},
    models::Feed,
};

#[derive(Clone)]
enum Action {
    Refresh,
    Add(String),
    Toggle(i64, bool),
    Ingest(i64),
    All,
    Delete(i64),
}

fn newest_first(mut feeds: Vec<Feed>) -> Vec<Feed> {
    feeds.sort_by_key(|f| std::cmp::Reverse(f.id));
    feeds
}

#[island]
pub fn SourcesView(initial: Result<Vec<Feed>, String>) -> impl IntoView {
    let feeds = RwSignal::new(newest_first(initial.clone().unwrap_or_default()));
    let url = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(initial.err());
    let success = RwSignal::new(None::<String>);
    let pending_delete = RwSignal::new(None::<i64>);
    let action = Callback::new(move |action: Action| {
        // Serialize mutations so repeated clicks cannot race refreshes or delete a
        // source while another operation is still updating it.
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        success.set(None);
        spawn_local(async move {
            let api = Api::default();
            let mut refresh = true;
            let result = match action {
                Action::Refresh => Ok("Sources refreshed.".to_owned()),
                Action::Add(value) => {
                    if value.trim().is_empty() {
                        Err("Source URL is required.".into())
                    } else {
                        api.create_feed(value.trim()).await.map(|_| {
                            url.set(String::new());
                            "Source saved.".into()
                        })
                    }
                }
                Action::Toggle(id, enabled) => api.enable_feed(id, enabled).await.map(|_| {
                    if enabled {
                        "Source enabled.".into()
                    } else {
                        "Source paused.".into()
                    }
                }),
                Action::Delete(id) => api.delete_feed(id).await.map(|_| {
                    pending_delete.set(None);
                    "Source deleted.".into()
                }),
                Action::Ingest(id) => {
                    refresh = false;
                    let paused =
                        feeds.with_untracked(|f| f.iter().any(|f| f.id == id && !f.is_enabled));
                    if paused {
                        Err("Cannot fetch a paused source. Enable it first.".into())
                    } else {
                        api.ingest(id).await.map(|_| "Fetch queued.".into())
                    }
                }
                Action::All => {
                    refresh = false;
                    let ids: Vec<_> = feeds.with_untracked(|f| {
                        f.iter().filter(|f| f.is_enabled).map(|f| f.id).collect()
                    });
                    let results =
                        futures_util::future::join_all(ids.iter().map(|id| api.ingest(*id))).await;
                    let failed = results.iter().filter(|r| r.is_err()).count();
                    if failed == 0 {
                        Ok(format!("Fetch queued for {} sources.", ids.len()))
                    } else {
                        Err(format!(
                            "Queued fetch for {} of {} sources. {failed} failed.",
                            ids.len() - failed,
                            ids.len()
                        ))
                    }
                }
            };
            match result {
                Ok(message) => {
                    success.set(Some(message));
                    if refresh {
                        match api.feeds().await {
                            Ok(result) => feeds.set(newest_first(result)),
                            Err(err) => error.set(Some(err)),
                        }
                    }
                }
                Err(err) => error.set(Some(err)),
            }
            busy.set(false);
        });
    });
    view! {
        <section class=PANEL>
            <h2 class="text-sm font-semibold">"Add source"</h2>
            <form
                class="flex flex-col gap-2 sm:flex-row"
                on:submit=move |event| {
                    event.prevent_default();
                    action.run(Action::Add(url.get_untracked()));
                }
            >
                <input
                    class=format!("{INPUT} flex-1")
                    type="url"
                    aria-label="Source URL"
                    placeholder="https://example.com/feed.xml"
                    required
                    prop:value=move || url.get()
                    on:input=move |event| url.set(event_target_value(&event))
                />
                <button class=BUTTON type="submit" disabled=move || busy.get()>
                    "Save source"
                </button>
            </form>
        </section>
        {move || error.get().map(|message| view! { <ErrorNotice message /> })}
        {move || {
            success
                .get()
                .map(|message| {
                    view! {
                        <p role="status" class="mx-4 text-sm text-spring-green">
                            {message}
                        </p>
                    }
                })
        }}
        <section class="mx-4 my-5 rounded-lg border border-sumi-ink3 bg-sumi-ink2/60">
            <div class="flex flex-wrap items-center justify-between gap-2 border-b border-sumi-ink3 px-4 py-3">
                <h2 class="text-sm font-semibold">
                    {move || format!("Sources ({})", feeds.with(Vec::len))}
                </h2>
                <div class="flex gap-2">
                    <button
                        class=BUTTON
                        disabled=move || busy.get() || !feeds.with(|f| f.iter().any(|f| f.is_enabled))
                        on:click=move |_| action.run(Action::All)
                    >
                        "Fetch all"
                    </button>
                    <button class=BUTTON disabled=move || busy.get() on:click=move |_| action.run(Action::Refresh)>
                        {move || if busy.get() { "Working..." } else { "Refresh" }}
                    </button>
                </div>
            </div>
            <Show when=move || feeds.with(Vec::is_empty)>
                <p class="px-4 py-8 text-sm text-katana-gray">
                    "No sources yet. Add one above to start collecting news."
                </p>
            </Show>
            <div class="divide-y divide-sumi-ink3">
                <For each=move || feeds.get() key=|f| f.clone() let:feed>
                    <SourceRow feed busy pending_delete action />
                </For>
            </div>
        </section>
    }
}

#[component]
fn SourceRow(
    feed: Feed,
    busy: RwSignal<bool>,
    pending_delete: RwSignal<Option<i64>>,
    action: Callback<Action>,
) -> impl IntoView {
    let id = feed.id;
    let enabled = feed.is_enabled;
    let url = feed.url.clone();
    view! {
        <article class="px-4 py-4 space-y-3">
            <div class="flex flex-wrap items-center gap-2">
                <h3 class="text-sm font-semibold">{feed.name().to_owned()}</h3>
                <span class=if enabled {
                    "rounded px-2 py-0.5 text-xs bg-winter-green text-spring-green"
                } else {
                    "rounded px-2 py-0.5 text-xs bg-sumi-ink3 text-old-white"
                }>{if enabled { "Enabled" } else { "Paused" }}</span>
            </div>
            <div class="space-y-1 text-xs text-fuji-gray">
                <p class="break-all text-old-white">{feed.url.clone()}</p>
                <p>
                    {format!(
                        "Last checked: {} | Last success: {}",
                        absolute_date(feed.last_checked_at.as_deref()),
                        absolute_date(feed.last_success_at.as_deref()),
                    )}
                </p>
                <p>
                    {format!(
                        "Check every {} | Failures: {}",
                        poll_interval(feed.poll_interval_seconds),
                        feed.failure_count,
                    )}
                </p>
            </div>
            <div class="flex flex-wrap gap-2">
                <button class=BUTTON disabled=move || busy.get() on:click=move |_| action.run(Action::Toggle(id, !enabled))>
                    {if enabled { "Pause" } else { "Enable" }}
                </button>
                <button class=BUTTON disabled=move || busy.get() || !enabled on:click=move |_| action.run(Action::Ingest(id))>
                    "Fetch now"
                </button>
                <button
                    class=format!("{BUTTON} text-wave-red")
                    disabled=move || busy.get()
                    on:click=move |_| pending_delete.set(Some(id))
                >
                    "Delete"
                </button>
            </div>
            <Show when=move || pending_delete.get() == Some(id)>
                <div role="alert" class="rounded border border-autumn-red p-3 space-y-2">
                    <p class="text-sm">{format!("Delete source? {url}")}</p>
                    <button
                        class=format!("{BUTTON} text-wave-red")
                        disabled=move || busy.get()
                        on:click=move |_| action.run(Action::Delete(id))
                    >
                        "Confirm delete"
                    </button>
                    <button class=BUTTON disabled=move || busy.get() on:click=move |_| pending_delete.set(None)>
                        "Cancel"
                    </button>
                </div>
            </Show>
        </article>
    }
}
