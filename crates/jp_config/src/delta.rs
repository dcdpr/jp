//! Configuration delta calculation.

use indexmap::IndexMap;
use schematic::PartialConfig;

use crate::types::vec::{MergeableVec, MergedVec, MergedVecStrategy};

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
    #[must_use]
    fn delta(&self, next: Self) -> Self;

    /// Diff `next` against `self`, reporting fields that merging cannot reach.
    ///
    /// Merging is per field, and a field that combines its two layers cannot
    /// reach a value that drops part of what `self` holds.
    /// Such a field carries `next`'s whole value in the returned partial, and
    /// its path joins `unsets`.
    /// The two together reproduce `next`: clearing the field leaves the merge
    /// nothing to combine with, so the value lands verbatim.
    ///
    /// `prefix` is the dotted path of `self` within the configuration, empty at
    /// the root.
    ///
    /// The default reports nothing, which is correct for any type whose fields
    /// all merge by replacement.
    #[must_use]
    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        let _ = (prefix, unsets);
        self.delta(next)
    }
}

/// Fields a delta never stores.
///
/// Each is read while the config file declaring it is loaded, and only its
/// effect outlives that: `extends` has already been merged in by the time a
/// partial exists, `inherit` has already stopped the merge chain, and `loader`
/// steered how its own entry was loaded ([RFD 038]).
/// Carrying any of them into a conversation would re-apply a decision that was
/// made once, so [`PartialConfigDelta::delta`] zeroes all three.
///
/// [RFD 038]: https://jp.computer/rfd/038
pub const LOAD_TIME_ONLY: &[&str] = &["extends", "inherit", "loader"];

/// Join a field name onto its parent's dotted path.
#[must_use]
pub fn path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

/// Calculate the delta between two strategy-carrying lists.
///
/// Appending reaches `next` exactly when `next` starts with `prev` and repeats
/// no element, and the delta is then the tail, carried as a plain list so the
/// fold appends it.
/// Every other difference — an element removed, reordered, inserted before the
/// last one, or repeated — carries the whole of `next` with `replace`.
///
/// Order is part of the answer, not a detail.
/// A delta that reproduced the set of elements while appending them in a
/// different order changes the meaning of any list whose order matters, and
/// says nothing at all about a list that only lost an element.
///
/// A [`MergeableVec`] can express `replace` on the wire, which is why this
/// needs no separate path report.
/// A plain `Vec` cannot; see [`delta_opt_vec_at`].
pub fn delta_mergeable_vec<T: Clone + PartialEq>(
    prev: &MergeableVec<T>,
    next: MergeableVec<T>,
) -> MergeableVec<T> {
    if next.starts_with(prev) && !repeats_an_element(&next) {
        return next.iter().skip(prev.len()).cloned().collect();
    }

    MergeableVec::Merged(MergedVec {
        value: next.into_vec(),
        strategy: Some(MergedVecStrategy::Replace),
        dedup: None,
        discard_when_merged: false,
    })
}

/// Whether the list holds the same element more than once.
///
/// An appending merge deduplicates unless a config opts out, keeping the first
/// occurrence, so a repeated element does not survive the fold: the list it
/// reaches is shorter than the one asked for.
/// For a list whose order decides precedence, a repeat that trails an element
/// of equal specificity is what settles the tie.
fn repeats_an_element<T: PartialEq>(items: &[T]) -> bool {
    items
        .iter()
        .enumerate()
        .any(|(index, item)| items[..index].contains(item))
}

/// Calculate the delta between two optional strategy-carrying lists.
///
/// Wraps [`delta_mergeable_vec`] for a field whose partial is
/// `Option<MergeableVec<T>>`: an absent list on either side is no change, and
/// an empty delta is reported as absent so it does not read as one.
pub fn delta_opt_mergeable_vec<T: Clone + PartialEq>(
    prev: Option<&MergeableVec<T>>,
    next: Option<MergeableVec<T>>,
) -> Option<MergeableVec<T>> {
    let next = next?;
    let Some(prev) = prev else {
        return Some(next);
    };

    let delta = delta_mergeable_vec(prev, next);
    (!delta.is_empty()).then_some(delta)
}

/// Delta for an optional nested partial, reporting the fields it cannot reach.
///
/// Mirrors [`delta_opt_partial`], descending with `path` as the nested value's
/// own dotted path.
pub fn delta_opt_partial_at<T: PartialConfigDelta + PartialEq>(
    path: &str,
    prev: Option<&T>,
    next: Option<T>,
    unsets: &mut Vec<String>,
) -> Option<T> {
    match (prev, next) {
        (Some(prev), Some(next)) if prev != &next => {
            Some(prev.delta_with_unsets(next, path, unsets))
        }
        // The whole block went away, which merging cannot say.
        (Some(_), None) => {
            unsets.push(path.to_owned());
            None
        }
        (None, next) => next,
        _ => None,
    }
}

/// Calculate the delta between two maps, reporting each entry's unsets.
///
/// Mirrors [`delta_map`], descending into each entry with the entry's own
/// dotted path so a field inside it reports where it lives.
pub fn delta_map_with_unsets<V>(
    prefix: &str,
    prev: &IndexMap<String, V>,
    next: IndexMap<String, V>,
    unsets: &mut Vec<String>,
) -> IndexMap<String, V>
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

            let mut entry = Vec::new();
            let delta = prev.delta_with_unsets(next, &path(prefix, &key), &mut entry);
            let cleared = !entry.is_empty();
            unsets.append(&mut entry);

            (cleared || !delta.is_empty()).then_some((key, delta))
        })
        .collect()
}

/// Calculate the delta between two optional values, reporting a cleared field.
///
/// A value that went away cannot be expressed by merging: schematic keeps the
/// previous value when the next layer has none.
/// The path joins `unsets` so the fold clears the field before merging, and
/// resolution then supplies whatever the field's absence means.
pub fn delta_opt_at<T: PartialEq>(
    path: &str,
    prev: Option<&T>,
    next: Option<T>,
    unsets: &mut Vec<String>,
) -> Option<T> {
    if prev.is_some() && next.is_none() {
        unsets.push(path.to_owned());
        return None;
    }

    delta_opt(prev, next)
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

#[cfg(test)]
#[path = "delta_law_tests.rs"]
mod law_tests;
