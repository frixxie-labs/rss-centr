use leptos::prelude::*;
use leptos_meta::{MetaTags, Stylesheet, Title, provide_meta_context};
use leptos_router::{
    SsrMode,
    components::{Route, Router, Routes},
    hooks::use_location,
    path,
};

use crate::pages::*;

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
                <AutoReload options=options.clone() />
                <HydrationScripts options islands=true />
                <MetaTags />
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    view! {
        <Stylesheet id="leptos" href="/pkg/rss-centr-frontend.css" />
        <Title text="RSS Centr" />
        <Router>
            <Layout>
                <Routes fallback=NotFound>
                    <Route path=path!("/") view=Summary ssr=SsrMode::Async />
                    <Route path=path!("/timeline") view=Timeline ssr=SsrMode::Async />
                    <Route path=path!("/news") view=News ssr=SsrMode::Async />
                    <Route path=path!("/sources") view=Sources ssr=SsrMode::Async />
                    <Route path=path!("/topics") view=Topics ssr=SsrMode::Async />
                </Routes>
            </Layout>
        </Router>
    }
}

#[component]
fn Layout(children: Children) -> impl IntoView {
    let current = use_location().pathname.get_untracked();
    let nav = [
        ("/", "AI Summary"),
        ("/timeline", "Timeline"),
        ("/news", "News"),
        ("/sources", "Sources"),
        ("/topics", "Topics"),
    ];
    view! {
        <div class="min-h-screen flex flex-col">
            <header class="flex flex-col gap-2 border-b border-sumi-ink3 px-3 py-3 sm:flex-row sm:items-center sm:justify-between sm:px-4">
                <h1 class="whitespace-nowrap text-lg font-semibold tracking-tight text-fuji-white">
                    "RSS Centr"
                </h1>
                <nav class="flex w-full flex-wrap items-center justify-between gap-1 sm:w-auto sm:justify-start sm:gap-2">
                    {nav
                        .into_iter()
                        .map(|(href, label)| {
                            let class = if current == href {
                                "rounded-md bg-sumi-ink3 px-2 py-1 text-sm text-fuji-white"
                            } else {
                                "rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
                            };
                            view! { <a class=class href=href>{label}</a> }
                        })
                        .collect_view()}
                </nav>
            </header>
            <main class="mx-auto w-full min-w-0 max-w-3xl flex-1">{children()}</main>
        </div>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    if let Some(response) = use_context::<leptos_axum::ResponseOptions>() {
        response.set_status(axum::http::StatusCode::NOT_FOUND);
    }
    view! {
        <Title text="RSS Centr - Not found" />
        <p class="p-4">"Page not found."</p>
    }
}
