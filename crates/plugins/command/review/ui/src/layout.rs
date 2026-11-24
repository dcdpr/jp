//! Where each comment of a review shows on the page of one of its files.
//!
//! Comments on the review as a whole, on the whole file, and on text that
//! changed since they were written go above the file.
//! A comment on a range of the file goes under the block its range starts in,
//! and its range is highlighted.
//! Comments on the review's other files show on their own pages.

use jp_review_api::{Comment, Status};

use crate::blocks::Block;

/// The highlight drawn over the text of draft comments.
pub const DRAFT: &str = "jp-draft";

/// The highlight drawn over the text of open comments.
pub const OPEN: &str = "jp-open";

/// The highlight drawn over the text of resolved comments, when they are shown.
pub const RESOLVED: &str = "jp-resolved";

/// The highlight drawn over lines a changes view shows as changed.
pub const CHANGED: &str = "jp-changed";

/// The highlight drawn over the text being commented on: the selection the
/// comment bar offers, or the range the composer writes about.
pub const PENDING: &str = "jp-pending";

/// Every highlight the UI draws.
pub const HIGHLIGHTS: [&str; 5] = [DRAFT, OPEN, RESOLVED, CHANGED, PENDING];

/// Where the comments on one file's page go, by their index in the review's
/// comments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    /// Above the file: on the review, on the whole file, or on text that
    /// changed since.
    pub above: Vec<usize>,

    /// Under each block, by the block's index.
    pub under: Vec<Vec<usize>>,
}

/// Where each of `comments` goes on the page of `file`, rendered as `blocks`.
#[must_use]
pub fn place(comments: &[Comment], file: &str, blocks: &[Block]) -> Placement {
    let mut placement = Placement {
        above: vec![],
        under: vec![vec![]; blocks.len()],
    };

    for (index, comment) in comments.iter().enumerate() {
        let anchor = &comment.anchor;
        match (anchor.path.as_deref(), anchor.range.as_deref()) {
            (Some(path), _) if path != file => {}
            (None, _) | (Some(_), None) => placement.above.push(index),
            (Some(_), Some(_)) => {
                let block = anchor
                    .projected
                    .as_ref()
                    .and_then(|placed| placed.bytes)
                    .and_then(|[start, _]| block_at(blocks, start));
                match block {
                    Some(block) => placement.under[block].push(index),
                    None => placement.above.push(index),
                }
            }
        }
    }

    placement
}

/// The block holding source byte `offset`: the first whose text ends after it,
/// which is the next block when `offset` falls between two, or the last block
/// with text when none does.
#[must_use]
pub fn block_at(blocks: &[Block], offset: usize) -> Option<usize> {
    let mapped = || {
        blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| block.bytes.map(|bytes| (index, bytes)))
    };

    mapped()
        .find(|(_, [_, end])| *end > offset)
        .or_else(|| mapped().next_back())
        .map(|(index, _)| index)
}

/// The ranges of `file` to highlight, and the highlight of each: every comment
/// on a range of it that stands in the version shown, those resolved only when
/// `resolved` says so.
#[must_use]
pub fn highlights(
    comments: &[Comment],
    file: &str,
    resolved: bool,
) -> Vec<(&'static str, [usize; 2])> {
    comments
        .iter()
        .filter(|comment| comment.anchor.path.as_deref() == Some(file))
        .filter_map(|comment| {
            let bytes = comment.anchor.projected.as_ref()?.bytes?;
            let name = match comment.status {
                Status::Draft => DRAFT,
                Status::Open => OPEN,
                Status::Resolved if resolved => RESOLVED,
                Status::Resolved => return None,
            };
            Some((name, bytes))
        })
        .collect()
}

/// How many comments on `file` are not resolved, for its tab.
#[must_use]
pub fn unresolved(comments: &[Comment], file: &str) -> usize {
    comments
        .iter()
        .filter(|comment| comment.anchor.path.as_deref() == Some(file))
        .filter(|comment| comment.status != Status::Resolved)
        .count()
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
