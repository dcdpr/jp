//! String merge strategies.

#![expect(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]

use schematic::MergeResult;

use crate::types::string::{MergedStringStrategy, PartialMergeableString, PartialMergedString};

/// Merge two ` PartialMergeableString  ` values.
pub fn string_with_strategy(
    prev: PartialMergeableString,
    next: PartialMergeableString,
    _context: &(),
) -> MergeResult<PartialMergeableString> {
    // Resolve the explicit dedup opinion: next's choice wins, then inherit from
    // prev. `None` means neither side expressed one.
    //
    // A discarded prev still contributes dedup when next has no opinion, but
    // NOT when next explicitly sets it.
    let dedup = dedup_flag(&next).or_else(|| dedup_flag(&prev));

    // If prev is default, replace regardless of strategy.
    if prev.discard_when_merged() {
        return Ok(Some(with_dedup_flag(next, dedup)));
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

    // Skip an append or prepend whose value is already present, unless a config
    // explicitly opts out. Applying the same config source twice — through an
    // `extends` diamond, or by re-supplying `--cfg` on a conversation that
    // already merged it — must not duplicate its contribution.
    let dedup_active = dedup.unwrap_or(true);

    // An unstated strategy means `append`, matching `MergedStringStrategy`'s
    // default. Only the computation resolves it; the stored `strategy` keeps
    // the unstated form so later merges can still express their own.
    let resolved_strategy = strategy.unwrap_or_default();

    let sep = separator.as_ref().map_or("", |sep| sep.as_str());
    let value = match (prev_value, next_value) {
        (_, n) if resolved_strategy == MergedStringStrategy::Replace => n,
        (Some(p), Some(n)) if dedup_active && contains_block(&p, &n, sep) => Some(p),
        (Some(p), Some(n)) if resolved_strategy == MergedStringStrategy::Append => {
            Some(format!("{p}{sep}{n}"))
        }
        (Some(p), Some(n)) if resolved_strategy == MergedStringStrategy::Prepend => {
            Some(format!("{n}{sep}{p}"))
        }
        (Some(p), None) => Some(p),
        (None, Some(n)) => Some(n),
        _ => None,
    };

    Ok(Some(if next_is_replace {
        with_dedup_flag(
            PartialMergeableString::String(value.unwrap_or_default()),
            dedup,
        )
    } else {
        PartialMergeableString::Merged(PartialMergedString {
            value,
            strategy,
            separator,
            discard_when_merged,
            dedup,
        })
    }))
}

/// The explicit `dedup` opinion carried by a value, if any.
const fn dedup_flag(v: &PartialMergeableString) -> Option<bool> {
    match v {
        PartialMergeableString::String(_) => None,
        PartialMergeableString::Merged(m) => m.dedup,
    }
}

/// Attach an explicit `dedup` opinion, wrapping a plain string in `Merged` with
/// a `replace` strategy so the flag survives the next merge.
///
/// A `None` opinion is left implicit and the value's shape is unchanged.
fn with_dedup_flag(v: PartialMergeableString, dedup: Option<bool>) -> PartialMergeableString {
    if dedup.is_none() {
        return v;
    }

    match v {
        PartialMergeableString::String(value) => {
            PartialMergeableString::Merged(PartialMergedString {
                value: Some(value),
                strategy: Some(MergedStringStrategy::Replace),
                separator: None,
                discard_when_merged: None,
                dedup,
            })
        }
        PartialMergeableString::Merged(mut m) => {
            m.dedup = dedup;
            PartialMergeableString::Merged(m)
        }
    }
}

/// Whether `needle` already appears in `haystack` as a whole
/// `separator`-delimited block.
///
/// A match must start at the beginning of `haystack` or right after a
/// separator, and end at the end of `haystack` or right before one.
/// Anchoring on both sides keeps a value that merely occurs inside a larger
/// block from counting as present.
///
/// An empty separator leaves no boundaries to anchor on, so only an exact match
/// of the whole string counts.
fn contains_block(haystack: &str, needle: &str, separator: &str) -> bool {
    if needle.is_empty() || haystack == needle {
        return true;
    }

    if separator.is_empty() {
        return false;
    }

    haystack.match_indices(needle).any(|(index, matched)| {
        let end = index + matched.len();
        let starts_block = index == 0 || haystack[..index].ends_with(separator);
        let ends_block = end == haystack.len() || haystack[end..].starts_with(separator);

        starts_block && ends_block
    })
}

#[cfg(test)]
#[path = "string_tests.rs"]
mod tests;
