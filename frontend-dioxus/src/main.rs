fn main() {
    #[cfg(feature = "server")]
    {
        use tracing_subscriber::EnvFilter;
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .json()
            .init();
        dioxus::serve(|| async { Ok(rss_centr_frontend::server::router()) });
    }
    #[cfg(all(feature = "web", not(feature = "server")))]
    dioxus::launch(rss_centr_frontend::app::App);
}
