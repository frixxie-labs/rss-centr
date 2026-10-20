use dioxus::prelude::*;

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

#[component]
pub fn Sources() -> Element {
    let response = use_server_future(|| async { Api::default().feeds().await })?;
    let data = response.read();
    let Some(initial) = data.as_ref() else {
        return rsx! { "Loading…" };
    };
    rsx! { document::Title { "RSS Centr - Sources" } SourcesView { initial: initial.clone() } }
}

#[component]
fn SourcesView(initial: Result<Vec<Feed>, String>) -> Element {
    let mut feeds = use_signal(|| {
        let mut feeds = initial.clone().unwrap_or_default();
        feeds.sort_by_key(|f| std::cmp::Reverse(f.id));
        feeds
    });
    let mut url = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| initial.err());
    let mut success = use_signal(|| None::<String>);
    let mut pending_delete = use_signal(|| None::<i64>);
    let mut action = move |action: Action| {
        // Serialize mutations so repeated clicks cannot race refreshes or delete a
        // source while another operation is still updating it.
        if busy() {
            return;
        }
        busy.set(true);
        error.set(None);
        success.set(None);
        spawn(async move {
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
                    if feeds.read().iter().any(|f| f.id == id && !f.is_enabled) {
                        Err("Cannot fetch a paused source. Enable it first.".into())
                    } else {
                        api.ingest(id).await.map(|_| "Fetch queued.".into())
                    }
                }
                Action::All => {
                    refresh = false;
                    let ids: Vec<_> = feeds
                        .read()
                        .iter()
                        .filter(|f| f.is_enabled)
                        .map(|f| f.id)
                        .collect();
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
                            Ok(mut result) => {
                                result.sort_by_key(|f| std::cmp::Reverse(f.id));
                                feeds.set(result);
                            }
                            Err(err) => error.set(Some(err)),
                        }
                    }
                }
                Err(err) => error.set(Some(err)),
            }
            busy.set(false);
        });
    };
    rsx! {
        section { class: PANEL,
            h2 { class: "text-sm font-semibold", "Add source" }
            form { class: "flex flex-col gap-2 sm:flex-row", onsubmit: move |event| { event.prevent_default(); action(Action::Add(url())); },
                input { class: "{INPUT} flex-1", r#type: "url", aria_label: "Source URL", placeholder: "https://example.com/feed.xml", required: true, value: "{url}", oninput: move |event| url.set(event.value()) }
                button { class: BUTTON, r#type: "submit", disabled: busy(), "Save source" }
            }
        }
        if let Some(message) = error() { ErrorNotice { message } }
        if let Some(message) = success() { p { role: "status", class: "mx-4 text-sm text-spring-green", "{message}" } }
        section { class: "mx-4 my-5 rounded-lg border border-sumi-ink3 bg-sumi-ink2/60",
            div { class: "flex flex-wrap items-center justify-between gap-2 border-b border-sumi-ink3 px-4 py-3",
                h2 { class: "text-sm font-semibold", "Sources ({feeds.read().len()})" }
                div { class: "flex gap-2",
                    button { class: BUTTON, disabled: busy() || !feeds.read().iter().any(|f| f.is_enabled), onclick: move |_| action(Action::All), "Fetch all" }
                    button { class: BUTTON, disabled: busy(), onclick: move |_| action(Action::Refresh), if busy() { "Working..." } else { "Refresh" } }
                }
            }
            if feeds.read().is_empty() { p { class: "px-4 py-8 text-sm text-katana-gray", "No sources yet. Add one above to start collecting news." } }
            div { class: "divide-y divide-sumi-ink3",
                for feed in feeds.read().iter() {
                    article { key: "{feed.id}", class: "px-4 py-4 space-y-3",
                        div { class: "flex flex-wrap items-center gap-2",
                            h3 { class: "text-sm font-semibold", "{feed.name()}" }
                            span { class: if feed.is_enabled { "rounded px-2 py-0.5 text-xs bg-winter-green text-spring-green" } else { "rounded px-2 py-0.5 text-xs bg-sumi-ink3 text-old-white" }, if feed.is_enabled { "Enabled" } else { "Paused" } }
                        }
                        div { class: "space-y-1 text-xs text-fuji-gray",
                            p { class: "break-all text-old-white", "{feed.url}" }
                            p { "Last checked: {absolute_date(feed.last_checked_at.as_deref())} | Last success: {absolute_date(feed.last_success_at.as_deref())}" }
                            p { "Check every {poll_interval(feed.poll_interval_seconds)} | Failures: {feed.failure_count}" }
                        }
                        SourceActions { feed: feed.clone(), busy: busy(), pending_delete, onaction: action }
                    }
                }
            }
        }
    }
}

#[component]
fn SourceActions(
    feed: Feed,
    busy: bool,
    mut pending_delete: Signal<Option<i64>>,
    onaction: EventHandler<Action>,
) -> Element {
    let id = feed.id;
    let enabled = feed.is_enabled;
    rsx! {
        div { class: "flex flex-wrap gap-2",
            button { class: BUTTON, disabled: busy, onclick: move |_| onaction.call(Action::Toggle(id, !enabled)), if enabled { "Pause" } else { "Enable" } }
            button { class: BUTTON, disabled: busy || !enabled, onclick: move |_| onaction.call(Action::Ingest(id)), "Fetch now" }
            button { class: "{BUTTON} text-wave-red", disabled: busy, onclick: move |_| pending_delete.set(Some(id)), "Delete" }
        }
        if pending_delete() == Some(id) {
            div { role: "alert", class: "rounded border border-autumn-red p-3 space-y-2",
                p { class: "text-sm", "Delete source? {feed.url}" }
                button { class: "{BUTTON} text-wave-red", disabled: busy, onclick: move |_| onaction.call(Action::Delete(id)), "Confirm delete" }
                button { class: BUTTON, disabled: busy, onclick: move |_| pending_delete.set(None), "Cancel" }
            }
        }
    }
}
