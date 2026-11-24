//! Which comments of a review show beside one of its files, in what order, and
//! what of the file's text they mark.
//!
//! The comments panel lists the comments on the review as a whole, on the whole
//! file, and on text that changed since they were written first, in the order
//! they were written, and then the comments on a range of the file, in the
//! order their ranges start in it.
//! A comment on a range has that range highlighted in the file: its passage.
//! Picking a card brings out its passage, and picking a passage its card; where
//! several comments are on the same text, picking it again brings out the next
//! ([`next`]).
//! Resolved comments are listed and highlighted only when asked for, and
//! comments on the review's other files show on their own pages.
//!
//! On a wide screen, what is about one passage opens in the file's text, right
//! under the passage ([`inline`]): the thread of the comment picked, and the
//! composer while it writes a comment on a range, an edit of one, or a reply.
//! Anything else the composer writes opens under the button that asked for it,
//! and a narrow screen keeps the composer in a sheet ([`site`]).
//!
//! The file's outline lists its headings, and marks the one being read
//! ([`reading`]).
//!
//! The passages the reviewer approved are highlighted too ([`approved`]), those
//! that changed since are listed with the comments ([`changed_since`]), and a
//! line across the text says how far the file was read ([`read_line`]).

use std::ops::RangeInclusive;

use jp_review_api::{Approval, Comment, Heading, ReadUntil, Status};

use crate::{
    actions,
    composer::{Kind, Target},
};

/// The highlight drawn over the text of draft comments.
pub const DRAFT: &str = "jp-draft";

/// The highlight drawn over the text of open comments.
pub const OPEN: &str = "jp-open";

/// The highlight drawn over the text of resolved comments, when they are shown.
pub const RESOLVED: &str = "jp-resolved";

/// The highlight drawn over the passages the reviewer approved.
pub const APPROVED: &str = "jp-approved";

/// The highlight drawn over lines a changes view shows as changed.
pub const CHANGED: &str = "jp-changed";

/// The highlight drawn over the text being commented on: the selection the
/// reader is offered to comment on, or the range the composer writes about.
pub const PENDING: &str = "jp-pending";

/// The highlight drawn over the passage of the comment picked.
pub const FOCUS: &str = "jp-focus";

/// Every highlight the UI draws, a later one over an earlier one where two
/// cover the same text.
pub const HIGHLIGHTS: [&str; 7] = [RESOLVED, APPROVED, OPEN, DRAFT, CHANGED, PENDING, FOCUS];

/// Whether `comment` shows on the page of `file`: it is on the review as a
/// whole, or on `file`.
fn on_page(comment: &Comment, file: &str) -> bool {
    comment
        .anchor
        .path
        .as_deref()
        .is_none_or(|path| path == file)
}

/// The bytes of `file` that `comment` is on, in the version shown; `None` for a
/// comment on another file, on no range, or on text that changed since.
#[must_use]
pub fn passage(comment: &Comment, file: &str) -> Option<[usize; 2]> {
    let anchor = &comment.anchor;
    if anchor.path.as_deref() != Some(file) || anchor.range.is_none() {
        return None;
    }

    anchor.projected.as_ref()?.bytes
}

/// The comments the panel lists on the page of `file`, by their index in
/// `comments`, in the order it lists them; those resolved only when `resolved`
/// says so.
#[must_use]
pub fn order(comments: &[Comment], file: &str, resolved: bool) -> Vec<usize> {
    let listed = |comment: &Comment| {
        on_page(comment, file) && (resolved || comment.status != Status::Resolved)
    };
    let mut general = vec![];
    let mut ranged = vec![];
    for (index, comment) in comments.iter().enumerate().filter(|(_, c)| listed(c)) {
        match passage(comment, file) {
            Some([start, _]) => ranged.push((start, index)),
            None => general.push(index),
        }
    }
    ranged.sort_unstable();

    general
        .into_iter()
        .chain(ranged.into_iter().map(|(_, index)| index))
        .collect()
}

