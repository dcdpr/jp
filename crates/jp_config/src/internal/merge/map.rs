//! Map merge strategies.

#![expect(clippy::trivially_copy_pass_by_ref)]

use indexmap::map::Entry;
use schematic::{MergeError, MergeResult, PartialConfig, Schematic};
use serde::{Serialize, de::DeserializeOwned};

use crate::types::map::{MergeableMap, MergedMap, MergedMapStrategy};

/// Merge two [`MergeableMap`] values.
pub fn map_with_strategy<T>(
    prev: MergeableMap<T>,
    next: MergeableMap<T>,
    context: &(),
) -> MergeResult<MergeableMap<T>>
where
    T: Clone + PartialEq + Serialize + DeserializeOwned + Schematic + PartialConfig<Context = ()>,
{
    if prev.discard_when_merged() {
        return Ok(Some(next));
    }

    let mut prev_map = prev.into_map();

    let next_is_merged = matches!(next, MergeableMap::Merged(_));
    let (strategy, next_map, discard_when_merged) = match next {
        MergeableMap::Map(v) => (None, v, false),
        MergeableMap::Merged(v) => (v.strategy, v.value, v.discard_when_merged),
    };

    let value = match strategy {
        None | Some(MergedMapStrategy::DeepMerge) => {
            for (key, next_val) in next_map {
                match prev_map.entry(key) {
                    // Use PartialConfig::merge for recursive merge.
                    Entry::Occupied(mut e) => {
                        e.get_mut()
                            .merge(context, next_val)
                            .map_err(MergeError::new)?;
                    }
                    Entry::Vacant(e) => {
                        e.insert(next_val);
                    }
                }
            }
            prev_map
        }
        Some(MergedMapStrategy::Merge) => {
            for (key, next_val) in next_map {
                prev_map.insert(key, next_val);
            }
            prev_map
        }
        Some(MergedMapStrategy::Keep) => {
            for (key, next_val) in next_map {
                prev_map.entry(key).or_insert(next_val);
            }
            prev_map
        }
        Some(MergedMapStrategy::Replace) => next_map,
    };

    Ok(Some(if next_is_merged {
        MergeableMap::Merged(MergedMap {
            value,
            strategy,
            discard_when_merged,
        })
    } else {
        MergeableMap::Map(value)
    }))
}

#[cfg(test)]
#[path = "map_tests.rs"]
mod tests;
