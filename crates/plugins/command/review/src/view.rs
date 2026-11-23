//! What the commands show of a review: the records `jp -F json` prints, which
//! `render` lays out as text for a terminal.
//!
//! [`detail`] and [`comment`] place each comment's anchor in the version being
//! shown, the way `text::LineMap` carries a range from one version to another,
//! or mark it outdated there.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{
    anchor::{Anchor, Target},
    id::{CommentRef, ReviewId},
    model::{Comment, Reply, Review, Role, State, Status, Version},
    pass::Pass,
    text::{LineMap, Range, Text, hunks},
};

/// File contents, by the name of the blob holding them.
pub(crate) type Texts = HashMap<String, String>;

/// What every record of a review opens with.
#[derive(Debug, Serialize)]
pub(crate) struct Head<'a> {
    pub id: ReviewId,
    pub title: &'a str,

    #[serde(rename = "status")]
    pub state: State,

    pub author: &'a str,
    pub created_at: DateTime<Utc>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,

    pub files: &'a [String],
}

impl<'a> Head<'a> {
    fn of(id: ReviewId, review: &'a Review) -> Self {
        Self {
            id,
            title: &review.title,
            state: review.state(),
            author: &review.author,
            created_at: review.created_at,
            closed_at: review.closed_at,
            files: &review.paths,
        }
    }
}

/// A review in a listing.
#[derive(Debug, Serialize)]
pub(crate) struct Summary<'a> {
    #[serde(flatten)]
    pub head: Head<'a>,

    /// The number of the latest version.
    pub latest: u32,

    /// The version labelled `base`, if there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<u32>,

    pub comments: Counts,
}

/// How many of a review's comments stand where.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Counts {
    pub draft: usize,
    pub open: usize,
    pub resolved: usize,
}

pub(crate) fn summary(id: ReviewId, review: &Review) -> Summary<'_> {
    let mut comments = Counts::default();
    for comment in &review.comments {
        match comment.status() {
            Status::Draft => comments.draft += 1,
            Status::Open => comments.open += 1,
            Status::Resolved => comments.resolved += 1,
        }
    }

    Summary {
        head: Head::of(id, review),
        latest: review.latest().map_or(0, |latest| latest.number),
        base: review.base(),
        comments,
    }
}

/// A review, with its comments placed in one version.
#[derive(Debug, Serialize)]
pub(crate) struct Detail<'a> {
    #[serde(flatten)]
    pub head: Head<'a>,

    pub revision: u64,

    /// The version the comments are placed in.
    pub version: u32,

    pub versions: &'a [Version],

    /// The conversation the review's passes run in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<&'a str>,

    #[serde(skip_serializing_if = "<[Pass]>::is_empty")]
    pub passes: &'a [Pass],

    pub comments: Vec<CommentView<'a>>,
}

/// `review`, with its comments placed in version `shown`.
///
/// `texts` holds every blob [`blobs_needed`] names.
pub(crate) fn detail<'a>(
    id: ReviewId,
    review: &'a Review,
    shown: u32,
    texts: &'a Texts,
) -> Detail<'a> {
    Detail {
        head: Head::of(id, review),
        revision: review.revision,
        version: shown,
        versions: &review.versions,
        conversation: review.passes.conversation.as_deref(),
        passes: &review.passes.list,
        comments: review
            .comments
            .iter()
            .map(|found| comment(id, review, found, shown, texts))
            .collect(),
    }
}

/// A comment, with its anchor placed in the version shown.
#[derive(Debug, Serialize)]
pub(crate) struct CommentView<'a> {
    pub id: CommentRef,
    pub status: Status,
    pub role: Role,
    pub author: &'a str,
    pub created_at: DateTime<Utc>,
    pub anchor: AnchorView<'a>,

    /// The text the anchor covers, in the version it was written against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<&'a str>,

    pub body: &'a str,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<&'a str>,

    #[serde(skip_serializing_if = "<[Reply]>::is_empty")]
    pub replies: &'a [Reply],

    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_reply: Option<&'a Reply>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
}

