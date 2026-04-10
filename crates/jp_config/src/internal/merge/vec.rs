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
    let prev_dedup = dedup_flag(&prev);
    let next_dedup = dedup_flag(&next);

    // Resolve dedup: next's explicit choice wins, then inherit from prev.
    //
    // A discarded prev still contributes dedup when next has no opinion (None /
    // "inherit"), but NOT when next explicitly sets it.
    let dedup = next_dedup.or(prev_dedup).unwrap_or(false);

    // If prev is default, replace regardless of strategy.
    if prev.discard_when_merged() {
        if dedup {
            let mut next = ensure_dedup(next);
            dedup_in_place(&mut next);
            return Ok(Some(next));
        }

        return Ok(Some(next));
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

    if dedup {
        dedup_in_place(&mut value);
    }

    // Carry forward as Option<bool>: Some(true) when active, None otherwise.
    let resolved_dedup = if dedup { Some(true) } else { None };

    // When dedup is active, always use Merged to carry the flag forward.
    Ok(Some(if next_is_merged || dedup {
        MergeableVec::Merged(MergedVec {
            value,
            strategy,
            dedup: resolved_dedup,
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

/// Ensure the dedup flag is set on a `MergeableVec`.
fn ensure_dedup<T>(v: MergeableVec<T>) -> MergeableVec<T> {
    match v {
        MergeableVec::Vec(value) => MergeableVec::Merged(MergedVec {
            value,
            strategy: None,
            dedup: Some(true),
            discard_when_merged: false,
        }),
        MergeableVec::Merged(mut m) => {
            m.dedup = Some(true);
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
