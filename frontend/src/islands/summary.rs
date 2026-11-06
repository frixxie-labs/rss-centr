use leptos::prelude::*;

use crate::{api::Api, components::*, hooks::sleep_ms};

const POLL_INTERVAL_MS: u32 = 15_000;
// Generation can take most of an hour on a cold cache; stop polling after 30 minutes.
const POLL_ATTEMPTS: u32 = 120;

/// Heading row with the "Generate now" button; the summary text itself stays
/// server-only. After starting a generation it polls for a summary newer than
/// `latest_id` and reloads the page to show it.
#[island]
pub fn SummaryControls(latest_id: Option<i64>) -> impl IntoView {
    let generate = Action::new_local(|_: &()| async { Api::default().generate_summary().await });
    let busy = generate.pending();
    let result = generate.value();
    Effect::new(move |_| {
        if let Some(Ok(true)) = result.get() {
            leptos::task::spawn_local(wait_for_new_summary(latest_id));
        }
    });
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
                        "Generating a new summary. This page reloads when it is ready."
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

async fn wait_for_new_summary(latest_id: Option<i64>) {
    for _ in 0..POLL_ATTEMPTS {
        sleep_ms(POLL_INTERVAL_MS).await;
        if let Ok(Some(summary)) = Api::default().summary().await
            && Some(summary.id) != latest_id
        {
            reload_page();
            return;
        }
    }
}

fn reload_page() {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}
