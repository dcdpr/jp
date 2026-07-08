use base64::{Engine as _, prelude::BASE64_STANDARD};
use indoc::formatdoc;
use jp_github::models::pulls::{DraftReviewComment, ReviewComment, ReviewState, Side};
use jp_md::format::Formatter;

use super::auth;
use crate::{
    Context, Result,
    github::{ORG, REPO, handle_404},
    util::{ToolResult, error},
};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn github_pr_review_add_comment(
    ctx: Context,
    pull_number: u64,
    path: String,
    line: u64,
    body: String,
    side: Option<Side>,
    start_line: Option<u64>,
    start_side: Option<Side>,
) -> ToolResult {
    // Silently round 0 up to 1 — the LLM occasionally sends a 0 when it
    // means "the first line"; rejecting forces a re-call for no real gain.
    let line = line.max(1);
    let start_line = start_line.map(|v| v.max(1));

    if ctx.action.is_format_arguments() {
        return Ok(format_for_approval(
            pull_number,
            &path,
            line,
            &body,
            side,
            start_line,
            start_side,
        )
        .await
        .into());
    }

    auth().await?;

    if path.trim().is_empty() {
        return error("`path` must not be empty");
    }
    if body.trim().is_empty() {
        return error("`body` must not be empty");
    }
    // Fail fast on an inverted range. Without this, the execute path
    // would create an empty pending review via `ensure_pending_review`
    // and only then have GitHub reject the GraphQL mutation — leaving an
    // empty draft on the PR for the user to clean up.
    if let Some(start) = start_line
        && start > line
    {
        return error(format!(
            "`start_line` ({start}) must be less than or equal to `line` ({line})"
        ));
    }

    let resolved_side = side.unwrap_or(Side::Right);
    // For multi-line ranges, default `start_side` to the resolved `side`.
    // This matches the tool contract: omitted `start_side` follows `side`.
    // Without this, the GraphQL payload omits `startSide` and relies on
    // GitHub's default, while the local preview shows the chosen `side`
    // — producing a subtly inconsistent rendering.
    let resolved_start_side = if start_line.is_some() {
        Some(start_side.unwrap_or(resolved_side))
    } else {
        None
    };

    // Validate the anchor against the PR's diff before creating anything.
    //
    // GitHub's `addPullRequestReviewThread` mutation accepts anchors outside
    // the diff without an error, creating a thread the review UI never
    // renders — the comment silently vanishes. Rejecting here (before
    // `ensure_pending_review`, for the same reason as the range check above)
    // turns that silent loss into an actionable error.
    match fetch_file_diff(pull_number, &path).await? {
        FileDiff::NotInDiff => {
            return error(format!(
                "`{path}` is not among the files changed by PR #{pull_number}; review comments \
                 can only anchor to lines in the PR's diff. Call `github_pr_diff` to list the \
                 changed files."
            ));
        }
        // No textual patch (binary or oversized file): fail open and rely on
        // the post-mutation anchor check below.
        FileDiff::Unverifiable => {}
        FileDiff::Ranges(ranges) => {
            if let Err(msg) = check_anchor(
                &ranges,
                &path,
                line,
                start_line,
                resolved_side,
                resolved_start_side,
            ) {
                return error(msg);
            }
        }
    }

    let comment = DraftReviewComment {
        path: path.clone(),
        body: body.clone(),
        line,
        side: Some(resolved_side),
        start_line,
        start_side: resolved_start_side,
    };

    let review_node_id = ensure_pending_review(pull_number).await?;

    let thread = jp_github::instance()
        .pulls(ORG, REPO)
        .add_review_thread(&review_node_id, &comment)
        .await
        .map_err(|e| handle_404(e, format!("Pull #{pull_number} not found in {ORG}/{REPO}")))?;

    let location = format_location(&path, line, start_line, resolved_side, resolved_start_side);

    // Safety net behind the pre-validation above (a patch-less file, or a
    // head that moved between validation and posting): GitHub reports the
    // anchor it actually resolved, and a `None` line means the thread exists
    // but the review UI will never render it.
    if thread.line.is_none() {
        return error(format!(
            "GitHub created review thread {id} on PR #{pull_number} at {location}, but could not \
             anchor it to the current diff; the comment will NOT be visible in the review. \
             Re-anchor it to a line that is part of the PR's diff.",
            id = thread.id,
        ));
    }

    Ok(format!(
        "Comment queued on PR #{pull_number} at {location} (thread {id}).",
        id = thread.id
    )
    .into())
}

