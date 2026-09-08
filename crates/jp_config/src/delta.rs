//! Configuration delta calculation.

use schematic::PartialConfig;

use crate::types::{
    map::{MergeableMap, MergedMap, MergedMapStrategy},
    vec::{MergeableVec, MergedVec, MergedVecStrategy},
};

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
/// Appending reaches `next` exactly when `next` starts with `prev`, and the
/// delta is then the tail, carried as a plain list so the fold appends it.
/// Every other difference — an element removed, reordered, or inserted before
/// the last one — carries the whole of `next` with `replace`.
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
    if next.starts_with(prev) {
        return next.iter().skip(prev.len()).cloned().collect();
    }

    MergeableVec::Merged(MergedVec {
        value: next.into_vec(),
        strategy: Some(MergedVecStrategy::Replace),
        dedup: None,
        discard_when_merged: false,
    })
}

/// Calculate the delta between two strategy-carrying maps.
///
/// An entry only `next` has is carried whole, an entry both maps have carries
/// its own delta, and an entry whose delta is empty is left out: a missing
/// entry already means "unchanged", so carrying an empty one reads as a change
/// that isn't there.
///
/// A key `prev` has and `next` does not was dropped, which entries cannot
/// spell.
/// The whole map is then carried with `replace`, since a deep merge would
/// resurrect the dropped key.
pub fn delta_mergeable_map<T>(prev: &MergeableMap<T>, next: MergeableMap<T>) -> MergeableMap<T>
where
    T: PartialConfigDelta + PartialEq,
{
    if prev.keys().any(|key| !next.contains_key(key)) {
        // Stated rather than inherited from `next`'s shape: a plain map
        // deep-merges on the fold and brings the dropped key back.
        return MergeableMap::Merged(MergedMap {
            value: next.into_map(),
            strategy: Some(MergedMapStrategy::Replace),
            discard_when_merged: false,
        });
    }

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

/// Calculate the delta between two strategy-carrying maps of plain values.
///
/// Mirrors [`delta_mergeable_map`] for a map whose values carry no partial of
/// their own, so an entry is compared and carried whole rather than diffed.
pub fn delta_mergeable_value_map<T: Clone + PartialEq>(
    prev: &MergeableMap<T>,
    next: MergeableMap<T>,
) -> MergeableMap<T> {
    if prev.keys().any(|key| !next.contains_key(key)) {
        // Stated rather than inherited from `next`'s shape: a plain map
        // deep-merges on the fold and brings the dropped key back.
        return MergeableMap::Merged(MergedMap {
            value: next.into_map(),
            strategy: Some(MergedMapStrategy::Replace),
            discard_when_merged: false,
        });
    }

    next.into_iter()
        .filter(|(key, next)| !prev.get(key).is_some_and(|prev| prev == next))
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
