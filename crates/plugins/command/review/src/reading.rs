//! CLI commands recording what the reviewer approved and read: `approve`,
//! `unapprove`, and `read-until`.
//!
//! Like the comment commands, `--at` is read against the working tree, which is
//! recorded as a new version first when it changed since the latest.

use doc::author;
use jp_review_api::{self as wire, Unapproved, Unread};

use crate::{
    Output, anchor,
    anchor::Target,
    cmd::{self, Context, json, load},
    comments,
    id::ReviewId,
    paths,
    progress::ReadUntil,
    render,
    store::{Store, now},
    view,
};

/// Approve the range `at` names in the review `id`.
pub(crate) fn approve(ctx: &Context<'_>, id: ReviewId, at: &str) -> Result<Output, String> {
    let author = author::resolve(&ctx.root, ctx.config)?;
    let made = now();

    let number = ctx.store.update(id, |review, blobs| {
        review.writable().map_err(|refusal| refusal.message(id))?;
        let anchor = comments::anchor_at(ctx, review, blobs, Some(at), made)?;
        review
            .progress
            .approve(anchor, author, made)
            .map_err(|refusal| refusal.message(id))
    })?;

    let (approval, shown) = approval(&ctx.store, id, number)?;
    if ctx.format.is_json() {
        return json(&approval);
    }

    Ok(format!(
        "Approved {}, as approval {number}.\n",
        render::place(&approval.anchor, shown)
    )
    .into())
}

/// Take back approval `number` of the review `id`.
pub(crate) fn unapprove(ctx: &Context<'_>, id: ReviewId, number: u32) -> Result<Output, String> {
    ctx.store.update(id, |review, _| {
        review
            .writable()
            .and_then(|()| review.progress.unapprove(number))
            .map_err(|refusal| refusal.message(id))
    })?;

    if ctx.format.is_json() {
        return json(&Unapproved {
            review: id.to_string(),
            approval: number,
        });
    }

    Ok(format!("Took back approval {number}.\n").into())
}

/// Record that the file `at` names in the review `id` was read through the last
/// line `at` names.
pub(crate) fn read_until(ctx: &Context<'_>, id: ReviewId, at: &str) -> Result<Output, String> {
    let author = author::resolve(&ctx.root, ctx.config)?;
    let made = now();

    let path = ctx.store.update(id, |review, blobs| {
        review.writable().map_err(|refusal| refusal.message(id))?;
        let anchor = comments::anchor_at(ctx, review, blobs, Some(at), made)?;
        let Target::Range(path, range) = anchor.target else {
            return Err(format!(
                "`{at}` names no line; name the last line read, as in `docs/a.md:120`."
            ));
        };
        review.progress.read(path.clone(), ReadUntil {
            version: anchor.version,
            line: range.last_line(),
            author,
            created_at: made,
        });

        Ok(path)
    })?;

    let (read, shown) = read(&ctx.store, id, &path)?;
    if ctx.format.is_json() {
        return json(&read);
    }

    Ok(format!("Read {}.\n", render::read_through(&read, shown)).into())
}

/// Forget how far the file `at` names in the review `id` was read.
pub(crate) fn unread(ctx: &Context<'_>, id: ReviewId, at: &str) -> Result<Output, String> {
    let (root, cwd) = ctx.bases();

    let (path, cleared) = ctx.store.update(id, |review, _| {
        let (path, _) = anchor::place(at, &review.paths, |typed| {
            paths::tracked(&root, &cwd, &review.paths, typed)
        })?;
        review.writable().map_err(|refusal| refusal.message(id))?;
        let cleared = review.progress.unread(&path);

        Ok::<_, String>((path, cleared))
    })?;

    if ctx.format.is_json() {
        return json(&Unread {
            review: id.to_string(),
            path,
            cleared,
        });
    }

    Ok(if cleared {
        format!("Forgot how far {path} was read.\n")
    } else {
        format!("{id} doesn't say how far {path} was read.\n")
    }
    .into())
}

/// Approval `number` of the review `id`, placed in the review's latest version,
/// and that version's number.
pub(crate) fn approval(
    store: &Store,
    id: ReviewId,
    number: u32,
) -> Result<(wire::Approval, u32), String> {
    let review = store.read(id)?;
    let shown = cmd::version(id, &review, None)?;
    let found = review
        .progress
        .approvals
        .iter()
        .find(|approval| approval.number == number)
        .ok_or_else(|| format!("{id} has no approval {number}."))?;
    let texts = load(&store.blobs(id), view::progress_blobs(&review, shown))?;

    Ok((view::approval(&review, found, shown, &texts), shown))
}

/// How far `path` was read in the review `id`, placed in the review's latest
/// version, and that version's number.
pub(crate) fn read(
    store: &Store,
    id: ReviewId,
    path: &str,
) -> Result<(wire::ReadUntil, u32), String> {
    let review = store.read(id)?;
    let shown = cmd::version(id, &review, None)?;
    let found = review
        .progress
        .read_until
        .get(path)
        .ok_or_else(|| format!("{id} doesn't say how far {path} was read."))?;
    let texts = load(&store.blobs(id), view::progress_blobs(&review, shown))?;

    Ok((view::read_until(&review, path, found, shown, &texts), shown))
}
