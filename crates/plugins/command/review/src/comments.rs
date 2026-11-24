//! Comment and reply CLI commands: `comment`, `reply`, `edit`, `withdraw`,
//! `resolve`, and `reopen` for a comment.
//!
//! Each command mutates the review via [`Store::update`] under the review's
//! file lock.
//! Because CLI line and column arguments refer to the current working tree,
//! commands that anchor a comment to a file first snapshot the working tree as
//! a new version if any tracked file has changed.

use chrono::{DateTime, Utc};
use doc::author;
use jp_review_api::{Resolved, Withdrawal};

use crate::{
    Output, anchor,
    anchor::{Anchor, Target},
    cmd::{self, Context, json, load, to_json},
    id::{CommentRef, ReviewId},
    model::{Content, Draft, Edit, NewComment, Refusal, Replied, Resolution, Review, Role},
    paths, snapshot,
    store::{Blobs, Store, now},
    text::{Range, Text},
    view,
};

/// Add a draft comment to the review `id`, on what `at` names, or on the review
/// as a whole.
pub(crate) fn comment(
    ctx: &Context<'_>,
    id: ReviewId,
    at: Option<&str>,
    replace: Option<String>,
    body: Option<&str>,
) -> Result<Output, String> {
    let author = author::resolve(&ctx.root, ctx.config)?;
    let written = now();

    let number = ctx.store.update(id, |review, blobs| {
        review.writable().map_err(|refusal| refusal.message(id))?;
        let anchor = anchor_at(ctx, review, blobs, at, written)?;
        review
            .add_comment(NewComment {
                role: Role::Reviewer,
                author,
                created_at: written,
                content: Content {
                    anchor,
                    body: body.map_or_default(str::trim).to_owned(),
                    suggestion: replace,
                },
            })
            .map_err(|refusal| refusal.message(id))
    })?;

    let comment = CommentRef { review: id, number };
    if ctx.format.is_json() {
        return show(ctx, comment);
    }

    Ok(format!("{comment}\n").into())
}

