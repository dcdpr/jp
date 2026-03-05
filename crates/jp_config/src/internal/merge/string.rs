//! String merge strategies.

#![expect(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]

use schematic::MergeResult;

use crate::types::string::{MergedStringStrategy, PartialMergeableString, PartialMergedString};

/// Merge two `PartialMergeableString ` values.
pub fn string_with_strategy(
    prev: PartialMergeableString,
    next: PartialMergeableString,
    _context: &(),
) -> MergeResult<PartialMergeableString> {
    // If prev is default, replace regardless of strategy.
    if prev.discard_when_merged() {
        return Ok(Some(next));
    }

    let prev_value = match prev {
        PartialMergeableString::String(v) => Some(v),
        PartialMergeableString::Merged(v) => v.value,
    };

    let next_is_replace = matches!(next, PartialMergeableString::String(_));
    let (strategy, separator, next_value, discard_when_merged) = match next {
        PartialMergeableString::String(v) => {
            (Some(MergedStringStrategy::Replace), None, Some(v), None)
        }
        PartialMergeableString::Merged(v) => {
            (v.strategy, v.separator, v.value, v.discard_when_merged)
        }
    };

    let sep = separator.as_ref().map_or("", |sep| sep.as_str());
    let value = match (prev_value, next_value) {
        (_, n) if strategy == Some(MergedStringStrategy::Replace) => n,
        (Some(p), Some(n)) if strategy == Some(MergedStringStrategy::Append) => {
            Some(format!("{p}{sep}{n}"))
        }
        (Some(p), Some(n)) if strategy == Some(MergedStringStrategy::Prepend) => {
            Some(format!("{n}{sep}{p}"))
        }
        (Some(p), None) => Some(p),
        (None, Some(n)) => Some(n),
        _ => None,
    };

    Ok(Some(if next_is_replace {
        PartialMergeableString::String(value.unwrap_or_default())
    } else {
        PartialMergeableString::Merged(PartialMergedString {
            value,
            strategy,
            separator,
            discard_when_merged,
        })
    }))
}

#[cfg(test)]
#[path = "string_tests.rs"]
mod tests;