/// Find the current user's pending review on the PR, or lazily create an empty
/// one.
/// Returns the review's GraphQL `node_id`.
async fn ensure_pending_review(pull_number: u64) -> Result<String> {
    let me = jp_github::instance().current().user().await?;

    let page = jp_github::instance()
        .pulls(ORG, REPO)
        .list_reviews(pull_number)
        .await
        .map_err(|e| handle_404(e, format!("Pull #{pull_number} not found in {ORG}/{REPO}")))?;

    let reviews = jp_github::instance().all_pages(page).await?;

    let existing = reviews.into_iter().find(|r| {
        r.state == ReviewState::Pending && r.user.as_ref().is_some_and(|u| u.login == me.login)
    });

    if let Some(review) = existing {
        if review.node_id.is_empty() {
            return Err("existing pending review is missing node_id; cannot append".into());
        }
        return Ok(review.node_id);
    }

    let review = jp_github::instance()
        .pulls(ORG, REPO)
        .create_review(pull_number)
        .send()
        .await
        .map_err(|e| handle_404(e, format!("Pull #{pull_number} not found in {ORG}/{REPO}")))?;

    if review.node_id.is_empty() {
        return Err("created review has no node_id; cannot append further comments".into());
    }

    Ok(review.node_id)
}

fn format_location(
    path: &str,
    line: u64,
    start_line: Option<u64>,
    side: Side,
    start_side: Option<Side>,
) -> String {
    let side_str = side_str(side);

    match (start_line, start_side) {
        (Some(start), Some(start_side_v)) if start_side_v != side => {
            let start_side_str = self::side_str(start_side_v);
            format!("{path}:{start}({start_side_str})-{line}({side_str})")
        }
        (Some(start), _) => format!("{path}:{start}-{line} ({side_str})"),
        _ => format!("{path}:{line} ({side_str})"),
    }
}

async fn format_for_approval(
    pull_number: u64,
    path: &str,
    line: u64,
    body: &str,
    side: Option<Side>,
    start_line: Option<u64>,
    start_side: Option<Side>,
) -> String {
    let resolved_side = side.unwrap_or(Side::Right);
    // Mirror the execute path: an omitted `start_side` defaults to the
    // resolved `side` so the previewed range matches what will be sent.
    let resolved_start_side = if start_line.is_some() {
        Some(start_side.unwrap_or(resolved_side))
    } else {
        None
    };
    let location = format_location(path, line, start_line, resolved_side, resolved_start_side);

    let snippet = match fetch_snippet(pull_number, path, line, start_line, resolved_side).await {
        Ok(text) => text,
        Err(e) => format!("(snippet unavailable: {e})"),
    };

    // Warn the approver when the anchor isn't commentable: the snippet is
    // rendered from the full file, so an out-of-diff anchor otherwise looks
    // perfectly plausible. Fails quiet — the preview must not break over the
    // extra API call, and the execute path re-validates with a hard error.
    let warning = match fetch_file_diff(pull_number, path).await {
        Ok(FileDiff::NotInDiff) => Some(format!(
            "\u{26a0} `{path}` is not part of this PR's diff; posting will be rejected."
        )),
        Ok(FileDiff::Ranges(ranges)) => check_anchor(
            &ranges,
            path,
            line,
            start_line,
            resolved_side,
            resolved_start_side,
        )
        .err()
        .map(|msg| format!("\u{26a0} {msg}")),
        Ok(FileDiff::Unverifiable) | Err(_) => None,
    };
    let warning = warning.map_or_else(String::new, |w| format!("\n{w}\n"));

    let lang = crate::util::lang_from_path(path);
    let block = format!("`````{lang}\n{snippet}\n`````");
    let highlighted = Formatter::new().format_terminal(&block).unwrap_or(block);

    // Render the body as markdown so backticks become syntax-highlighted
    // inline code, lists render as lists, and so on.
    let body_rendered = Formatter::new()
        .format_terminal(body)
        .unwrap_or_else(|_| body.to_owned());

    formatdoc!(
        "
        PR #{pull_number} \u{2014} {location}
        {warning}
        {highlighted}

        {body_rendered}
        "
    )
}

