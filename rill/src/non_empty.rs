//! Collections with a guaranteed first element.

use std::iter::{Chain, Once, once};
use std::slice;
use std::vec;

/// An ordered collection that always contains at least one element.
///
/// The private head and tail prevent safe operations from making it empty.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[must_use]
pub struct NonEmpty<T> {
    head: T,
    tail: Vec<T>,
}

impl<T> NonEmpty<T> {
    /// Creates a singleton collection.
    pub fn new(head: T) -> Self {
        Self {
            head,
            tail: Vec::new(),
        }
    }

    /// Creates a collection from a first element and the remaining elements.
    pub fn from_head_tail(head: T, tail: impl IntoIterator<Item = T>) -> Self {
        Self {
            head,
            tail: tail.into_iter().collect(),
        }
    }

    /// Converts a vector, returning `None` if it is empty.
    pub fn from_vec(values: Vec<T>) -> Option<Self> {
        let mut values = values.into_iter();
        values.next().map(|head| Self::from_head_tail(head, values))
    }

    /// Returns the first element.
    pub fn first(&self) -> &T {
        &self.head
    }

    /// Returns the last element.
    pub fn last(&self) -> &T {
        self.tail.last().unwrap_or(&self.head)
    }

    /// Returns the number of elements, which is always positive.
    pub fn len(&self) -> usize {
        1 + self.tail.len()
    }

    /// Always returns `false`.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Appends an element.
    pub fn push(&mut self, value: T) {
        self.tail.push(value);
    }

    /// Iterates over all elements in order.
    pub fn iter(&self) -> Chain<Once<&T>, slice::Iter<'_, T>> {
        once(&self.head).chain(self.tail.iter())
    }

    /// Iterates over all elements mutably, without allowing removal.
    pub fn iter_mut(&mut self) -> Chain<Once<&mut T>, slice::IterMut<'_, T>> {
        once(&mut self.head).chain(self.tail.iter_mut())
    }

    /// Transforms each element, preserving order and non-emptiness.
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> NonEmpty<U> {
        NonEmpty::from_head_tail(f(self.head), self.tail.into_iter().map(f))
    }

    /// Concatenates two collections in left-to-right order.
    pub fn append(mut self, other: Self) -> Self {
        self.extend(other);
        self
    }

    /// Converts into a standard vector in iteration order.
    pub fn into_vec(self) -> Vec<T> {
        self.into_iter().collect()
    }
}

impl<T> Extend<T> for NonEmpty<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.tail.extend(iter);
    }
}

impl<T> IntoIterator for NonEmpty<T> {
    type Item = T;
    type IntoIter = Chain<Once<T>, vec::IntoIter<T>>;

    fn into_iter(self) -> Self::IntoIter {
        once(self.head).chain(self.tail)
    }
}

impl<'a, T> IntoIterator for &'a NonEmpty<T> {
    type Item = &'a T;
    type IntoIter = Chain<Once<&'a T>, slice::Iter<'a, T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut NonEmpty<T> {
    type Item = &'a mut T;
    type IntoIter = Chain<Once<&'a mut T>, slice::IterMut<'a, T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
