//! What the commands show of a review: the records `jp -F json` prints and the
//! HTTP API answers with, which `render` lays out as text for a terminal.
//!
//! The records are `jp_review_api`'s types, built here from the review as
//! `review.json` holds it.
//! [`detail`] and [`comment`] place each comment's anchor in the version being
//! shown, the way `text::LineMap` carries a range from one version to another,
//! or mark it outdated there, and render each comment's and reply's body as
//! HTML.

use std::collections::{BTreeSet, HashMap};

use jp_review_api as wire;
use serde::Serialize;

use crate::{
    anchor::{Anchor, Target},
    id::{CommentRef, ReviewId},
    model::{Comment, Reply, Review, Role, Status, Version},
    pass::{Ended, Pass},
    text::{self, LineMap, Text, hunks},
};

/// File contents, by the name of the blob holding them.
pub(crate) type Texts = HashMap<String, String>;

/// What every record of a review opens with.
fn head(id: ReviewId, review: &Review) -> wire::Head {
    wire::Head {
        id: id.to_string(),
        title: review.title.clone(),
        status: review.state(),
        author: review.author.clone(),
        created_at: review.created_at,
        closed_at: review.closed_at,
        files: review.paths.clone(),
    }
}

/// The review `id`, as a listing shows it.
pub(crate) fn summary(id: ReviewId, review: &Review) -> wire::Summary {
    let mut comments = wire::Counts::default();
    for comment in &review.comments {
        match comment.status() {
            Status::Draft => comments.draft += 1,
            Status::Open => comments.open += 1,
            Status::Resolved => comments.resolved += 1,
        }
    }

    wire::Summary {
        head: head(id, review),
        latest: review.latest().map_or(0, |latest| latest.number),
        base: review.base(),
        comments,
        running: review.passes.running().map(|pass| pass.number),
    }
}

/// `review`, with its comments placed in version `shown`.
///
/// `texts` holds every blob [`blobs_needed`] names.
pub(crate) fn detail(id: ReviewId, review: &Review, shown: u32, texts: &Texts) -> wire::Detail {
    wire::Detail {
        head: head(id, review),
        revision: review.revision,
        version: shown,
        versions: review.versions.iter().map(version).collect(),
        conversation: review.passes.conversation.clone(),
        passes: review.passes.list.iter().map(pass).collect(),
        comments: review
            .comments
            .iter()
            .map(|found| comment(id, review, found, shown, texts))
            .collect(),
    }
}

fn version(version: &Version) -> wire::Version {
    wire::Version {
        number: version.number,
        label: version.label.clone(),
        created_at: version.created_at,
        files: version.files.clone(),
    }
}

pub(crate) fn pass(pass: &Pass) -> wire::Pass {
    wire::Pass {
        number: pass.number,
        author: pass.author.clone(),
        created_at: pass.created_at,
        version: pass.version,
        comments: pass.comments.clone(),
        message: pass.message.clone(),
        conversation: pass.conversation.clone(),
        ended: pass.ended.as_ref().map(ended),
    }
}

fn ended(ended: &Ended) -> wire::Ended {
    wire::Ended {
        outcome: ended.outcome,
        at: ended.at,
        version: ended.version,
        error: ended.error.clone(),
        answer: ended.answer.clone(),
    }
}

/// `found`, a comment on `review`, with its anchor placed in version `shown`.
///
/// `texts` holds every blob [`blobs_of`] names for the comment.
pub(crate) fn comment(
    id: ReviewId,
    review: &Review,
    found: &Comment,
    shown: u32,
    texts: &Texts,
) -> wire::Comment {
    let content = &found.content;
    let projected = place(review, &content.anchor, shown, texts);
    let resolved = projected
        .is_none()
        .then(|| last_resolved(review, &content.anchor, shown, texts))
        .flatten();
    let (path, range) = match &content.anchor.target {
        Target::Review => (None, None),
        Target::File(path) => (Some(path.clone()), None),
        Target::Range(path, range) => (Some(path.clone()), Some(range.to_string())),
    };

    wire::Comment {
        id: CommentRef {
            review: id,
            number: found.number,
        }
        .to_string(),
        status: found.status(),
        role: found.role,
        author: found.author.clone(),
        created_at: found.created_at,
        anchor: wire::AnchorView {
            version: content.anchor.version,
            path,
            range,
            outdated: projected.is_none(),
            projected,
            resolved,
        },
        excerpt: excerpt(review, &content.anchor, texts).map(ToOwned::to_owned),
        body: content.body.clone(),
        body_html: html(&content.body),
        suggestion: content.suggestion.clone(),
        replies: found.replies.iter().map(reply).collect(),
        draft_reply: found.draft_reply.as_ref().map(reply),
        resolved_at: found.resolved_at,
    }
}

fn reply(reply: &Reply) -> wire::Reply {
    wire::Reply {
        role: reply.role,
        author: reply.author.clone(),
        created_at: reply.created_at,
        body: reply.body.clone(),
        body_html: html(&reply.body),
        verdict: reply.verdict,
        pass: reply.pass,
    }
}

/// `markdown` rendered as HTML, raw HTML in it escaped.
fn html(markdown: &str) -> String {
    md_html::render(markdown).html
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
fn last_resolved(
    review: &Review,
    anchor: &Anchor,
    shown: u32,
    texts: &Texts,
) -> Option<wire::Placed> {
    (anchor.version..shown)
        .rev()
        .find_map(|version| place(review, anchor, version, texts))
}

/// Where `anchor` falls in version `shown`, or `None` when it is outdated
/// there: its file is absent, or a line its range covers changed.
fn place(review: &Review, anchor: &Anchor, shown: u32, texts: &Texts) -> Option<wire::Placed> {
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
            (Some(projected.to_string()), Some([bytes.start, bytes.end]))
        }
    };

    Some(wire::Placed {
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

/// How `path` changed from `old` to `new`, each `None` when the file was
/// absent; `None` when it didn't change.
///
/// The lines added, modified, and removed are placed in `new`, which a deleted
/// file has none of.
pub(crate) fn file_diff(
    path: &str,
    old: Option<&str>,
    new: Option<&str>,
) -> Option<wire::FileDiff> {
    let change = match (old, new) {
        (None, None) => return None,
        (Some(old), Some(new)) if old == new => return None,
        (None, Some(_)) => wire::Change::Added,
        (Some(_), None) => wire::Change::Deleted,
        (Some(_), Some(_)) => wire::Change::Modified,
    };
    let placed = new.map_or_default(|new| text::changes(old.unwrap_or_default(), new));

    Some(wire::FileDiff {
        path: path.to_owned(),
        change,
        diff: hunks(old.unwrap_or_default(), new.unwrap_or_default()),
        added: placed.added,
        removed: placed.removed,
    })
}
