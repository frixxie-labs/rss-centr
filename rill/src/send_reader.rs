//! Thread-safe, reusable computations sharing a borrowed environment.

/// A [`crate::Reader`] variant whose captured closure is `Send + Sync`.
///
/// The reader can be moved or shared between threads, even when it borrows
/// captures (use scoped threads in that case). `R` and `T` need not themselves
/// be `Send` or `Sync` unless the operation requires it. For non-thread-safe
/// captures such as `Rc` or `RefCell`, keep using [`crate::Reader`].
///
/// ```compile_fail
/// use std::rc::Rc;
/// use rillfp::SendReader;
///
/// let value = Rc::new(1);
/// let reader = SendReader::new(move |_: &()| *value);
/// ```
#[must_use]
pub struct SendReader<'a, R, T> {
    run: Box<dyn Fn(&R) -> T + Send + Sync + 'a>,
}

impl<'a, R: 'a, T: 'a> SendReader<'a, R, T> {
    /// Wraps a thread-safe computation without running it.
    pub fn new(f: impl Fn(&R) -> T + Send + Sync + 'a) -> Self {
        Self { run: Box::new(f) }
    }

    /// Runs the computation against the supplied environment.
    pub fn run(&self, environment: &R) -> T {
        (self.run)(environment)
    }

    /// Creates a constant computation, cloning its value for each run.
    pub fn pure(value: T) -> Self
    where
        T: Clone + Send + Sync,
    {
        Self::new(move |_| value.clone())
    }

    /// Transforms the computed value lazily.
    pub fn map<U: 'a>(self, f: impl Fn(T) -> U + Send + Sync + 'a) -> SendReader<'a, R, U> {
        SendReader::new(move |environment| f(self.run(environment)))
    }

    /// Chains computations, passing the same environment to both.
    pub fn and_then<U: 'a>(
        self,
        f: impl Fn(T) -> SendReader<'a, R, U> + Send + Sync + 'a,
    ) -> SendReader<'a, R, U> {
        SendReader::new(move |environment| f(self.run(environment)).run(environment))
    }

    /// Runs both computations left-to-right against the same environment.
    pub fn zip<U: 'a>(self, other: SendReader<'a, R, U>) -> SendReader<'a, R, (T, U)> {
        SendReader::new(move |environment| (self.run(environment), other.run(environment)))
    }

    /// Adapts an outer environment to the one this computation needs.
    pub fn local<S: 'a>(self, f: impl Fn(&S) -> R + Send + Sync + 'a) -> SendReader<'a, S, T> {
        SendReader::new(move |environment| self.run(&f(environment)))
    }
}

impl<'a, R: Clone + 'a> SendReader<'a, R, R> {
    /// Reads a clone of the environment.
    pub fn ask() -> Self {
        Self::new(Clone::clone)
    }
}