/// Resolve `--at` to an [`Anchor`]: the review as a whole in the latest version
/// when `at` is `None`, or the specified file/range in a current working-tree
/// snapshot.
pub(crate) fn anchor_at(
    ctx: &Context<'_>,
    review: &mut Review,
    blobs: &Blobs,
    at: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Anchor, String> {
    let Some(at) = at else {
        let version = review
            .latest()
            .map(|latest| latest.number)
            .ok_or("The review has no versions.")?;
        return Ok(Anchor {
            version,
            target: Target::Review,
        });
    };

    let version = snapshot::record(&ctx.root, review, blobs, now)?;
    let (root, cwd) = ctx.bases();
    let (path, range) = anchor::place(at, &review.paths, |typed| {
        paths::tracked(&root, &cwd, &review.paths, typed)
    })?;
    let target = target(review, blobs, version, path, range)?;

    Ok(Anchor { version, target })
}

/// Validate that `path` (and optional `range`) exists in `version` of `review`,
/// returning the corresponding [`Target`].
pub(crate) fn target(
    review: &Review,
    blobs: &Blobs,
    version: u32,
    path: String,
    range: Option<Range>,
) -> Result<Target, String> {
    let name = review
        .blob(version, &path)
        .ok_or_else(|| format!("{path} is absent from version {version}."))?;
    let Some(range) = range else {
        return Ok(Target::File(path));
    };

    let contents = blobs.get(name)?;
    if Text::new(&contents).bytes(range).is_none() {
        return Err(format!(
            "{path}:{range} lies outside {path} as version {version} holds it."
        ));
    }

    Ok(Target::Range(path, range))
}

/// Write the reviewer's draft reply on the open comment `comment`.
pub(crate) fn reply(ctx: &Context<'_>, comment: CommentRef, body: &str) -> Result<Output, String> {
    let author = author::resolve(&ctx.root, ctx.config)?;
    let written = now();
    let replied = ctx.store.update(comment.review, |review, _| {
        review
            .reply(comment.number, author, written, body.trim().to_owned())
            .map_err(|refusal| refusal.message(comment.review))
    })?;

    if ctx.format.is_json() {
        return show(ctx, comment);
    }

    Ok(match replied {
        Replied::Added => format!("Drafted a reply to {comment}.\n"),
        Replied::Replaced => format!("Replaced the draft reply to {comment}.\n"),
    }
    .into())
}

/// Change the draft comment `comment`, or the draft reply on it.
pub(crate) fn edit(
    ctx: &Context<'_>,
    comment: CommentRef,
    at: Option<&str>,
    replace: Option<String>,
    body: Option<String>,
) -> Result<Output, String> {
    let written = now();
    let edited = ctx.store.update(comment.review, |review, blobs| {
        review
            .writable()
            .map_err(|refusal| refusal.message(comment.review))?;
        let anchor = at
            .map(|at| anchor_at(ctx, review, blobs, Some(at), written))
            .transpose()?;
        // The command acts on the comment as it is, whichever draft it holds.
        let edit = Edit {
            anchor,
            suggestion: replace,
            body: body.map(|body| body.trim().to_owned()),
            expects: None,
        };
        review
            .edit(comment.number, edit)
            .map_err(|refusal| refusal.message(comment.review))
    })?;

    if ctx.format.is_json() {
        return show(ctx, comment);
    }

    Ok(match edited {
        Draft::Comment => format!("Edited {comment}.\n"),
        Draft::Reply => format!("Edited the draft reply to {comment}.\n"),
    }
    .into())
}

/// Withdraw the draft comment `comment`, or the draft reply on it.
pub(crate) fn withdraw(ctx: &Context<'_>, comment: CommentRef) -> Result<Output, String> {
    let withdrawn = ctx.store.update(comment.review, |review, _| {
        review
            .withdraw(comment.number)
            .map_err(|refusal| refusal.message(comment.review))
    })?;

    if ctx.format.is_json() {
        return json(&Withdrawal {
            id: comment.to_string(),
            withdrawn,
        });
    }

    Ok(match withdrawn {
        Draft::Comment => format!("Withdrew {comment}.\n"),
        Draft::Reply => format!("Withdrew the draft reply to {comment}.\n"),
    }
    .into())
}

/// Resolve the `named` comments, or, with `accepted`, every open comment in
/// that review whose last reply is the assistant accepting it.
pub(crate) fn resolve(
    ctx: &Context<'_>,
    named: &[CommentRef],
    accepted: Option<ReviewId>,
) -> Result<Output, String> {
    match accepted {
        Some(id) => resolve_accepted(ctx, id),
        None => resolve_named(ctx, named),
    }
}

/// Open the resolved comment `comment` again.
pub(crate) fn reopen(ctx: &Context<'_>, comment: CommentRef) -> Result<Output, String> {
    let reopened = ctx.store.update(comment.review, |review, _| {
        reopen_in(review, comment.number).map_err(|refusal| refusal.message(comment.review))
    })?;

    if ctx.format.is_json() {
        return show(ctx, comment);
    }

    Ok(if reopened {
        format!("Reopened {comment}.\n")
    } else {
        format!("{comment} is open already.\n")
    }
    .into())
}

/// Open comment `number` of `review` again, returning `false` when it was open
/// already.
pub(crate) fn reopen_in(review: &mut Review, number: u32) -> Result<bool, Refusal> {
    review.writable()?;

    review.comment_mut(number)?.reopen()
}

/// Resolve each of `named`, or none of them when one can't be.
fn resolve_named(ctx: &Context<'_>, named: &[CommentRef]) -> Result<Output, String> {
    let at = now();
    let groups = by_review(named);

    // Every review is tried as it reads now before any is written, so a
    // comment that can't be resolved leaves all of them as they were.
    for (id, numbers) in &groups {
        let mut review = ctx.store.read(*id)?;
        resolve_all(&mut review, numbers, at).map_err(|refusal| refusal.message(*id))?;
    }

    let mut resolved = vec![];
    for (id, numbers) in &groups {
        let outcomes = ctx.store.update(*id, |review, _| {
            resolve_all(review, numbers, at).map_err(|refusal| refusal.message(*id))
        })?;
        resolved.extend(numbers.iter().zip(outcomes).map(|(number, outcome)| {
            Resolved {
                id: CommentRef {
                    review: *id,
                    number: *number,
                }
                .to_string(),
                outcome,
            }
        }));
    }

    report(ctx, &resolved)
}

fn resolve_accepted(ctx: &Context<'_>, id: ReviewId) -> Result<Output, String> {
    let at = now();
    let resolved = ctx.store.update(id, |review, _| {
        let numbers = review.accepted();
        let outcomes = resolve_all(review, &numbers, at).map_err(|refusal| refusal.message(id))?;

        Ok::<_, String>(
            numbers
                .into_iter()
                .zip(outcomes)
                .map(|(number, outcome)| Resolved {
                    id: CommentRef { review: id, number }.to_string(),
                    outcome,
                })
                .collect::<Vec<_>>(),
        )
    })?;

    let mut output = report(ctx, &resolved)?;
    if resolved.is_empty() && !ctx.format.is_json() {
        output.notes.push(format!(
            "No open comment in {id} ends with the assistant accepting it."
        ));
    }

    Ok(output)
}

fn report(ctx: &Context<'_>, resolved: &[Resolved]) -> Result<Output, String> {
    if ctx.format.is_json() {
        return json(&resolved);
    }

    Ok(resolved
        .iter()
        .map(|resolved| match resolved.outcome {
            Resolution::Resolved => format!("Resolved {}.\n", resolved.id),
            Resolution::AlreadyResolved => format!("{} is resolved already.\n", resolved.id),
        })
        .collect::<String>()
        .into())
}

fn resolve_all(
    review: &mut Review,
    numbers: &[u32],
    at: DateTime<Utc>,
) -> Result<Vec<Resolution>, Refusal> {
    numbers
        .iter()
        .map(|number| review.resolve(*number, at))
        .collect()
}

/// `comments` grouped by review, in the order they are first named, each number
/// once.
fn by_review(comments: &[CommentRef]) -> Vec<(ReviewId, Vec<u32>)> {
    let mut groups: Vec<(ReviewId, Vec<u32>)> = vec![];
    for comment in comments {
        match groups.iter_mut().find(|(id, _)| *id == comment.review) {
            Some((_, numbers)) if numbers.contains(&comment.number) => {}
            Some((_, numbers)) => numbers.push(comment.number),
            None => groups.push((comment.review, vec![comment.number])),
        }
    }

    groups
}

/// The comment `comment`, placed in its review's latest version, printed as one
/// JSON record.
fn show(ctx: &Context<'_>, comment: CommentRef) -> Result<Output, String> {
    Ok(Output {
        text: record(&ctx.store, comment)?,
        json: true,
        ..Output::default()
    })
}

/// The comment `comment`, placed in its review's latest version, as JSON text.
pub(crate) fn record(store: &Store, comment: CommentRef) -> Result<String, String> {
    let review = store.read(comment.review)?;
    let shown = cmd::version(comment.review, &review, None)?;
    let found = review
        .comment(comment.number)
        .map_err(|refusal| refusal.message(comment.review))?;
    let texts = load(
        &store.blobs(comment.review),
        view::blobs_of(&review, found, shown),
    )?;

    to_json(&view::comment(
        comment.review,
        &review,
        found,
        shown,
        &texts,
    ))
}
