//! Sorting items by a single label key.
//!
//! [`Vocabulary::sort_key`] returns a [`SortKey`] for a sortable key, and
//! [`SortKey::compare`] compares two items' label sets by that key.
//! Comparison considers only that key; callers chain their own tie-breaker.
//!
//! [`Vocabulary::sort_key`]: crate::Vocabulary::sort_key

use std::cmp::Ordering;

use crate::{Labels, vocabulary::Facet};

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Earliest date or first declared value first.
    Ascending,

    /// Latest date or last declared value first.
    Descending,
}

/// A sortable label key obtained from [`Vocabulary::sort_key`].
///
/// [`Vocabulary::sort_key`]: crate::Vocabulary::sort_key
#[derive(Debug, Clone, Copy)]
pub struct SortKey<'a> {
    pub(crate) key: &'a str,
    pub(crate) facet: &'a Facet,
}

impl<'a> SortKey<'a> {
    /// Canonical key name in the vocabulary.
    #[must_use]
    pub fn key(&self) -> &'a str {
        self.key
    }

    /// Compare two label sets by their value under this key.
    ///
    /// In both directions, items with an active value sort first, followed by
    /// items with a retired value (on an `ordered` key), followed by items
    /// without a sortable value (missing the key, carrying a non-date value on
    /// a `date` key, or carrying an undeclared value on an `ordered` key).
    /// `direction` orders active values among themselves and retired values
    /// among themselves.
    /// Two items without a sortable value, or two items with the same value,
    /// compare equal.
    ///
    /// If a hand-edited item carries multiple values for the key, its earliest
    /// value (active before retired) is used in both directions.
    #[must_use]
    pub fn compare(&self, a: &Labels, b: &Labels, direction: Direction) -> Ordering {
        match (self.rank(a), self.rank(b)) {
            (Some(a), Some(b)) => a.retired.cmp(&b.retired).then(match direction {
                Direction::Ascending => a.rank.cmp(&b.rank),
                Direction::Descending => b.rank.cmp(&a.rank),
            }),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }

    /// Smallest rank among the values `labels` carries under this key, active
    /// values before retired ones.
    fn rank<'l>(&self, labels: &'l Labels) -> Option<Ranked<'l>> {
        labels
            .values(self.key)
            .filter_map(|value| self.facet.rank(value))
            .min()
    }
}

/// Sort position of a single label value: whether it is retired, then its
/// [`Rank`].
///
/// Field order matters: the derived `Ord` puts every active value before every
/// retired one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Ranked<'a> {
    pub(crate) retired: bool,
    pub(crate) rank: Rank<'a>,
}

/// Sort rank of a single label value.
///
/// All values for a given key produce the same variant, so [`Date`] and
/// [`Position`] are never compared against each other.
///
/// [`Date`]: Self::Date
/// [`Position`]: Self::Position
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Rank<'a> {
    /// A `YYYY-MM-DD` date string, ordered lexicographically.
    Date(&'a str),

    /// Zero-based index in an `ordered` key's declared value list.
    Position(usize),
}

#[cfg(test)]
#[path = "sort_tests.rs"]
mod tests;
