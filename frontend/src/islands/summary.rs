use leptos::prelude::*;

use crate::{api::Api, components::*};

/// Heading row with the "Generate now" button; the summary text itself stays server-only.
#[island]
pub fn SummaryControls() -> impl IntoView {
    let generate = Action::new_local(|_: &()| async { Api::default().generate_summary().await });
    let busy = generate.pending();
    let result = generate.value();
    view! {
        <div class="flex items-center justify-between gap-2">
            <h2 id="daily-summary-title" class="text-sm font-semibold text-fuji-white">
                "Last 24 hours"
            </h2>
            <button
                class=BUTTON
                disabled=move || busy.get()
                on:click=move |_| {
                    generate.dispatch(());
                }
            >
                {move || if busy.get() { "Starting…" } else { "Generate now" }}
            </button>
        </div>
        {move || {
            if busy.get() {
                return ().into_any();
            }
            match result.get() {
                Some(Ok(started)) => {
                    let message = if started {
                        "Generating a new summary. Reload the page in a minute to see it."
                    } else {
                        "A summary is already being generated."
                    };
                    view! {
                        <p role="status" class="mt-2 text-xs text-fuji-gray">
                            {message}
                        </p>
                    }
                        .into_any()
                }
                Some(Err(_)) => {
                    view! { <ErrorNotice message="Could not start summary generation." /> }
                        .into_any()
                }
                None => ().into_any(),
            }
        }}
    }
}
