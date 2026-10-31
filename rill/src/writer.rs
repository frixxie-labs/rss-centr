//! Values paired with ordered, deferred events.

use crate::{AsyncLogger, Logger};

/// A value and zero or more events, accumulated in encounter order.
///
/// Building and composing writers never emits events. [`write_to`](Self::write_to)
/// explicitly consumes the writer and emits its events through a logger.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[must_use]
pub struct Writer<T, Event> {
    value: T,
    events: Vec<Event>,
}

impl<T, Event> Writer<T, Event> {
    /// Pairs a value with events in iteration order.
    pub fn new(value: T, events: impl IntoIterator<Item = Event>) -> Self {
        Self {
            value,
            events: events.into_iter().collect(),
        }
    }

    /// Creates a value without events.
    pub fn pure(value: T) -> Self {
        Self {
            value,
            events: Vec::new(),
        }
    }

    /// Borrows the computed value.
    pub fn value(&self) -> &T {
        &self.value
    }

    /// Borrows the collected events in encounter order.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Returns the value and events without emitting anything.
    pub fn into_parts(self) -> (T, Vec<Event>) {
        (self.value, self.events)
    }

    /// Transforms the value while preserving events.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Writer<U, Event> {
        Writer {
            value: f(self.value),
            events: self.events,
        }
    }

    /// Transforms events while preserving the value and event order.
    pub fn map_events<Other>(self, f: impl FnMut(Event) -> Other) -> Writer<T, Other> {
        Writer {
            value: self.value,
            events: self.events.into_iter().map(f).collect(),
        }
    }

    /// Chains computations, appending the next computation's events.
    pub fn and_then<U>(self, f: impl FnOnce(T) -> Writer<U, Event>) -> Writer<U, Event> {
        let next = f(self.value);
        let mut events = self.events;
        events.extend(next.events);
        Writer {
            value: next.value,
            events,
        }
    }

    /// Pairs values and concatenates their events left-to-right.
    pub fn zip<U>(self, other: Writer<U, Event>) -> Writer<(T, U), Event> {
        self.and_then(|left| other.map(|right| (left, right)))
    }

    /// Emits every event in order and returns the computed value.
    ///
    /// This is not transactional: if the logger panics, earlier events may
    /// already have been emitted and remaining events are dropped.
    pub fn write_to(self, logger: &(impl Logger<Event> + ?Sized)) -> T {
        for event in self.events {
            logger.log(event);
        }
        self.value
    }

    /// Emits events sequentially through an asynchronous logger.
    ///
    /// No events are emitted before polling. Each event is awaited before the
    /// next is submitted. Cancellation or panic may leave earlier events
    /// emitted; neither delivery nor rollback is transactional.
    pub async fn write_to_async(self, logger: &(impl AsyncLogger<Event> + Sync + ?Sized)) -> T
    where
        T: Send,
        Event: Send,
    {
        for event in self.events {
            logger.log(event).await;
        }
        self.value
    }
}

impl<Event> Writer<(), Event> {
    /// Records a single event without an associated value or emission.
    pub fn tell(event: Event) -> Self {
        Self::new((), [event])
    }
}
