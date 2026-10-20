#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::net::{IpAddr, SocketAddr};

    use leptos::config::get_configuration;
    use rss_centr_frontend::server::{Proxy, router};
    use tracing_subscriber::EnvFilter;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .json()
        .init();
    let options = get_configuration(None)?.leptos_options;
    // `IP`/`PORT` keep the deployment contract; Leptos' own settings are the fallback.
    let ip = match std::env::var("IP") {
        Ok(ip) => ip.parse::<IpAddr>()?,
        Err(_) => options.site_addr.ip(),
    };
    let port = match std::env::var("PORT") {
        Ok(port) => port.parse::<u16>()?,
        Err(_) => options.site_addr.port(),
    };
    let addr = SocketAddr::new(ip, port);
    let app = router(options, Proxy::from_env());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "Listening");
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

// The browser build only uses the library; this keeps `cargo check --target wasm32-…` working.
#[cfg(target_arch = "wasm32")]
fn main() {}
