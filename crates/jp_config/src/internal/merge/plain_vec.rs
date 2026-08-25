//! Merge strategies for plain `Vec` fields.
//!
//! These operate on `Vec<T>` directly, unlike [`vec_with_strategy`], which
//! reads its strategy from a [`MergeableVec`] wrapper.
//!
//! [`MergeableVec`]: crate::types::vec::MergeableVec
//! [`vec_with_strategy`]: super::vec_with_strategy

use schematic::MergeResult;

/// Append `next` to `prev`, dropping items already present.
///
/// Comparison uses `PartialEq` and the first occurrence wins, so the result
/// keeps `prev`'s order with `next`'s new items appended.
///
/// Only combining merges reach this function: schematic's `merge_setting`
/// invokes a merge strategy only when both layers supply a value, so a list
/// supplied by a single layer is stored as written, duplicates included.
/// That is the same rule `replace` follows on [`MergeableVec`] — repeated
/// items within one source are the author's own data, not something a merge of
/// two sources should rewrite.
///
/// Deduplicating here rather than through a `transform` is deliberate:
/// transforms run in [`PartialConfig::finalize`], which JP's config pipeline
/// never calls — it merges layers with `load_partial` and resolves them with
/// `AppConfig::from_partial_with_defaults`.
///
/// [`MergeableVec`]: crate::types::vec::MergeableVec
/// [`PartialConfig::finalize`]: schematic::PartialConfig::finalize
#[expect(clippy::unnecessary_wraps)]
pub fn append_vec_dedup<T: PartialEq, C>(
    mut prev: Vec<T>,
    next: Vec<T>,
    _: &C,
) -> MergeResult<Vec<T>> {
    for item in next {
        if !prev.contains(&item) {
            prev.push(item);
        }
    }

    Ok(Some(prev))
}

#[cfg(test)]
#[path = "plain_vec_tests.rs"]
mod tests;
