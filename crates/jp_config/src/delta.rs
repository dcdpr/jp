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

/// Join a field name onto its parent's dotted path.
#[must_use]
pub fn path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

/// Delta for an appending list, reporting when appending cannot reach `next`.
///
/// Returns the elements `next` adds while appending suffices.
/// Otherwise pushes `path` to `unsets` and returns the whole of `next`, which
/// is what the caller merges after clearing the field.
pub fn delta_opt_vec_at<T: PartialEq + Clone>(
    path: &str,
    prev: Option<&Vec<T>>,
    next: Option<Vec<T>>,
    unsets: &mut Vec<String>,
) -> Option<Vec<T>> {
    let next = next?;
    let Some(prev) = prev else {
        return Some(next);
    };

    // Appending reaches `next` exactly when `next` starts with `prev`; the
    // delta is then the tail. Anything else — a dropped element, a reorder, an
    // insertion in the middle — needs the field cleared first.
    if next.starts_with(prev) {
        let added = next[prev.len()..].to_vec();
        return (!added.is_empty()).then_some(added);
    }

    unsets.push(path.to_owned());
    Some(next)
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
