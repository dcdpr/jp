//! String merge strategies.

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
    // If prev is default, replace regardless of strategy.
    if prev.discard_when_merged() {
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

    let value = match strategy {
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

    Ok(Some(if next_is_merged {
        MergeableVec::Merged(MergedVec {
            value,
            strategy,
            discard_when_merged,
        })
    } else {
        MergeableVec::Vec(value)
    }))
}

#[cfg(test)]
#[path = "vec_tests.rs"]
mod tests;
