//! Sorting items by the value they carry under one key.
//!
//! [`Vocabulary::sort_key`] hands out a [`SortKey`] for a key items can be
//! sorted by, and [`SortKey::compare`] compares two items' labels.
//! The comparison covers the key alone; a caller chains its own tie-break.
//!
//! [`Vocabulary::sort_key`]: crate::Vocabulary::sort_key

use std::cmp::Ordering;

use crate::{Labels, vocabulary::Facet};

/// Which end of a sort comes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Earliest date, or first declared value, first.
    Ascending,

    /// Latest date, or last declared value, first.
    Descending,
}

/// A key items can be sorted by, from [`Vocabulary::sort_key`].
///
/// [`Vocabulary::sort_key`]: crate::Vocabulary::sort_key
#[derive(Debug, Clone, Copy)]
pub struct SortKey<'a> {
    pub(crate) key: &'a str,
    pub(crate) facet: &'a Facet,
}

impl<'a> SortKey<'a> {
    /// The key, as the vocabulary spells it.
    #[must_use]
    pub fn key(&self) -> &'a str {
        self.key
    }

    /// Compare two items by the value each carries under this key.
    ///
    /// An item without a value to sort by comes after every item that has one,
    /// in both directions: one that carries no value of the key, a value that
    /// isn't a date on a `date` key, or a value an `ordered` key doesn't
    /// declare.
    /// Two such items, or two with the same value, compare equal.
    ///
    /// An item carrying more than one value, which only a hand-edited item can,
    /// sorts by the earliest in either direction.
    #[must_use]
    pub fn compare(&self, a: &Labels, b: &Labels, direction: Direction) -> Ordering {
        match (self.rank(a), self.rank(b)) {
            (Some(a), Some(b)) => match direction {
                Direction::Ascending => a.cmp(&b),
                Direction::Descending => b.cmp(&a),
            },
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }

    /// The earliest rank among the values `labels` carries under this key.
    fn rank<'l>(&self, labels: &'l Labels) -> Option<Rank<'l>> {
        labels
            .values(self.key)
            .filter_map(|value| self.facet.rank(value))
            .min()
    }
}

/// Where one value falls in a sort by its key.
///
/// Every value of one key ranks as the same variant, so the variants never
/// compare against each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Rank<'a> {
    /// A `YYYY-MM-DD` date, whose text order is its calendar order.
    Date(&'a str),

    /// The value's position in an `ordered` key's declaration.
    Position(usize),
}

#[cfg(test)]
#[path = "sort_tests.rs"]
mod tests;