/// Fetch a few lines of context around the commented line(s).
///
/// For `RIGHT` side, fetch the file at the PR's head SHA.
/// For `LEFT` side, fetch the base.
/// Falls back to an error message if any step fails.
async fn fetch_snippet(
    pull_number: u64,
    path: &str,
    line: u64,
    start_line: Option<u64>,
    side: Side,
) -> Result<String> {
    auth().await?;

    let pr = jp_github::instance()
        .pulls(ORG, REPO)
        .get(pull_number)
        .await
        .map_err(|e| handle_404(e, format!("Pull #{pull_number} not found in {ORG}/{REPO}")))?;

    let git_ref = match side {
        Side::Right => pr.head.as_ref(),
        Side::Left => pr.base.as_ref(),
    }
    .ok_or("PR has no head/base reference")?;

    let items = jp_github::instance()
        .repos(ORG, REPO)
        .get_content()
        .path(path)
        .r#ref(&git_ref.sha)
        .send()
        .await
        .map_err(|e| {
            handle_404(
                e,
                format!(
                    "File {path} not found at {} ({})",
                    &git_ref.sha[..7],
                    match side {
                        Side::Right => "head",
                        Side::Left => "base",
                    }
                ),
            )
        })?
        .take_items();

    let item = items.into_iter().next().ok_or("file not found")?;
    let raw = item
        .content
        .ok_or("file has no content (likely a directory)")?;
    let decoded = match item.encoding.as_deref() {
        Some("base64") => {
            let bytes = BASE64_STANDARD
                .decode(
                    raw.chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>(),
                )
                .map_err(|e| format!("base64 decode failed: {e}"))?;
            String::from_utf8(bytes).map_err(|e| format!("file is not UTF-8: {e}"))?
        }
        _ => raw,
    };

    Ok(extract_window(&decoded, line, start_line))
}

