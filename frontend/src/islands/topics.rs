use leptos::prelude::*;

use crate::{
    api::Api,
    components::*,
    hooks::reload_error,
    logic::{FeedNames, feed_name, topic_url},
    models::*,
};

#[island]
pub fn TopicsView(initial: Result<Vec<Topic>, String>, feeds: FeedNames) -> impl IntoView {
    // Stays false until the list loads, so a failed load is not shown as "no topics".
    let loaded = RwSignal::new(initial.is_ok());
    let (initial, initial_error) = match initial {
        Ok(topics) => (topics, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let topics = RwSignal::new(initial);
    let selected = RwSignal::new(None::<Topic>);
    let feeds = StoredValue::new(feeds);
    let refresh = Action::new_local(move |_: &()| async move {
        let result = Api::default().topics().await?;
        topics.try_set(result);
        selected.try_set(None);
        loaded.try_set(true);
        Ok(())
    });
    let loading = refresh.pending();
    let error = reload_error(refresh, initial_error);
    // Selecting a word only notifies the two words whose state changed.
    let selection = Selector::new(move || selected.with(|s| s.as_ref().map(|t| t.word.clone())));
    let range = Memo::new(move |_| {
        topics.with(|data| {
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
            (min, max)
        })
    });
    view! {
        <div class="px-4 py-2 border-b border-sumi-ink3 flex items-center justify-between text-xs text-fuji-gray">
            <span>{move || format!("{} topics found in the last 24 hours", topics.with(Vec::len))}</span>
            <button
                class=BUTTON
                disabled=move || loading.get()
                on:click=move |_| {
                    refresh.dispatch(());
                }
            >
                {move || if loading.get() { LOADING } else { "Refresh" }}
            </button>
        </div>
        {move || error.get().map(|message| view! { <ErrorNotice message /> })}
        <Show when=move || loaded.get() && topics.with(Vec::is_empty)>
            <p class="px-4 py-12 text-center text-katana-gray">
                "No topic data available for the last 24 hours."
            </p>
        </Show>
        <div class="px-4 py-6 flex flex-wrap items-baseline justify-center gap-x-3 gap-y-1">
            <For each=move || topics.get() key=|t| t.word.clone() let:entry>
                <TopicWord entry range selected selection=selection.clone() />
            </For>
        </div>
        <div class="flex flex-wrap justify-center gap-4 px-4 pb-2 text-xs text-fuji-gray">
            <span class="text-carp-yellow">"most distinctive"</span>
            <span class="text-spring-blue">"distinctive"</span>
            <span class="text-spring-green">"somewhat distinctive"</span>
            <span class="text-old-white">"common"</span>
        </div>
        {move || selected.get().map(|entry| view! { <TopicDetail entry feeds /> })}
    }
}

#[component]
fn TopicDetail(entry: Topic, feeds: StoredValue<FeedNames>) -> impl IntoView {
    let top = entry
        .items
        .iter()
        .map(|s| s.occurrences)
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let sources = entry
        .items
        .iter()
        .map(|source| {
            let width = ((source.occurrences as f64 / top) * 100.0).max(2.0);
            view! {
                <a
                    class="block space-y-1 rounded-md px-2 py-1 transition hover:bg-sumi-ink3"
                    href=topic_url(&entry.word, source.feed_src_id)
                >
                    <div class="flex justify-between text-sm">
                        <span>{feeds.with_value(|names| feed_name(names, source.feed_src_id))}</span>
                        <span>{format!("{}x", source.occurrences)}</span>
                    </div>
                    <div class="h-1.5 w-full rounded-full bg-sumi-ink3">
                        <div class="h-1.5 rounded-full bg-carp-yellow" style:width=format!("{width}%")></div>
                    </div>
                </a>
            }
        })
        .collect_view();
    view! {
        <div class=PANEL>
            <h3 class="text-lg font-semibold">{entry.word.clone()}</h3>
            <p class="text-sm text-fuji-gray">
                {format!(
                    "{} occurrences in {} titles across {} sources",
                    entry.total_occurrences,
                    entry.document_frequency,
                    entry.items.len(),
                )}
            </p>
            <p class="text-xs text-fuji-gray">{format!("tf-idf score: {:.2}", entry.tf_idf)}</p>
            {sources}
        </div>
    }
}

#[component]
fn TopicWord(
    entry: Topic,
    range: Memo<(f64, f64)>,
    selected: RwSignal<Option<Topic>>,
    selection: Selector<Option<String>>,
) -> impl IntoView {
    let tf_idf = entry.tf_idf;
    let word = entry.word.clone();
    let key = Some(word.clone());
    on_cleanup({
        let selection = selection.clone();
        let key = key.clone();
        move || selection.remove(&key)
    });
    let is_selected = Signal::derive(move || selection.selected(&key));
    let font_size = move || {
        let (min, max) = range.get();
        let size = if max == min {
            1.875
        } else {
            0.75 + (tf_idf - min) / (max - min) * 2.25
        };
        format!("{size}rem")
    };
    let class = move || {
        let (_, max) = range.get();
        let ratio = if max == 0.0 { 0.0 } else { tf_idf / max };
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
        let underline = if is_selected.get() { " underline" } else { "" };
        format!("cursor-pointer leading-[1.3] transition-all hover:opacity-80 {color}{underline}")
    };
    let title = format!(
        "{}: {} occurrences, tf-idf {:.2}",
        entry.word, entry.total_occurrences, entry.tf_idf
    );
    view! {
        <button
            type="button"
            class=class
            style:font-size=font_size
            aria-pressed=move || is_selected.get().to_string()
            title=title
            on:click=move |_| {
                selected.set(if is_selected.get_untracked() { None } else { Some(entry.clone()) })
            }
        >
            {word}
        </button>
    }
}
