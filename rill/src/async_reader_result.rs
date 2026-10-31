//! Thread-safe asynchronous computations using standard `Result` semantics.

use std::sync::Arc;

use crate::{AsyncReader, BoxFuture, SendReaderResult};

/// An asynchronous reader with short-circuiting `Result` composition.
///
/// Factories must own their captures and be `Send + Sync`; returned futures are
/// `Send` and may borrow the environment across `.await`. Both successful and
/// error outputs must be `Send + 'static`. See [`AsyncReader`] for execution
/// and cancellation semantics. `zip` is sequential, not concurrent.
#[must_use]
pub struct AsyncReaderResult<R, T, E> {
    reader: AsyncReader<R, Result<T, E>>,
}

impl<R, T, E> Clone for AsyncReaderResult<R, T, E> {
    fn clone(&self) -> Self {
        Self {
            reader: self.reader.clone(),
        }
    }
}

impl<R: Sync + 'static, T: Send + 'static, E: Send + 'static> AsyncReaderResult<R, T, E> {
    /// Wraps a future factory without running it.
    pub fn new(
        f: impl for<'r> Fn(&'r R) -> BoxFuture<'r, Result<T, E>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            reader: AsyncReader::new(f),
        }
    }

    /// Creates a lazy `Send` future that borrows the environment.
    pub fn run<'r>(&self, environment: &'r R) -> BoxFuture<'r, Result<T, E>> {
        self.reader.run(environment)
    }

    /// Lifts an infallible asynchronous reader.
    pub fn from_reader(reader: AsyncReader<R, T>) -> Self {
        Self {
            reader: reader.map(Ok),
        }
    }

    /// Lifts a synchronous fallible reader without spawning or offloading.
    pub fn from_sync(reader: SendReaderResult<'static, R, T, E>) -> Self {
        Self {
            reader: AsyncReader::from_sync(reader.into_reader()),
        }
    }

    /// Unwraps the underlying reader of a standard result.
    pub fn into_reader(self) -> AsyncReader<R, Result<T, E>> {
        self.reader
    }

    /// Transforms successful values, leaving errors unchanged.
    pub fn map<U: Send + 'static>(
        self,
        f: impl Fn(T) -> U + Send + Sync + 'static,
    ) -> AsyncReaderResult<R, U, E> {
        AsyncReaderResult {
            reader: self.reader.map(move |result| result.map(&f)),
        }
    }

    /// Transforms errors, leaving successful values unchanged.
    pub fn map_err<F: Send + 'static>(
        self,
        f: impl Fn(E) -> F + Send + Sync + 'static,
    ) -> AsyncReaderResult<R, T, F> {
        AsyncReaderResult {
            reader: self.reader.map(move |result| result.map_err(&f)),
        }
    }

    /// Awaits dependent computations, skipping the next computation on error.
    pub fn and_then<U: Send + 'static>(
        self,
        f: impl Fn(T) -> AsyncReaderResult<R, U, E> + Send + Sync + 'static,
    ) -> AsyncReaderResult<R, U, E> {
        let f = Arc::new(f);
        AsyncReaderResult::new(move |environment| {
            let future = self.run(environment);
            let f = Arc::clone(&f);
            Box::pin(async move {
                let value = future.await?;
                f(value).run(environment).await
            })
        })
    }

    /// Recovers from an error using the same environment.
    pub fn or_else<F: Send + 'static>(
        self,
        f: impl Fn(E) -> AsyncReaderResult<R, T, F> + Send + Sync + 'static,
    ) -> AsyncReaderResult<R, T, F> {
        let f = Arc::new(f);
        AsyncReaderResult::new(move |environment| {
            let future = self.run(environment);
            let f = Arc::clone(&f);
            Box::pin(async move {
                match future.await {
                    Ok(value) => Ok(value),
                    Err(error) => f(error).run(environment).await,
                }
            })
        })
    }

    /// Awaits computations left-to-right, never starting the second on error.
    pub fn zip<U: Send + 'static>(
        self,
        other: AsyncReaderResult<R, U, E>,
    ) -> AsyncReaderResult<R, (T, U), E> {
        AsyncReaderResult::new(move |environment| {
            let left = self.run(environment);
            let other = other.clone();
            Box::pin(async move {
                let left = left.await?;
                let right = other.run(environment).await?;
                Ok((left, right))
            })
        })
    }

    /// Builds an owned inner environment and keeps it alive across `.await`.
    pub fn local<S: Sync + 'static>(
        self,
        f: impl Fn(&S) -> R + Send + Sync + 'static,
    ) -> AsyncReaderResult<S, T, E>
    where
        R: Send,
    {
        AsyncReaderResult {
            reader: self.reader.local(f),
        }
    }
}
