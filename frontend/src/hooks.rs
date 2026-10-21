use leptos::prelude::*;

/// Starts at the server's render time so hydration matches, then ticks every minute.
pub fn use_clock(initial: i64) -> Signal<i64> {
    let now = RwSignal::new(initial);
    #[cfg(target_arch = "wasm32")]
    if let Ok(handle) = set_interval_with_handle(
        move || now.set(chrono::Utc::now().timestamp_millis()),
        std::time::Duration::from_secs(60),
    ) {
        on_cleanup(move || handle.clear());
    }
    now.into()
}

/// The error for data the server loaded (`initial`) that `reload` can fetch again.
/// It is hidden while a reload is pending and replaced by the latest result.
pub fn reload_error<I, T>(
    reload: Action<I, Result<T, String>>,
    initial: Option<String>,
) -> Signal<Option<String>>
where
    I: 'static,
    T: 'static,
{
    let pending = reload.pending();
    let value = reload.value();
    Signal::derive(move || {
        if pending.get() {
            return None;
        }
        value.with(|result| match result {
            Some(result) => result.as_ref().err().cloned(),
            None => initial.clone(),
        })
    })
}

pub fn sleep_ms(_ms: u32) -> impl std::future::Future<Output = ()> {
    #[cfg(target_arch = "wasm32")]
    return gloo_timers::future::TimeoutFuture::new(_ms);
    #[cfg(not(target_arch = "wasm32"))]
    std::future::ready(())
}
