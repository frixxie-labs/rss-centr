use leptos::{prelude::*, task::spawn_local};

use crate::{api::Api, components::*};

/// Heading row with the "Generate now" button; the summary text itself stays server-only.
#[island]
pub fn SummaryControls() -> impl IntoView {
    let busy = RwSignal::new(false);
    let status = RwSignal::new(None::<Result<String, String>>);
    let generate = move |_| {
        busy.set(true);
        status.set(None);
        spawn_local(async move {
            status.set(Some(match Api::default().generate_summary().await {
                Ok(true) => {
                    Ok("Generating a new summary. Reload the page in a minute to see it.".into())
                }
                Ok(false) => Ok("A summary is already being generated.".into()),
                Err(err) => Err(err),
            }));
            busy.set(false);
        });
    };
    view! {
        <div class="flex items-center justify-between gap-2">
            <h2 id="daily-summary-title" class="text-sm font-semibold text-fuji-white">
                "Last 24 hours"
            </h2>
            <button class=BUTTON disabled=move || busy.get() on:click=generate>
                {move || if busy.get() { "Starting..." } else { "Generate now" }}
            </button>
        </div>
        {move || match status.get() {
            Some(Ok(message)) => {
                view! {
                    <p role="status" class="mt-2 text-xs text-fuji-gray">
                        {message}
                    </p>
                }
                    .into_any()
            }
            Some(Err(_)) => {
                view! { <ErrorNotice message="Could not start summary generation." /> }.into_any()
            }
            None => ().into_any(),
        }}
    }
}