/// How many resolved comments the page of `file` lists when asked to.
#[must_use]
pub fn resolved(comments: &[Comment], file: &str) -> usize {
    comments
        .iter()
        .filter(|comment| on_page(comment, file) && comment.status == Status::Resolved)
        .count()
}

/// The passage of each comment highlighted in `file`, with the comment's
/// number; those resolved only when `resolved` says so.
#[must_use]
pub fn passages(comments: &[Comment], file: &str, resolved: bool) -> Vec<(u32, [usize; 2])> {
    comments
        .iter()
        .filter(|comment| resolved || comment.status != Status::Resolved)
        .filter_map(|comment| Some((actions::number(comment)?, passage(comment, file)?)))
        .collect()
}

/// Of the passages a point of the file falls in, each with its comment's
/// number, the comment to bring out: the one with the shortest passage, which
/// is the inner one of two that nest, and the first of those as short.
///
/// When `picked` is one of them, it is the comment after `picked` instead, from
/// the shortest passage to the longest and around again, so clicking the same
/// text again brings out each comment on it in turn.
#[must_use]
pub fn next(hits: &[(u32, [usize; 2])], picked: Option<u32>) -> Option<u32> {
    let mut order = hits.to_vec();
    // Stable, so passages as short stay in the order given.
    order.sort_by_key(|(_, [start, end])| end.saturating_sub(*start));

    let after = order
        .iter()
        .position(|(number, _)| Some(*number) == picked)
        .map_or(0, |index| (index + 1) % order.len());

    order.get(after).map(|(number, _)| *number)
}

/// The passage of each approval that stands in `file` as the version shown
/// holds it, with the approval's number.
#[must_use]
pub fn approved(approvals: &[Approval], file: &str) -> Vec<(u32, [usize; 2])> {
    approvals
        .iter()
        .filter(|approval| approval.anchor.path.as_deref() == Some(file))
        .filter_map(|approval| Some((approval.number, approval.anchor.projected.as_ref()?.bytes?)))
        .collect()
}

/// The approvals of passages of `file` that changed since they were approved:
/// those that don't stand in version `shown`, made in it or in an earlier one.
///
/// An approval made in a later version than the one shown is of text that
/// version doesn't hold yet.
#[must_use]
pub fn changed_since<'a>(approvals: &'a [Approval], file: &str, shown: u32) -> Vec<&'a Approval> {
    approvals
        .iter()
        .filter(|approval| approval.anchor.path.as_deref() == Some(file))
        .filter(|approval| approval.anchor.outdated && approval.anchor.version <= shown)
        .collect()
}

/// The bytes of the last line of `file` the reviewer read, in the version
/// shown; `None` when they didn't say how far they read it, or that version
/// holds none of the lines read.
#[must_use]
pub fn read_line(read: &[ReadUntil], file: &str) -> Option<[usize; 2]> {
    read.iter()
        .find(|read| read.path == file)?
        .projected
        .as_ref()?
        .bytes
}

/// What sits in the text of a file, right under a passage of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inline {
    /// The composer alone, under these bytes: writing a new comment on them, or
    /// an edit of the comment on them, which the fields it edits stand in for.
    Composer([usize; 2]),

    /// The thread of the comment numbered `number`, under its passage, and
    /// under the thread the composer, while it writes a reply to it.
    Thread {
        number: u32,
        bytes: [usize; 2],
        replying: bool,
    },
}

impl Inline {
    /// The bytes of the file it sits under.
    #[must_use]
    pub fn bytes(self) -> [usize; 2] {
        match self {
            Self::Composer(bytes) | Self::Thread { bytes, .. } => bytes,
        }
    }

    /// Whether the composer sits in the file's text.
    #[must_use]
    pub fn composing(self) -> bool {
        match self {
            Self::Composer(_) => true,
            Self::Thread { replying, .. } => replying,
        }
    }
}

