//! Applicative validation with ordered error accumulation.

use crate::NonEmpty;

/// A valid value or one or more errors.
///
/// Use [`zip`](Self::zip) or [`map2`](Self::map2) to combine independent
/// checks and retain all their errors. Unlike those operations,
/// [`and_then`](Self::and_then) cannot accumulate independent errors because
/// the next computation requires a successful value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[must_use]
pub enum Validation<T, E> {
    /// A successfully validated value.
    Valid(T),
    /// One or more validation errors, in encounter order.
    Invalid(NonEmpty<E>),
}

impl<T, E> Validation<T, E> {
    /// Creates an invalid value with a single error.
    pub fn invalid(error: E) -> Self {
        Self::Invalid(NonEmpty::new(error))
    }

    /// Transforms the successful value, leaving errors unchanged.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Validation<U, E> {
        match self {
            Self::Valid(value) => Validation::Valid(f(value)),
            Self::Invalid(errors) => Validation::Invalid(errors),
        }
    }

    /// Transforms every error, preserving their order and non-emptiness.
    pub fn map_err<F>(self, f: impl FnMut(E) -> F) -> Validation<T, F> {
        match self {
            Self::Valid(value) => Validation::Valid(value),
            Self::Invalid(errors) => Validation::Invalid(errors.map(f)),
        }
    }

    /// Combines independent values, accumulating errors left-to-right.
    pub fn zip<U>(self, other: Validation<U, E>) -> Validation<(T, U), E> {
        match (self, other) {
            (Self::Valid(left), Validation::Valid(right)) => Validation::Valid((left, right)),
            (Self::Invalid(left), Validation::Invalid(right)) => {
                Validation::Invalid(left.append(right))
            }
            (Self::Invalid(errors), _) | (_, Validation::Invalid(errors)) => {
                Validation::Invalid(errors)
            }
        }
    }

    /// Combines independent values with `f`, accumulating all errors.
    pub fn map2<U, V>(
        self,
        other: Validation<U, E>,
        f: impl FnOnce(T, U) -> V,
    ) -> Validation<V, E> {
        self.zip(other).map(|(left, right)| f(left, right))
    }

    /// Chains dependent validation, skipping `f` when this value is invalid.
    ///
    /// This operation short-circuits; use `zip` for independent checks.
    pub fn and_then<U>(self, f: impl FnOnce(T) -> Validation<U, E>) -> Validation<U, E> {
        match self {
            Self::Valid(value) => f(value),
            Self::Invalid(errors) => Validation::Invalid(errors),
        }
    }

    /// Converts to a standard result without losing any errors.
    pub fn into_result(self) -> Result<T, NonEmpty<E>> {
        match self {
            Self::Valid(value) => Ok(value),
            Self::Invalid(errors) => Err(errors),
        }
    }
}

impl<T, E> From<Result<T, E>> for Validation<T, E> {
    fn from(result: Result<T, E>) -> Self {
        match result {
            Ok(value) => Self::Valid(value),
            Err(error) => Self::invalid(error),
        }
    }
}

impl<T, E> From<Validation<T, E>> for Result<T, NonEmpty<E>> {
    fn from(validation: Validation<T, E>) -> Self {
        validation.into_result()
    }
}
