//! Vec merge strategies.

#![expect(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]

use schematic::{MergeResult, Schematic};
use serde::{Serialize, de::DeserializeOwned};

use crate::types::vec::{MergeableVec, MergedVec, MergedVecStrategy};

/// Merge two `MergeableVec` values.
pub fn vec_with_strategy<T>(
    prev: MergeableVec<T>,
    next: MergeableVec<T>,
    _context: &(),
) -> MergeResult<MergeableVec<T>>
where
    T: Clone + PartialEq + Serialize + DeserializeOwned + Schematic,
{
    // Resolve the explicit dedup opinion: next's choice wins, then inherit from
    // prev. `None` means neither side expressed one.
    //
    // A discarded prev still contributes dedup when next has no opinion (None /
    // "inherit"), but NOT when next explicitly sets it.
    let dedup = dedup_flag(&next).or_else(|| dedup_flag(&prev));

    // If prev is default, replace regardless of strategy. Nothing is combined,
    // so only an explicit opt-in deduplicates — see the note on `dedup_active`
    // below.
    if prev.discard_when_merged() {
        let mut next = next;
        if dedup == Some(true) {
            dedup_in_place(&mut next);
        }

        return Ok(Some(with_dedup_flag(next, dedup)));
    }

    let mut prev_value = match prev {
        MergeableVec::Vec(v) => v,
        MergeableVec::Merged(v) => v.value,
    };

    let next_is_merged = matches!(next, MergeableVec::Merged(_));
    let (strategy, mut next_value, discard_when_merged) = match next {
        MergeableVec::Vec(v) => (None, v, false),
        MergeableVec::Merged(v) => (v.strategy, v.value, v.discard_when_merged),
    };

    // Deduplicate unless a config explicitly opts out. Applying the same config
    // source twice — through an `extends` diamond, or by re-supplying `--cfg`
    // on a conversation that already merged it — must not duplicate entries.
    //
    // Only merges that actually combine two sources deduplicate by default. A
    // `replace` contributes no second source, so duplicates in it are the
    // author's own data: free-form `JsonValue` arrays (tool options, template
    // values) are merged through here with an implicit `replace`, and their
    // contents must survive untouched. An explicit `dedup = true` still applies.
    let dedup_active = if matches!(strategy, Some(MergedVecStrategy::Replace)) {
        dedup == Some(true)
    } else {
        dedup.unwrap_or(true)
    };

    let mut value = match strategy {
        None | Some(MergedVecStrategy::Append) => {
            prev_value.append(&mut next_value);
            prev_value
        }
        Some(MergedVecStrategy::Prepend) => {
            next_value.append(&mut prev_value);
            next_value
        }
        Some(MergedVecStrategy::Replace) => next_value,
    };

    if dedup_active {
        dedup_in_place(&mut value);
    }

    // An explicit opinion needs the `Merged` wrapper to survive the next merge.
    // Without one, the shape is left alone.
    Ok(Some(if next_is_merged || dedup.is_some() {
        MergeableVec::Merged(MergedVec {
            value,
            strategy,
            dedup,
            discard_when_merged,
        })
    } else {
        MergeableVec::Vec(value)
    }))
}

/// Extract the explicit dedup flag from a `MergeableVec`.
const fn dedup_flag<T>(v: &MergeableVec<T>) -> Option<bool> {
    match v {
        MergeableVec::Merged(m) => m.dedup,
        MergeableVec::Vec(_) => None,
    }
}

/// Attach an explicit `dedup` opinion to a `MergeableVec`, wrapping a plain
/// `Vec` in `Merged` so the flag survives the next merge.
///
/// A `None` opinion is left implicit and the value's shape is unchanged.
fn with_dedup_flag<T>(v: MergeableVec<T>, dedup: Option<bool>) -> MergeableVec<T> {
    if dedup.is_none() {
        return v;
    }

    match v {
        MergeableVec::Vec(value) => MergeableVec::Merged(MergedVec {
            value,
            strategy: None,
            dedup,
            discard_when_merged: false,
        }),
        MergeableVec::Merged(mut m) => {
            m.dedup = dedup;
            MergeableVec::Merged(m)
        }
    }
}

/// Remove duplicate items in-place, preserving insertion order.
fn dedup_in_place<T: PartialEq>(vec: &mut Vec<T>) {
    let mut i = 0;
    while i < vec.len() {
        if vec[..i].iter().any(|prev| prev == &vec[i]) {
            vec.remove(i);
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
#[path = "vec_tests.rs"]
mod tests;
