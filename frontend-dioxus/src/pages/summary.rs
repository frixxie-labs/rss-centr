use dioxus::prelude::*;

use crate::{api::Api, components::*, logic::absolute_date};

#[component]
pub fn Summary() -> Element {
    let response = use_server_future(|| async { Api::default().summary().await })?;
    let mut busy = use_signal(|| false);
    let mut status = use_signal(|| None::<Result<String, String>>);
    let data = response.read();
    rsx! {
        document::Title { "RSS Centr - AI Summary" }
        section { class: PANEL, aria_labelledby: "daily-summary-title",
            div { class: "flex items-center justify-between gap-2",
                h2 { id: "daily-summary-title", class: "text-sm font-semibold text-fuji-white", "Last 24 hours" }
                button { class: BUTTON, disabled: busy(), onclick: move |_| async move {
                    busy.set(true);
                    status.set(None);
                    status.set(Some(match Api::default().generate_summary().await {
                        Ok(true) => Ok("Generating a new summary. Reload the page in a minute to see it.".into()),
                        Ok(false) => Ok("A summary is already being generated.".into()),
                        Err(err) => Err(err),
                    }));
                    busy.set(false);
                }, if busy() { "Starting..." } else { "Generate now" } }
            }
            match status() {
                Some(Ok(message)) => rsx! { p { role: "status", class: "mt-2 text-xs text-fuji-gray", "{message}" } },
                Some(Err(_)) => rsx! { ErrorNotice { message: "Could not start summary generation." } },
                None => rsx! {},
            }
            match data.as_ref() {
                Some(Ok(Some(summary))) => rsx! {
                    p { class: "mt-2 whitespace-pre-line text-sm leading-relaxed text-fuji-gray", "{summary.summary}" }
                    p { class: "mt-3 text-xs text-katana-gray", "Generated {absolute_date(Some(&summary.generated_at))} using {summary.model} · {summary.feed_ids.len()} sources" }
                },
                Some(Ok(None)) => rsx! { p { role: "status", class: "text-sm text-fuji-gray", "No summary is available yet. The next scheduled generation will appear here." } },
                Some(Err(_)) => rsx! { ErrorNotice { message: "Could not load the overview." } },
                None => rsx! { p { role: "status", "Loading latest overview…" } },
            }
        }
    }
}