/// What sits in the text of `file`, shown as version `shown`, where `comments`
/// are placed in version `placed`: what the composer writes, `draft`, when that
/// is about a passage of the file, or else the thread of the comment `picked`.
///
/// `None` when neither has a passage in the file as shown.
/// What is about no passage (the review, a whole file, text that changed since,
/// a pass) is written beside the file.
#[must_use]
pub fn inline(
    draft: Option<&Kind>,
    picked: Option<u32>,
    comments: &[Comment],
    file: &str,
    [shown, placed]: [u32; 2],
) -> Option<Inline> {
    let passage_of = |number: u32| {
        let comment = comments
            .iter()
            .find(|comment| actions::number(comment) == Some(number))?;

        passage(comment, file).filter(|_| shown == placed)
    };
    let thread = |number: u32, replying: bool| {
        Some(Inline::Thread {
            number,
            bytes: passage_of(number)?,
            replying,
        })
    };

    match draft {
        Some(Kind::Comment {
            on:
                Target::Range {
                    path,
                    version,
                    bytes,
                    ..
                },
        }) => (path == file && *version == shown).then_some(Inline::Composer(*bytes)),
        Some(Kind::Edit { number, .. }) => passage_of(*number).map(Inline::Composer),
        Some(Kind::Reply { number }) => thread(*number, true),
        Some(Kind::Comment { .. } | Kind::Pass) | None => thread(picked?, false),
    }
}

/// Where the composer is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    /// In the file's text, under the passage it writes about.
    File,

    /// Under the top bar's button that sends to the assistant.
    Send,

    /// At the top of the comments, under the button that adds one.
    Comments,

    /// In a sheet over the bottom of the screen, above the on-screen keyboard.
    Sheet,
}

/// Where the composer writing a draft of `kind` is drawn.
///
/// `in_file` says the draft is about a passage the file's text shows, which the
/// composer sits under ([`inline`]).
/// Otherwise a screen that isn't `wide` has it in a sheet, and a wide one under
/// the button that opened it: the one that sends a pass, or, for a comment on
/// no passage shown, the one that adds a comment.
#[must_use]
pub fn site(kind: &Kind, in_file: bool, wide: bool) -> Site {
    match kind {
        _ if in_file => Site::File,
        _ if !wide => Site::Sheet,
        Kind::Pass => Site::Send,
        Kind::Comment { .. } | Kind::Edit { .. } | Kind::Reply { .. } => Site::Comments,
    }
}

/// The levels of the headings a file's outline lists: its sections and their
/// subsections, as the documentation site's outline does.
const OUTLINED: RangeInclusive<u8> = 2..=3;

/// The headings a file's outline lists, of `headings`.
#[must_use]
pub fn outline(headings: &[Heading]) -> Vec<&Heading> {
    headings
        .iter()
        .filter(|heading| OUTLINED.contains(&heading.level))
        .collect()
}

/// The heading being read, by its index in `tops`, the distance of each of the
/// outline's headings from the top of the window, in document order: the last
/// one at or above `line`, a distance from the top of the window too.
///
/// `None` above the first heading.
#[must_use]
pub fn reading(tops: &[f64], line: f64) -> Option<usize> {
    tops.iter().rposition(|top| *top <= line)
}

/// How far down to scroll the page for a passage spanning `passage` to show
/// inside `band`, both a top and a bottom measured from the top of the window:
/// nothing for a passage that shows there whole, and otherwise what puts the
/// passage's top at the band's.
#[must_use]
pub fn scroll_for([top, bottom]: [f64; 2], [from, until]: [f64; 2]) -> f64 {
    if top >= from && bottom <= until {
        return 0.0;
    }

    top - from
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
        .filter_map(|comment| {
            let bytes = passage(comment, file)?;
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

/// How many comments on `file` are not resolved, for where the file is picked.
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
