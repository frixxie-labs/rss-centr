use dioxus::prelude::*;

use crate::{api::Api, components::*, logic::topic_url, models::*};

#[component]
pub fn Topics() -> Element {
    let response = use_server_future(|| async {
        let api = Api::default();
        futures_util::join!(api.topics(), api.feeds())
    })?;
    let data = response.read();
    let Some((topics, feeds)) = data.as_ref() else {
        return rsx! { "Loading…" };
    };
    rsx! {
        document::Title { "RSS Centr - Topics" }
        TopicsView { initial: topics.clone(), feeds: feeds.clone().unwrap_or_default() }
    }
}

#[component]
fn TopicsView(initial: Result<Vec<Topic>, String>, feeds: Vec<Feed>) -> Element {
    let mut topics = use_signal(|| initial.clone().unwrap_or_default());
    let mut selected = use_signal(|| None::<Topic>);
    let mut loading = use_signal(|| false);
    let mut error = use_signal(|| initial.err());
    let data = topics.read();
    let min = data
        .iter()
        .map(|t| t.tf_idf)
        .reduce(f64::min)
        .unwrap_or(0.0);
    let max = data
        .iter()
        .map(|t| t.tf_idf)
        .reduce(f64::max)
        .unwrap_or(0.0);
    rsx! {
        div { class: "px-4 py-2 border-b border-sumi-ink3 flex items-center justify-between text-xs text-fuji-gray",
            span { "{data.len()} topics found in the last 24 hours" }
            button { class: BUTTON, disabled: loading(), onclick: move |_| async move {
                loading.set(true);
                error.set(None);
                match Api::default().topics().await {
                    Ok(result) => { topics.set(result); selected.set(None); }
                    Err(err) => error.set(Some(err)),
                }
                loading.set(false);
            }, if loading() { "Loading..." } else { "Refresh" } }
        }
        if let Some(message) = error() { ErrorNotice { message } }
        if data.is_empty() { p { class: "px-4 py-12 text-center text-katana-gray", "No topic data available for the last 24 hours." } }
        div { class: "px-4 py-6 flex flex-wrap items-baseline justify-center gap-x-3 gap-y-1",
            for entry in data.iter() {
                TopicWord { key: "{entry.word}", entry: entry.clone(), min, max, selected }
            }
        }
        div { class: "flex flex-wrap justify-center gap-4 px-4 pb-2 text-xs text-fuji-gray",
            span { class: "text-carp-yellow", "most distinctive" }
            span { class: "text-spring-blue", "distinctive" }
            span { class: "text-spring-green", "somewhat distinctive" }
            span { class: "text-old-white", "common" }
        }
        if let Some(entry) = selected() {
            div { class: PANEL,
                h3 { class: "text-lg font-semibold", "{entry.word}" }
                p { class: "text-sm text-fuji-gray", "{entry.total_occurrences} occurrences in {entry.document_frequency} titles across {entry.items.len()} sources" }
                p { class: "text-xs text-fuji-gray", "tf-idf score: {entry.tf_idf:.2}" }
                for source in &entry.items {
                    a { class: "block space-y-1 rounded-md px-2 py-1 transition hover:bg-sumi-ink3", href: topic_url(&entry.word, source.feed_src_id),
                        div { class: "flex justify-between text-sm",
                            span { "{feed_name(&feeds, source.feed_src_id)}" }
                            span { "{source.occurrences}x" }
                        }
                        div { class: "h-1.5 w-full rounded-full bg-sumi-ink3",
                            div { class: "h-1.5 rounded-full bg-carp-yellow", style: "width: {((source.occurrences as f64 / entry.items.iter().map(|s| s.occurrences).max().unwrap_or(1).max(1) as f64) * 100.0).max(2.0)}%" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TopicWord(entry: Topic, min: f64, max: f64, mut selected: Signal<Option<Topic>>) -> Element {
    let size = if max == min {
        1.875
    } else {
        0.75 + (entry.tf_idf - min) / (max - min) * 2.25
    };
    let ratio = if max == 0.0 { 0.0 } else { entry.tf_idf / max };
    let color = if max == 0.0 {
        "text-fuji-gray"
    } else if ratio > 0.7 {
        "text-carp-yellow"
    } else if ratio > 0.4 {
        "text-spring-blue"
    } else if ratio > 0.2 {
        "text-spring-green"
    } else {
        "text-old-white"
    };
    let is_selected = selected
        .read()
        .as_ref()
        .is_some_and(|s| s.word == entry.word);
    let decoration = if is_selected { "underline" } else { "none" };
    rsx! {
        button { r#type: "button", class: "cursor-pointer transition-all hover:opacity-80 {color}",
            style: "font-size: {size}rem; line-height: 1.3; text-decoration: {decoration}",
            aria_pressed: is_selected,
            title: "{entry.word}: {entry.total_occurrences} occurrences, tf-idf {entry.tf_idf:.2}",
            onclick: move |_| selected.set(if is_selected { None } else { Some(entry.clone()) }),
            "{entry.word}"
        }
    }
}
