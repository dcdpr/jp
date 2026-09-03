//! String merge strategies.

#![expect(clippy::unnecessary_wraps, clippy::trivially_copy_pass_by_ref)]

use schematic::MergeResult;
use tracing::debug;

use crate::types::string::{
    MergedStringStrategy, PartialMergeableString, PartialMergedString, StringDedup,
};

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

    // Skip an append or prepend whose value is already present. Applying the
    // same config source twice — through an `extends` diamond, or by
    // re-supplying `--cfg` on a conversation that already merged it — must not
    // duplicate its contribution.
    let dedup_mode = dedup.unwrap_or_default();

    // An unstated strategy means `append` and an unstated separator means
    // `paragraph`, matching the two types' defaults. Only the computation
    // resolves them; the stored fields keep the unstated form so later merges
    // can still express their own.
    let resolved_strategy = strategy.unwrap_or_default();
    let sep = separator.unwrap_or_default().as_str();
    let value = match (prev_value, next_value) {
        (_, n) if resolved_strategy == MergedStringStrategy::Replace => n,
        // Nothing on one side means nothing to separate from.
        (Some(p), Some(n)) if p.is_empty() => Some(n),
        (Some(p), Some(n)) if n.is_empty() => Some(p),
        (Some(p), Some(n)) if is_present(&p, &n, dedup_mode) => {
            // The only path that drops a value the author wrote. Nothing
            // downstream can tell it apart from a value never supplied, so a
            // reader hunting a missing prompt section has this line and
            // nothing else.
            debug!(
                mode = %dedup_mode,
                value = %excerpt(&n),
                "Skipping a value already present in the merged string."
            );
            Some(p)
        }
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
const fn dedup_flag(v: &PartialMergeableString) -> Option<StringDedup> {
    match v {
        PartialMergeableString::String(_) => None,
        PartialMergeableString::Merged(m) => m.dedup,
    }
}

/// Attach an explicit `dedup` opinion, wrapping a plain string in `Merged` with
/// a `replace` strategy so the flag survives the next merge.
///
/// A `None` opinion is left implicit and the value's shape is unchanged.
fn with_dedup_flag(
    v: PartialMergeableString,
    dedup: Option<StringDedup>,
) -> PartialMergeableString {
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

/// A short single-line excerpt of `value`, for a log field.
///
/// Merged strings run to whole prompt sections; the excerpt identifies which
/// one was skipped rather than reproducing it.
fn excerpt(value: &str) -> String {
    /// Characters kept before the excerpt is cut short.
    const MAX_CHARS: usize = 60;

    let line = value.lines().next().unwrap_or_default();

    match line.char_indices().nth(MAX_CHARS) {
        Some((index, _)) => format!("{}\u{2026}", &line[..index]),
        // The whole first line fits, but the value carries more after it.
        None if line.len() < value.len() => format!("{line}\u{2026}"),
        None => line.to_owned(),
    }
}

/// Whether `value` is already merged into `accumulated`, under `mode`.
fn is_present(accumulated: &str, value: &str, mode: StringDedup) -> bool {
    match mode {
        StringDedup::Off => false,
        StringDedup::Exact => accumulated == value,
        StringDedup::Block => contains_block(accumulated, value),
        StringDedup::Contains => accumulated.contains(value),
    }
}

/// Line-break characters, either of which bounds a block.
///
/// Both are listed so a value carrying CRLF endings is bounded on the side that
/// ends with `\r` as well as the side that starts with `\n`.
const BLOCK_BOUNDARY: [char; 2] = ['\n', '\r'];

/// Whether `needle` already appears in `haystack` as a whole block.
///
/// A match must start at the beginning of `haystack` or right after a line
/// break, and end at the end of `haystack` or right before one.
/// Anchoring on both sides keeps a value that merely occurs inside a larger
/// block from counting as present.
///
/// The anchor is the line break rather than the separator the incoming value
/// carries, because `haystack` is assembled from contributions that each chose
/// their own separator: a block appended with `paragraph` can be followed by
/// one appended with `line`, and both boundaries have to read as boundaries.
/// A value merged with `space` or `none` leaves no line break to anchor on, so
/// only a match of the whole string counts.
fn contains_block(haystack: &str, needle: &str) -> bool {
    if haystack == needle {
        return true;
    }

    haystack.match_indices(needle).any(|(index, matched)| {
        let end = index + matched.len();
        let starts_block = index == 0 || haystack[..index].ends_with(BLOCK_BOUNDARY);
        let ends_block = end == haystack.len() || haystack[end..].starts_with(BLOCK_BOUNDARY);

        starts_block && ends_block
    })
}

#[cfg(test)]
#[path = "string_tests.rs"]
mod tests;