/// An anchor as it is stored, and where it falls in the version shown.
#[derive(Debug, Serialize)]
pub(crate) struct AnchorView<'a> {
    #[serde(flatten)]
    pub anchor: &'a Anchor,

    /// Where the anchor falls in the version shown; absent when it is outdated
    /// there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected: Option<Placed>,

    pub outdated: bool,

    /// Where the anchor last stood, for one outdated in the version shown: the
    /// latest version before it that still holds the anchor's text unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Placed>,
}

/// Where an anchor falls in a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Placed {
    pub version: u32,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,

    /// The UTF-8 byte offsets `range` covers in the version, end exclusive: how
    /// a browser addresses the text.
    /// A range of whole lines covers them without the last one's line ending.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<[usize; 2]>,
}

/// `found`, a comment on `review`, with its anchor placed in version `shown`.
///
/// `texts` holds every blob [`blobs_of`] names for the comment.
pub(crate) fn comment<'a>(
    id: ReviewId,
    review: &'a Review,
    found: &'a Comment,
    shown: u32,
    texts: &'a Texts,
) -> CommentView<'a> {
    let content = &found.content;
    let projected = place(review, &content.anchor, shown, texts);
    let resolved = projected
        .is_none()
        .then(|| last_resolved(review, &content.anchor, shown, texts))
        .flatten();

    CommentView {
        id: CommentRef {
            review: id,
            number: found.number,
        },
        status: found.status(),
        role: found.role,
        author: &found.author,
        created_at: found.created_at,
        anchor: AnchorView {
            anchor: &content.anchor,
            projected,
            outdated: projected.is_none(),
            resolved,
        },
        excerpt: excerpt(review, &content.anchor, texts),
        body: &content.body,
        suggestion: content.suggestion.as_deref(),
        replies: &found.replies,
        draft_reply: found.draft_reply.as_ref(),
        resolved_at: found.resolved_at,
    }
}

/// The blobs [`detail`] reads to place the comments of `review` in version
/// `shown`.
pub(crate) fn blobs_needed(review: &Review, shown: u32) -> BTreeSet<&str> {
    review
        .comments
        .iter()
        .flat_map(|found| blobs_of(review, found, shown))
        .collect()
}

/// The blobs [`comment`] reads to place `found` in version `shown`: the file
/// its range is in, as each version from the anchor's to `shown` holds it.
pub(crate) fn blobs_of<'a>(review: &'a Review, found: &Comment, shown: u32) -> Vec<&'a str> {
    let anchor = &found.content.anchor;
    let Target::Range(path, _) = &anchor.target else {
        return vec![];
    };

    (anchor.version.min(shown)..=anchor.version.max(shown))
        .filter_map(|version| review.blob(version, path))
        .collect()
}

/// The latest version after the anchor's and before `shown` where `anchor`
/// still stands, or the anchor's own; `None` when `shown` comes before the
/// anchor's version.
fn last_resolved(review: &Review, anchor: &Anchor, shown: u32, texts: &Texts) -> Option<Placed> {
    (anchor.version..shown)
        .rev()
        .find_map(|version| place(review, anchor, version, texts))
}

/// Where `anchor` falls in version `shown`, or `None` when it is outdated
/// there: its file is absent, or a line its range covers changed.
fn place(review: &Review, anchor: &Anchor, shown: u32, texts: &Texts) -> Option<Placed> {
    let (range, bytes) = match &anchor.target {
        Target::Review => (None, None),
        Target::File(path) => {
            review.blob(shown, path)?;
            (None, None)
        }
        Target::Range(path, range) => {
            let old = Text::new(text_at(review, anchor.version, path, texts)?);
            let new = Text::new(text_at(review, shown, path, texts)?);
            let projected = LineMap::new(&old, &new).project(*range)?;
            let bytes = new.bytes(projected)?;
            (Some(projected), Some([bytes.start, bytes.end]))
        }
    };

    Some(Placed {
        version: shown,
        range,
        bytes,
    })
}

/// The text `anchor` covers, as the version it was written against holds it.
fn excerpt<'a>(review: &Review, anchor: &Anchor, texts: &'a Texts) -> Option<&'a str> {
    let Target::Range(path, range) = &anchor.target else {
        return None;
    };

    Text::new(text_at(review, anchor.version, path, texts)?).excerpt(*range)
}

