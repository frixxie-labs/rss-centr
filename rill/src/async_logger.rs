//! An explicit boundary for asynchronously emitting structured events.

use std::future::Future;

use crate::BoxFuture;

/// An asynchronous sink for application-defined structured events.
///
/// Each returned future completes according to the backend's delivery policy.
/// Backend failures are handled by the implementation, just as for
/// [`crate::Logger`]. Implementations can borrow themselves until delivery
/// completes. Closures returning `Send + 'static` futures implement this trait
/// automatically; use owned captures such as `Arc` inside those futures.
pub trait AsyncLogger<Event> {
    /// Creates a `Send` future that emits one event when polled.
    fn log(&self, event: Event) -> BoxFuture<'_, ()>;
}

impl<Event, F, Fut> AsyncLogger<Event> for F
where
    F: Fn(Event) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    fn log(&self, event: Event) -> BoxFuture<'_, ()> {
        Box::pin(self(event))
    }
}
