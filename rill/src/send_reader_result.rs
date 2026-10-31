//! Thread-safe environment-dependent computations with `Result` semantics.

use crate::SendReader;

/// A thread-safe [`crate::ReaderResult`] with short-circuiting composition.
///
/// Captured closures must be `Send + Sync`; they may still borrow for `'a`.
/// The computation itself is always `Send + Sync`, without imposing those
/// bounds on the environment, result, or error types.
#[must_use]
pub struct SendReaderResult<'a, R, T, E> {
    reader: SendReader<'a, R, Result<T, E>>,
}

impl<'a, R: 'a, T: 'a, E: 'a> SendReaderResult<'a, R, T, E> {
    /// Wraps a thread-safe fallible computation without running it.
    pub fn new(f: impl Fn(&R) -> Result<T, E> + Send + Sync + 'a) -> Self {
        Self {
            reader: SendReader::new(f),
        }
    }

    /// Runs the computation against the supplied environment.
    pub fn run(&self, environment: &R) -> Result<T, E> {
        self.reader.run(environment)
    }

    /// Lifts an infallible thread-safe reader.
    pub fn from_reader(reader: SendReader<'a, R, T>) -> Self {
        Self {
            reader: reader.map(Ok),
        }
    }

    /// Unwraps the underlying reader of a standard result.
    pub fn into_reader(self) -> SendReader<'a, R, Result<T, E>> {
        self.reader
    }

    /// Transforms successful values, leaving errors unchanged.
    pub fn map<U: 'a>(
        self,
        f: impl Fn(T) -> U + Send + Sync + 'a,
    ) -> SendReaderResult<'a, R, U, E> {
        SendReaderResult::new(move |environment| self.run(environment).map(&f))
    }

    /// Transforms errors, leaving successful values unchanged.
    pub fn map_err<F: 'a>(
        self,
        f: impl Fn(E) -> F + Send + Sync + 'a,
    ) -> SendReaderResult<'a, R, T, F> {
        SendReaderResult::new(move |environment| self.run(environment).map_err(&f))
    }

    /// Chains successful computations using the same environment.
    pub fn and_then<U: 'a>(
        self,
        f: impl Fn(T) -> SendReaderResult<'a, R, U, E> + Send + Sync + 'a,
    ) -> SendReaderResult<'a, R, U, E> {
        SendReaderResult::new(move |environment| {
            self.run(environment)
                .and_then(|value| f(value).run(environment))
        })
    }

    /// Recovers from an error using the same environment.
    pub fn or_else<F: 'a>(
        self,
        f: impl Fn(E) -> SendReaderResult<'a, R, T, F> + Send + Sync + 'a,
    ) -> SendReaderResult<'a, R, T, F> {
        SendReaderResult::new(move |environment| {
            self.run(environment)
                .or_else(|error| f(error).run(environment))
        })
    }

    /// Combines computations left-to-right, skipping the second on error.
    pub fn zip<U: 'a>(
        self,
        other: SendReaderResult<'a, R, U, E>,
    ) -> SendReaderResult<'a, R, (T, U), E> {
        SendReaderResult::new(move |environment| {
            let left = self.run(environment)?;
            let right = other.run(environment)?;
            Ok((left, right))
        })
    }

    /// Adapts an outer environment to the one this computation needs.
    pub fn local<S: 'a>(
        self,
        f: impl Fn(&S) -> R + Send + Sync + 'a,
    ) -> SendReaderResult<'a, S, T, E> {
        SendReaderResult {
            reader: self.reader.local(f),
        }
    }
}
