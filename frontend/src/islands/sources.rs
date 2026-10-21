use leptos::prelude::*;

use crate::{
    api::Api,
    components::*,
    logic::{absolute_date, poll_interval},
    models::Feed,
};

#[derive(Clone)]
enum SourceAction {
    Refresh,
    Add(String),
    Toggle(i64, bool),
    Ingest(i64),
    All,
    Delete(i64),
}

impl SourceAction {
    fn reloads_list(&self) -> bool {
        !matches!(self, Self::Ingest(_) | Self::All)
    }
}

fn newest_first(mut feeds: Vec<Feed>) -> Vec<Feed> {
    feeds.sort_by_key(|f| std::cmp::Reverse(f.id));
    feeds
}

/// Runs the request for `action` and returns the success message.
async fn perform(
    api: &Api,
    action: &SourceAction,
    feeds: RwSignal<Vec<Feed>>,
) -> Result<String, String> {
    match action {
        SourceAction::Refresh => Ok("Sources refreshed.".into()),
        SourceAction::Add(url) if url.trim().is_empty() => Err("Source URL is required.".into()),
        SourceAction::Add(url) => api
            .create_feed(url.trim())
            .await
            .map(|_| "Source saved.".into()),
        SourceAction::Toggle(id, enabled) => {
            api.enable_feed(*id, *enabled).await?;
            Ok(if *enabled {
                "Source enabled."
            } else {
                "Source paused."
            }
            .into())
        }
        SourceAction::Delete(id) => api.delete_feed(*id).await.map(|_| "Source deleted.".into()),
        SourceAction::Ingest(id) => {
            let paused = feeds.with_untracked(|f| f.iter().any(|f| f.id == *id && !f.is_enabled));
            if paused {
                return Err("Cannot fetch a paused source. Enable it first.".into());
            }
            api.ingest(*id).await.map(|_| "Fetch queued.".into())
        }
        SourceAction::All => {
            let ids: Vec<_> =
                feeds.with_untracked(|f| f.iter().filter(|f| f.is_enabled).map(|f| f.id).collect());
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
    }
}

#[island]
pub fn SourcesView(initial: Result<Vec<Feed>, String>) -> impl IntoView {
    // Stays false until the list loads, so a failed load is not shown as "no sources".
    let loaded = RwSignal::new(initial.is_ok());
    let (initial, initial_error) = match initial {
        Ok(feeds) => (newest_first(feeds), None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let feeds = RwSignal::new(initial);
    let url = RwSignal::new(String::new());
    let pending_delete = RwSignal::new(None::<i64>);
    let action = Action::new_local(move |action: &SourceAction| {
        let action = action.clone();
        async move {
            let api = Api::default();
            let message = perform(&api, &action, feeds).await?;
            match action {
                SourceAction::Add(_) => url.set(String::new()),
                SourceAction::Delete(_) => pending_delete.set(None),
                _ => {}
            }
            if action.reloads_list() {
                let list = api
                    .feeds()
                    .await
                    .map_err(|error| format!("{message} Could not reload sources: {error}"))?;
                feeds.set(newest_first(list));
                loaded.set(true);
            }
            Ok(message)
        }
    });
    let pending = action.pending();
    let busy = Signal::from(pending);
    let result = action.value();
    // Serialize mutations so repeated clicks cannot race refreshes or delete a
    // source while another operation is still updating it.
    let run = Callback::new(move |input: SourceAction| {
        if !pending.get_untracked() {
            action.dispatch(input);
        }
    });
    let feedback = move || {
        if pending.get() {
            return None;
        }
        result.with(|result| match result {
            Some(result) => Some(result.clone()),
            None => initial_error.clone().map(Err),
        })
    };
    view! {
        <section class=PANEL>
            <h2 class="text-sm font-semibold">"Add source"</h2>
            <form
                class="flex flex-col gap-2 sm:flex-row"
                on:submit=move |event| {
                    event.prevent_default();
                    run.run(SourceAction::Add(url.get_untracked()));
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
        {move || {
            feedback()
                .map(|result| match result {
                    Ok(message) => {
                        view! {
                            <p role="status" class="mx-4 text-sm text-spring-green">
                                {message}
                            </p>
                        }
                            .into_any()
                    }
                    Err(message) => view! { <ErrorNotice message /> }.into_any(),
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
                        on:click=move |_| run.run(SourceAction::All)
                    >
                        "Fetch all"
                    </button>
                    <button class=BUTTON disabled=move || busy.get() on:click=move |_| run.run(SourceAction::Refresh)>
                        {move || if busy.get() { "Working…" } else { "Refresh" }}
                    </button>
                </div>
            </div>
            <Show when=move || loaded.get() && feeds.with(Vec::is_empty)>
                <p class="px-4 py-8 text-sm text-katana-gray">
                    "No sources yet. Add one above to start collecting news."
                </p>
            </Show>
            <div class="divide-y divide-sumi-ink3">
                // Keyed by id so rows (and keyboard focus) survive updates; each row
                // follows its own feed and only re-renders the parts that changed.
                <For each=move || feeds.get() key=|feed| feed.id let:feed>
                    {
                        let id = feed.id;
                        let feed = Memo::new(move |_| {
                            feeds
                                .with(|list| list.iter().find(|f| f.id == id).cloned())
                                .unwrap_or_else(|| feed.clone())
                        });
                        view! { <SourceRow feed busy pending_delete action=run /> }
                    }
                </For>
            </div>
        </section>
    }
}

#[component]
fn SourceRow(
    feed: Memo<Feed>,
    busy: Signal<bool>,
    pending_delete: RwSignal<Option<i64>>,
    action: Callback<SourceAction>,
) -> impl IntoView {
    let id = feed.with_untracked(|f| f.id);
    let enabled = Memo::new(move |_| feed.with(|f| f.is_enabled));
    let url = move || feed.with(|f| f.url.clone());
    view! {
        <article class="px-4 py-4 space-y-3">
            <div class="flex flex-wrap items-center gap-2">
                <h3 class="text-sm font-semibold">{move || feed.with(|f| f.name().to_owned())}</h3>
                <span
                    class="rounded px-2 py-0.5 text-xs"
                    class=("bg-winter-green", move || enabled.get())
                    class=("text-spring-green", move || enabled.get())
                    class=("bg-sumi-ink3", move || !enabled.get())
                    class=("text-old-white", move || !enabled.get())
                >
                    {move || if enabled.get() { "Enabled" } else { "Paused" }}
                </span>
            </div>
            <div class="space-y-1 text-xs text-fuji-gray">
                <p class="break-all text-old-white">{url}</p>
                <p>
                    {move || {
                        feed.with(|f| {
                            format!(
                                "Last checked: {} | Last success: {}",
                                absolute_date(f.last_checked_at.as_deref()),
                                absolute_date(f.last_success_at.as_deref()),
                            )
                        })
                    }}
                </p>
                <p>
                    {move || {
                        feed.with(|f| {
                            format!(
                                "Check every {} | Failures: {}",
                                poll_interval(f.poll_interval_seconds),
                                f.failure_count,
                            )
                        })
                    }}
                </p>
            </div>
            <div class="flex flex-wrap gap-2">
                <button
                    class=BUTTON
                    disabled=move || busy.get()
                    on:click=move |_| action.run(SourceAction::Toggle(id, !enabled.get_untracked()))
                >
                    {move || if enabled.get() { "Pause" } else { "Enable" }}
                </button>
                <button
                    class=BUTTON
                    disabled=move || busy.get() || !enabled.get()
                    on:click=move |_| action.run(SourceAction::Ingest(id))
                >
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
                <div
                    role="group"
                    aria-label="Confirm deletion"
                    class="rounded border border-autumn-red p-3 space-y-2"
                >
                    <p class="text-sm">{move || format!("Delete source? {}", url())}</p>
                    <button
                        class=format!("{BUTTON} text-wave-red")
                        disabled=move || busy.get()
                        on:click=move |_| action.run(SourceAction::Delete(id))
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