/// What `path` holds in `version`, when it is there and `texts` has it.
pub(crate) fn text_at<'a>(
    review: &Review,
    version: u32,
    path: &str,
    texts: &'a Texts,
) -> Option<&'a str> {
    texts.get(review.blob(version, path)?).map(String::as_str)
}

/// How a pass went, as `jp -F json review submit` prints it.
#[derive(Debug, Serialize)]
pub(crate) struct PassReport<'a> {
    pub review: ReviewId,

    #[serde(flatten)]
    pub pass: &'a Pass,

    /// The replies the assistant wrote during the pass.
    pub replies: Vec<Answer<'a>>,

    /// The comments the assistant opened during the pass.
    pub opened: Vec<CommentRef>,

    /// The comments the pass sent that still wait on the assistant.
    pub unanswered: Vec<CommentRef>,

    /// The files that differ between the version the pass was sent against and
    /// the one it ended with.
    pub changed: Vec<&'a str>,
}

/// A reply the assistant wrote, and the comment it answers.
#[derive(Debug, Serialize)]
pub(crate) struct Answer<'a> {
    pub comment: CommentRef,

    #[serde(flatten)]
    pub reply: &'a Reply,
}

/// How pass `number` of `review` went; `None` when there is no such pass.
pub(crate) fn pass_report(id: ReviewId, review: &Review, number: u32) -> Option<PassReport<'_>> {
    let pass = review.passes.get(number)?;
    let named = |number| CommentRef { review: id, number };
    let during =
        |role: Role, written: Option<u32>| role == Role::Assistant && written == Some(number);

    let replies = review
        .comments
        .iter()
        .flat_map(|comment| {
            comment
                .replies
                .iter()
                .filter(|reply| during(reply.role, reply.pass))
                .map(move |reply| Answer {
                    comment: named(comment.number),
                    reply,
                })
        })
        .collect();
    let opened = review
        .comments
        .iter()
        .filter(|comment| during(comment.role, comment.pass))
        .map(|comment| named(comment.number))
        .collect();
    let unanswered = pass
        .comments
        .iter()
        .filter(|sent| review.comment(**sent).is_ok_and(Comment::awaits_assistant))
        .map(|sent| named(*sent))
        .collect();
    let end = pass
        .ended
        .as_ref()
        .and_then(|ended| ended.version)
        .unwrap_or(pass.version);
    let changed = review
        .paths
        .iter()
        .filter(|path| review.blob(pass.version, path) != review.blob(end, path))
        .map(String::as_str)
        .collect();

    Some(PassReport {
        review: id,
        pass,
        replies,
        opened,
        unanswered,
        changed,
    })
}

/// What changed in a review's files from one version to another.
#[derive(Debug, Serialize)]
pub(crate) struct DiffView<'a> {
    pub review: ReviewId,
    pub from: u32,
    pub to: u32,

    /// Changed files; unchanged files are omitted.
    pub files: Vec<FileDiff<'a>>,
}

/// How one file changed.
#[derive(Debug, Serialize)]
pub(crate) struct FileDiff<'a> {
    pub path: &'a str,
    pub change: Change,

    /// The unified diff hunks, without file headers.
    pub diff: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Change {
    Added,
    Deleted,
    Modified,
}

/// How `path` changed from `old` to `new`, each `None` when the file was
/// absent; `None` when it didn't change.
pub(crate) fn file_diff<'a>(
    path: &'a str,
    old: Option<&str>,
    new: Option<&str>,
) -> Option<FileDiff<'a>> {
    let change = match (old, new) {
        (None, None) => return None,
        (Some(old), Some(new)) if old == new => return None,
        (None, Some(_)) => Change::Added,
        (Some(_), None) => Change::Deleted,
        (Some(_), Some(_)) => Change::Modified,
    };

    Some(FileDiff {
        path,
        change,
        diff: hunks(old.unwrap_or_default(), new.unwrap_or_default()),
    })
}
