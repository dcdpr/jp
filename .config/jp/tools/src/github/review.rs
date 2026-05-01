use base64::{Engine as _, prelude::BASE64_STANDARD};
use indoc::formatdoc;
use jp_github::models::pulls::{DraftReviewComment, ReviewState, Side};
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

    let comment = DraftReviewComment {
        path: path.clone(),
        body: body.clone(),
        line,
        side: Some(resolved_side),
        start_line,
        start_side: resolved_start_side,
    };

    let review_node_id = ensure_pending_review(pull_number).await?;

    jp_github::instance()
        .pulls(ORG, REPO)
        .add_review_thread(&review_node_id, &comment)
        .await
        .map_err(|e| handle_404(e, format!("Pull #{pull_number} not found in {ORG}/{REPO}")))?;

    let location = format_location(&path, line, start_line, resolved_side, resolved_start_side);
    Ok(format!("Comment queued on PR #{pull_number} at {location}.").into())
}

/// Find the current user's pending review on the PR, or lazily create an
/// empty one. Returns the review's GraphQL `node_id`.
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
    let side_str = match side {
        Side::Right => "RIGHT",
        Side::Left => "LEFT",
    };

    match (start_line, start_side) {
        (Some(start), Some(start_side_v)) if start_side_v != side => {
            let start_side_str = match start_side_v {
                Side::Right => "RIGHT",
                Side::Left => "LEFT",
            };
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

        {highlighted}

        {body_rendered}
        "
    )
}

/// Fetch a few lines of context around the commented line(s).
///
/// For `RIGHT` side, fetch the file at the PR's head SHA. For `LEFT` side,
/// fetch the base. Falls back to an error message if any step fails.
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

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;