/// Pull a window of source around the commented line(s), with a few lines of
/// context, marking the commented range with a `>` gutter.
fn extract_window(content: &str, line: u64, start_line: Option<u64>) -> String {
    const CONTEXT: usize = 3;

    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return "(empty file)".to_owned();
    }

    let line_idx = usize::try_from(line)
        .unwrap_or(usize::MAX)
        .saturating_sub(1)
        .min(lines.len() - 1);
    let start_idx = start_line.map_or(line_idx, |s| {
        usize::try_from(s)
            .unwrap_or(usize::MAX)
            .saturating_sub(1)
            .min(line_idx)
    });

    let window_start = start_idx.saturating_sub(CONTEXT);
    let window_end = (line_idx + 1 + CONTEXT).min(lines.len());

    let width = (window_end).to_string().len();

    lines[window_start..window_end]
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let n = window_start + i + 1;
            let in_range = (start_idx + 1..=line_idx + 1).contains(&n);
            let marker = if in_range { ">" } else { " " };
            format!("{marker} {n:>width$}  {text}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Files per page when walking a PR's changed files (the GitHub API max for
/// `/pulls/{N}/files`).
const FILES_PER_PAGE: u8 = 100;

/// Commentable line ranges for one file of a PR diff.
///
/// GitHub anchors review comments to diff hunks: a `RIGHT` anchor must be a
/// new-file line covered by a hunk, a `LEFT` anchor an old-file line (context
/// lines count on both sides).
/// The `addPullRequestReviewThread` mutation accepts anchors outside those
/// ranges without an error — it creates a thread the review UI never renders,
/// silently losing the comment — so anchors are validated here before anything
/// is posted.
#[derive(Debug, Default, PartialEq, Eq)]
struct DiffRanges {
    /// Inclusive old-file line ranges (`LEFT` side).
    left: Vec<(u64, u64)>,
    /// Inclusive new-file line ranges (`RIGHT` side).
    right: Vec<(u64, u64)>,
}

impl DiffRanges {
    /// Parse the hunk headers (`@@ -old,count +new,count @@`) out of a REST
    /// `patch` blob.
    ///
    /// Content lines always start with ` `, `+`, or `-`, so scanning for the `
    /// @@  ` prefix cannot match inside hunk bodies.
    fn from_patch(patch: &str) -> Self {
        let mut ranges = Self::default();

        for line in patch.lines() {
            let Some(rest) = line.strip_prefix("@@ ") else {
                continue;
            };
            let mut parts = rest.split_whitespace();
            let old = parts
                .next()
                .and_then(|p| p.strip_prefix('-'))
                .and_then(parse_start_count);
            let new = parts
                .next()
                .and_then(|p| p.strip_prefix('+'))
                .and_then(parse_start_count);
            let (Some((old_start, old_count)), Some((new_start, new_count))) = (old, new) else {
                continue;
            };

            // A zero count (pure addition/removal) contributes no
            // commentable lines on that side.
            if old_count > 0 {
                ranges.left.push((old_start, old_start + old_count - 1));
            }
            if new_count > 0 {
                ranges.right.push((new_start, new_start + new_count - 1));
            }
        }

        ranges
    }

    fn side(&self, side: Side) -> &[(u64, u64)] {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    fn contains(&self, side: Side, line: u64) -> bool {
        self.side(side)
            .iter()
            .any(|&(s, e)| (s..=e).contains(&line))
    }

    /// The commentable line closest to `line` on `side`, if any.
    fn nearest(&self, side: Side, line: u64) -> Option<u64> {
        self.side(side)
            .iter()
            .map(|&(s, e)| line.clamp(s, e))
            .min_by_key(|c| c.abs_diff(line))
    }

    /// Human-readable range list, e.g. `61-92, 361-401`.
    fn describe(&self, side: Side) -> String {
        let ranges = self.side(side);
        if ranges.is_empty() {
            return "(none)".to_owned();
        }

        ranges
            .iter()
            .map(|&(s, e)| {
                if s == e {
                    s.to_string()
                } else {
                    format!("{s}-{e}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Parse a hunk-header `start,count` section (bare `start` means count 1).
fn parse_start_count(s: &str) -> Option<(u64, u64)> {
    let mut it = s.split(',');
    let start = it.next()?.parse().ok()?;
    let count = it.next().map_or(Some(1), |c| c.parse().ok())?;
    Some((start, count))
}

/// Validate each end of the comment range against the diff's commentable
/// ranges.
///
/// The rejection message includes the valid ranges and the nearest commentable
/// line, so the caller can re-anchor in a single follow-up call.
fn check_anchor(
    ranges: &DiffRanges,
    path: &str,
    line: u64,
    start_line: Option<u64>,
    side: Side,
    start_side: Option<Side>,
) -> std::result::Result<(), String> {
    let mut anchors = vec![("line", line, side)];
    if let Some(start) = start_line {
        anchors.push(("start_line", start, start_side.unwrap_or(side)));
    }

    for (name, value, anchor_side) in anchors {
        if ranges.contains(anchor_side, value) {
            continue;
        }

        let side_name = side_str(anchor_side);
        let nearest = ranges
            .nearest(anchor_side, value)
            .map_or_else(String::new, |n| format!(" Nearest commentable line: {n}."));
        return Err(format!(
            "`{name}` ({value}, {side_name}) is not part of the PR's diff for `{path}`. GitHub \
             accepts such anchors but never displays the comment. Commentable {side_name} lines \
             (diff hunks including context): {ranges_desc}.{nearest}",
            ranges_desc = ranges.describe(anchor_side),
        ));
    }

    Ok(())
}

/// Result of resolving a file path against a PR's changed files.
enum FileDiff {
    /// The file is not part of the PR diff at all.
    NotInDiff,
    /// The file is in the diff, but GitHub returned no textual patch for it
    /// (binary or oversized); anchors cannot be verified up front.
    Unverifiable,
    /// Commentable ranges parsed from the file's patch.
    Ranges(DiffRanges),
}

/// Locate `path` among the PR's changed files and parse its commentable ranges.
///
/// Walks the paginated file list to the end before concluding the file is not
/// in the diff — PRs can exceed one page.
///
/// Authenticates defensively (like [`fetch_snippet`]): the approval-preview
/// path calls this without a prior successful [`auth`], and an uninitialized
/// client panics rather than erroring.
async fn fetch_file_diff(pull_number: u64, path: &str) -> Result<FileDiff> {
    auth().await?;

    let mut page = 1;

    loop {
        let entries = jp_github::instance()
            .pulls(ORG, REPO)
            .list_files(pull_number)
            .page(page)
            .per_page(FILES_PER_PAGE)
            .send()
            .await
            .map_err(|e| handle_404(e, format!("Pull #{pull_number} not found in {ORG}/{REPO}")))?;
        let full_page = entries.len() == usize::from(FILES_PER_PAGE);

        if let Some(entry) = entries.into_iter().find(|e| e.filename == path) {
            return Ok(entry
                .patch
                .as_deref()
                .map_or(FileDiff::Unverifiable, |patch| {
                    FileDiff::Ranges(DiffRanges::from_patch(patch))
                }));
        }
        if !full_page {
            return Ok(FileDiff::NotInDiff);
        }
        page += 1;
    }
}

const fn side_str(side: Side) -> &'static str {
    match side {
        Side::Right => "RIGHT",
        Side::Left => "LEFT",
    }
}

pub(crate) async fn github_pr_review_add_reply(
    ctx: Context,
    pull_number: u64,
    comment_id: u64,
    body: String,
) -> ToolResult {
    if ctx.action.is_format_arguments() {
        return Ok(format_reply_for_approval(pull_number, comment_id, &body)
            .await
            .into());
    }

    auth().await?;

    if body.trim().is_empty() {
        return error("`body` must not be empty");
    }

    let review_node_id = ensure_pending_review(pull_number).await?;
    let thread_node_id = jp_github::instance()
        .pulls(ORG, REPO)
        .fetch_thread_id_for_comment(pull_number, comment_id)
        .await
        .map_err(|e| {
            handle_404(
                e,
                format!("Comment id={comment_id} not found on pull #{pull_number}"),
            )
        })?;

    jp_github::instance()
        .pulls(ORG, REPO)
        .add_review_thread_reply(&thread_node_id, &review_node_id, &body)
        .await
        .map_err(|e| handle_404(e, format!("Pull #{pull_number} not found in {ORG}/{REPO}")))?;

    Ok(format!("Reply queued on PR #{pull_number} (in reply to comment id={comment_id}).").into())
}

async fn format_reply_for_approval(pull_number: u64, comment_id: u64, body: &str) -> String {
    // Re-fetch the parent comment instead of trusting whatever the LLM
    // saw via the attachment: the user is approving the post, the bot
    // saw a snapshot, and the human deserves a fresh view of what's
    // actually being replied to.
    match fetch_parent_comment(pull_number, comment_id).await {
        Ok(parent) => {
            let author = parent
                .user
                .as_ref()
                .map_or("(unknown)", |u| u.login.as_str());
            let location = format_parent_location(&parent);
            let parent_quoted = parent
                .body
                .lines()
                .map(|l| format!("> {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            let parent_rendered = Formatter::new()
                .format_terminal(&parent_quoted)
                .unwrap_or(parent_quoted);
            let body_rendered = Formatter::new()
                .format_terminal(body)
                .unwrap_or_else(|_| body.to_owned());

            formatdoc!(
                "
                PR #{pull_number} \u{2014} replying to {author} on {location}

                {parent_rendered}

                {body_rendered}
                "
            )
        }
        Err(e) => {
            // Fail soft: still surface the proposed body so the user can
            // approve or reject. Naming the failure helps them spot a
            // stale `comment_id` before posting.
            let body_rendered = Formatter::new()
                .format_terminal(body)
                .unwrap_or_else(|_| body.to_owned());
            formatdoc!(
                "
                PR #{pull_number} \u{2014} replying to comment id={comment_id} (parent preview \
                 unavailable: {e})

                {body_rendered}
                "
            )
        }
    }
}

async fn fetch_parent_comment(pull_number: u64, comment_id: u64) -> Result<ReviewComment> {
    auth().await?;
    let comments = jp_github::instance()
        .pulls(ORG, REPO)
        .fetch_review_comments(pull_number)
        .await?;
    comments
        .into_iter()
        .find(|c| c.id == comment_id)
        .ok_or_else(|| format!("comment id={comment_id} not found on pull #{pull_number}").into())
}

fn format_parent_location(c: &ReviewComment) -> String {
    let line = c.line.or(c.original_line).unwrap_or(0);
    let side = c.side.or(c.original_side).map_or("RIGHT", |s| match s {
        Side::Right => "RIGHT",
        Side::Left => "LEFT",
    });
    format!("{}:{line} ({side})", c.path)
}

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;
