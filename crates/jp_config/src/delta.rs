//! Configuration delta calculation.

use indexmap::IndexMap;
use schematic::PartialConfig;

/// Calculate the delta between two partial configurations.
///
/// It takes `self`, and should check for any value in `next` that differs from
/// `self`.
/// If a value differs, it must be returned in the final [`PartialConfig`].
///
/// If no difference is found, the field should be set to `None` for optional
/// values, or `next` for non-optional values.
///
/// If all values are equal, then the returned `PartialConfig` should be the
/// same as [`PartialConfig::empty`].
pub trait PartialConfigDelta: PartialConfig {
    /// See [`PartialConfigDelta`].
    fn delta(&self, next: Self) -> Self;
}

/// Calculate the delta between two optional values.
pub fn delta_opt<T: PartialEq>(prev: Option<&T>, next: Option<T>) -> Option<T> {
    match (prev, next) {
        (Some(prev), Some(next)) if prev != &next => Some(next),
        (None, next) => next,
        _ => None,
    }
}

/// Calculate the delta between two optional values.
pub fn delta_opt_partial<T: PartialConfigDelta + PartialEq>(
    prev: Option<&T>,
    next: Option<T>,
) -> Option<T> {
    match (prev, next) {
        (Some(prev), Some(next)) if prev != &next => Some(prev.delta(next)),
        (None, next) => next,
        _ => None,
    }
}

/// Calculate the delta between two optional vectors that merge by appending.
///
/// The delta holds the elements `next` adds to `prev`, since that is what an
/// appending merge needs to reach `next` from `prev`.
///
/// Returns `None` when `next` adds nothing.
/// An element dropped from `prev` cannot be expressed by appending, so a
/// removal also yields `None` rather than a delta that fails to remove
/// anything.
///
/// Use [`delta_opt`] instead for a vector field that merges by replacement:
/// there the whole of `next` is the delta.
pub fn delta_opt_vec<T: PartialEq>(prev: Option<&Vec<T>>, next: Option<Vec<T>>) -> Option<Vec<T>> {
    let next = next?;
    let Some(prev) = prev else {
        return Some(next);
    };

    let added = delta_vec(prev, next);
    (!added.is_empty()).then_some(added)
}

/// Calculate the delta between two maps of partial configurations.
///
/// An entry only `next` has is kept whole.
/// An entry both maps have contributes its own delta, and is left out when that
/// delta is empty.
///
/// Dropping the empty ones is what keeps [`PartialConfig::is_empty`] meaningful
/// for the enclosing config: a map counts as empty only when it has no entries
/// at all, so an entry that carries no values still reads as a change.
pub fn delta_map<V>(prev: &IndexMap<String, V>, next: IndexMap<String, V>) -> IndexMap<String, V>
where
    V: PartialConfigDelta + PartialEq,
{
    next.into_iter()
        .filter_map(|(key, next)| {
            let Some(prev) = prev.get(&key) else {
                return Some((key, next));
            };

            if prev == &next {
                return None;
            }

            let delta = prev.delta(next);
            (!delta.is_empty()).then_some((key, delta))
        })
        .collect()
}

/// Calculate the delta between two vectors that merge by appending.
///
/// The delta holds the elements `next` adds to `prev`.
pub fn delta_vec<T: PartialEq>(prev: &[T], next: Vec<T>) -> Vec<T> {
    next.into_iter().filter(|v| !prev.contains(v)).collect()
}

#[cfg(test)]
#[path = "delta_tests.rs"]
mod tests;
