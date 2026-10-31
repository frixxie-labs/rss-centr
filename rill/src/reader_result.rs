//! Environment-dependent computations using standard `Result` semantics.

use crate::Reader;

/// A lazy, reusable computation from `&R` to `Result<T, E>`.
///
/// Like [`Result`], dependent composition and `zip` stop at the first error.
/// Use [`crate::Validation`] instead when independent errors should accumulate.
/// Closure lifetimes and allocation follow [`Reader`].
#[must_use]
pub struct ReaderResult<'a, R, T, E> {
    reader: Reader<'a, R, Result<T, E>>,
}

impl<'a, R: 'a, T: 'a, E: 'a> ReaderResult<'a, R, T, E> {
    /// Wraps a fallible computation without running it.
    pub fn new(f: impl Fn(&R) -> Result<T, E> + 'a) -> Self {
        Self {
            reader: Reader::new(f),
        }
    }

    /// Runs the computation against the supplied environment.
    pub fn run(&self, environment: &R) -> Result<T, E> {
        self.reader.run(environment)
    }

    /// Lifts an infallible reader into a successful fallible computation.
    pub fn from_reader(reader: Reader<'a, R, T>) -> Self {
        Self {
            reader: reader.map(Ok),
        }
    }

    /// Unwraps the underlying reader of a standard result.
    pub fn into_reader(self) -> Reader<'a, R, Result<T, E>> {
        self.reader
    }

    /// Transforms successful values, leaving errors unchanged.
    pub fn map<U: 'a>(self, f: impl Fn(T) -> U + 'a) -> ReaderResult<'a, R, U, E> {
        ReaderResult::new(move |environment| self.run(environment).map(&f))
    }

    /// Transforms errors, leaving successful values unchanged.
    pub fn map_err<F: 'a>(self, f: impl Fn(E) -> F + 'a) -> ReaderResult<'a, R, T, F> {
        ReaderResult::new(move |environment| self.run(environment).map_err(&f))
    }

    /// Chains successful computations using the same environment.
    pub fn and_then<U: 'a>(
        self,
        f: impl Fn(T) -> ReaderResult<'a, R, U, E> + 'a,
    ) -> ReaderResult<'a, R, U, E> {
        ReaderResult::new(move |environment| {
            self.run(environment)
                .and_then(|value| f(value).run(environment))
        })
    }

    /// Recovers from an error using the same environment.
    pub fn or_else<F: 'a>(
        self,
        f: impl Fn(E) -> ReaderResult<'a, R, T, F> + 'a,
    ) -> ReaderResult<'a, R, T, F> {
        ReaderResult::new(move |environment| {
            self.run(environment)
                .or_else(|error| f(error).run(environment))
        })
    }

    /// Combines computations left-to-right, skipping the second on error.
    pub fn zip<U: 'a>(self, other: ReaderResult<'a, R, U, E>) -> ReaderResult<'a, R, (T, U), E> {
        ReaderResult::new(move |environment| {
            let left = self.run(environment)?;
            let right = other.run(environment)?;
            Ok((left, right))
        })
    }

    /// Adapts an outer environment to the one this computation needs.
    pub fn local<S: 'a>(self, f: impl Fn(&S) -> R + 'a) -> ReaderResult<'a, S, T, E> {
        ReaderResult {
            reader: self.reader.local(f),
        }
    }
}
