//! An explicit boundary for emitting application-defined structured events.

/// A sink for structured events, typically a domain-specific struct or enum.
///
/// Implement this trait to adapt an existing logging backend. Closures
/// implementing `Fn(Event)` work as loggers without an adapter. Logging is
/// synchronous; implementations own delivery, filtering, and failure policy.
/// No thread-safety bounds or global logger are imposed.
///
/// ```
/// use rillfp::Logger;
///
/// #[derive(Debug)]
/// struct UserCreated { id: u64 }
/// let logger = |event: UserCreated| println!("created user {}", event.id);
/// logger.log(UserCreated { id: 42 });
/// ```
pub trait Logger<Event> {
    /// Emits one event. Backend failures are handled by the implementation.
    fn log(&self, event: Event);
}

impl<Event, F: Fn(Event)> Logger<Event> for F {
    fn log(&self, event: Event) {
        self(event);
    }
}
