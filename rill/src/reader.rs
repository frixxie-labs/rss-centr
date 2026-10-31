//! Lazy, reusable computations sharing a borrowed environment.

/// A computation from an environment `&R` to a value `T`.
///
/// Computations run only when [`run`](Self::run) is called. Closures may borrow
/// captures for `'a`; they need not be `Send`, `Sync`, or `'static`. Each
/// constructor/combinator boxes a closure using dynamic dispatch. The `Fn`
/// bound makes repeated runs possible, but does not guarantee purity.
#[must_use]
pub struct Reader<'a, R, T> {
    run: Box<dyn Fn(&R) -> T + 'a>,
}

impl<'a, R: 'a, T: 'a> Reader<'a, R, T> {
    /// Wraps a computation without running it.
    pub fn new(f: impl Fn(&R) -> T + 'a) -> Self {
        Self { run: Box::new(f) }
    }

    /// Runs the computation against the supplied environment.
    pub fn run(&self, environment: &R) -> T {
        (self.run)(environment)
    }

    /// Creates a constant computation, cloning its value for each run.
    pub fn pure(value: T) -> Self
    where
        T: Clone,
    {
        Self::new(move |_| value.clone())
    }

    /// Transforms the computed value lazily.
    pub fn map<U: 'a>(self, f: impl Fn(T) -> U + 'a) -> Reader<'a, R, U> {
        Reader::new(move |environment| f(self.run(environment)))
    }

    /// Chains computations, passing the same environment to both.
    pub fn and_then<U: 'a>(self, f: impl Fn(T) -> Reader<'a, R, U> + 'a) -> Reader<'a, R, U> {
        Reader::new(move |environment| f(self.run(environment)).run(environment))
    }

    /// Runs both computations left-to-right against the same environment.
    pub fn zip<U: 'a>(self, other: Reader<'a, R, U>) -> Reader<'a, R, (T, U)> {
        Reader::new(move |environment| (self.run(environment), other.run(environment)))
    }

    /// Adapts an outer environment to the one this computation needs.
    pub fn local<S: 'a>(self, f: impl Fn(&S) -> R + 'a) -> Reader<'a, S, T> {
        Reader::new(move |environment| self.run(&f(environment)))
    }
}

impl<'a, R: Clone + 'a> Reader<'a, R, R> {
    /// Reads a clone of the environment.
    ///
    /// For a projection that avoids cloning the entire environment, use `new`.
    pub fn ask() -> Self {
        Self::new(Clone::clone)
    }
}
