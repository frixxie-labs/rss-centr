//! Runtime-independent asynchronous computations sharing an environment.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::SendReader;

/// A boxed, `Send` future that may borrow data for `'a`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

type Computation<R, T> = dyn for<'r> Fn(&'r R) -> BoxFuture<'r, T> + Send + Sync;

/// A lazy, reusable, thread-safe asynchronous computation from `&R` to `T`.
///
/// Factories own their captures (`'static`), but each future can borrow its
/// environment across `.await`. The output cannot borrow from that environment.
/// Use `Arc` for shared captured state. Cloning shares the factory, not a running
/// future. Every `run` creates an independent future. No executor is provided.
///
/// ```
/// use rillfp::AsyncReader;
///
/// let reader = AsyncReader::new(|name: &String| Box::pin(async move {
///     format!("hello, {name}")
/// })).map(|greeting| greeting.to_uppercase());
/// # let _ = reader;
/// ```
///
/// Futures must also be `Send`; keeping an `Rc` across `.await` is rejected:
///
/// ```compile_fail
/// use std::rc::Rc;
/// use rillfp::AsyncReader;
///
/// let reader = AsyncReader::new(|_: &()| Box::pin(async {
///     let value = Rc::new(1);
///     std::future::ready(()).await;
///     *value
/// }));
/// ```
#[must_use]
pub struct AsyncReader<R, T> {
    run: Arc<Computation<R, T>>,
}

impl<R, T> Clone for AsyncReader<R, T> {
    fn clone(&self) -> Self {
        Self {
            run: Arc::clone(&self.run),
        }
    }
}

impl<R: Sync + 'static, T: Send + 'static> AsyncReader<R, T> {
    /// Wraps a future factory without running it.
    ///
    /// Use `Box::pin(async move { ... })` to allow borrowing the environment.
    pub fn new(f: impl for<'r> Fn(&'r R) -> BoxFuture<'r, T> + Send + Sync + 'static) -> Self {
        Self { run: Arc::new(f) }
    }

    /// Creates a `Send` future, borrowing the environment until completion.
    ///
    /// Even the factory is deferred until the returned future is polled. The
    /// future owns a factory reference and does not borrow this reader. Dropping
    /// it cancels that run; completed side effects are not rolled back.
    pub fn run<'r>(&self, environment: &'r R) -> BoxFuture<'r, T> {
        let run = Arc::clone(&self.run);
        Box::pin(async move { run(environment).await })
    }

    /// Creates a constant computation, cloning its value for each run.
    pub fn pure(value: T) -> Self
    where
        T: Clone + Sync,
    {
        Self::new(move |_| {
            let value = value.clone();
            Box::pin(async move { value })
        })
    }

    /// Lifts a thread-safe synchronous reader without spawning or offloading.
    ///
    /// Blocking work in the reader still blocks the executor polling it.
    pub fn from_sync(reader: SendReader<'static, R, T>) -> Self {
        Self::new(move |environment| {
            let value = reader.run(environment);
            Box::pin(async move { value })
        })
    }

    /// Transforms the successful output after awaiting it.
    pub fn map<U: Send + 'static>(
        self,
        f: impl Fn(T) -> U + Send + Sync + 'static,
    ) -> AsyncReader<R, U> {
        let f = Arc::new(f);
        AsyncReader::new(move |environment| {
            let future = self.run(environment);
            let f = Arc::clone(&f);
            Box::pin(async move { f(future.await) })
        })
    }

    /// Awaits dependent computations using the same environment.
    pub fn and_then<U: Send + 'static>(
        self,
        f: impl Fn(T) -> AsyncReader<R, U> + Send + Sync + 'static,
    ) -> AsyncReader<R, U> {
        let f = Arc::new(f);
        AsyncReader::new(move |environment| {
            let future = self.run(environment);
            let f = Arc::clone(&f);
            Box::pin(async move { f(future.await).run(environment).await })
        })
    }

    /// Awaits both computations sequentially, left-to-right, not concurrently.
    pub fn zip<U: Send + 'static>(self, other: AsyncReader<R, U>) -> AsyncReader<R, (T, U)> {
        AsyncReader::new(move |environment| {
            let left = self.run(environment);
            let other = other.clone();
            Box::pin(async move {
                let left = left.await;
                let right = other.run(environment).await;
                (left, right)
            })
        })
    }

    /// Builds an owned inner environment and keeps it alive across `.await`.
    pub fn local<S: Sync + 'static>(
        self,
        f: impl Fn(&S) -> R + Send + Sync + 'static,
    ) -> AsyncReader<S, T>
    where
        R: Send,
    {
        AsyncReader::new(move |environment| {
            let inner = f(environment);
            let reader = self.clone();
            Box::pin(async move { reader.run(&inner).await })
        })
    }
}

impl<R: Clone + Send + Sync + 'static> AsyncReader<R, R> {
    /// Reads a clone of the environment.
    pub fn ask() -> Self {
        Self::new(|environment: &R| {
            let value = environment.clone();
            Box::pin(async move { value })
        })
    }
}
