use dioxus::prelude::*;

use crate::pages::*;

#[derive(Clone, PartialEq, Routable, Debug)]
pub enum Route {
    #[layout(Layout)]
    #[route("/")]
    Summary {},
    #[route("/timeline")]
    Timeline {},
    #[route("/news?:..query")]
    News { query: String },
    #[route("/sources")]
    Sources {},
    #[route("/topics")]
    Topics {},
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[component]
pub fn App() -> Element {
    rsx! {
        document::Link { rel: "icon", href: asset!("/assets/favicon.svg"), r#type: "image/svg+xml" }
        document::Stylesheet { href: asset!("/assets/styles.css") }
        Router::<Route> {}
    }
}

#[component]
fn Layout() -> Element {
    let current: Route = use_route();
    let nav = [
        (Route::Summary {}, "AI Summary"),
        (Route::Timeline {}, "Timeline"),
        (
            Route::News {
                query: String::new(),
            },
            "News",
        ),
        (Route::Sources {}, "Sources"),
        (Route::Topics {}, "Topics"),
    ];
    rsx! {
        div { class: "min-h-screen flex flex-col",
            header { class: "flex flex-col gap-2 border-b border-sumi-ink3 px-3 py-3 sm:flex-row sm:items-center sm:justify-between sm:px-4",
                h1 { class: "whitespace-nowrap text-lg font-semibold tracking-tight text-fuji-white", "RSS Centr" }
                nav { class: "flex w-full flex-wrap items-center justify-between gap-1 sm:w-auto sm:justify-start sm:gap-2",
                    for (route, label) in nav {
                        Link {
                            class: if std::mem::discriminant(&current) == std::mem::discriminant(&route) { "rounded-md bg-sumi-ink3 px-2 py-1 text-sm text-fuji-white" } else { "rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white" },
                            to: route,
                            "{label}"
                        }
                    }
                }
            }
            main { class: "mx-auto w-full min-w-0 max-w-3xl flex-1",
                SuspenseBoundary { fallback: |_| rsx! { p { class: "p-4 text-fuji-gray", role: "status", "Loading…" } },
                    Outlet::<Route> {}
                }
            }
        }
    }
}

#[component]
fn NotFound(segments: Vec<String>) -> Element {
    rsx! { document::Title { "RSS Centr - Not found" } p { class: "p-4", "Page not found." } }
}
