//! GitHub attachment handler for `gh://` URIs.
//!
//! Currently supports two resource types:
//!
//! - `gh://{owner}/{repo}/pull/{number}/diff` — the PR's title, description,
//!   changed-files summary, and unified diff, attached as one text resource.
//! - `gh://{owner}/{repo}/pull/{number}/reviews` — the PR's review summaries
//!   and inline comments (including the authenticated user's pending
//!   drafts), attached as one markdown resource.
//!
//! Both resource types support a shortform that resolves owner/repo to the
//! project-rooted defaults (`dcdpr/jp`):
//!
//! - `gh:pull/{number}/diff`
//! - `gh:pull/{number}/reviews`
//!
//! The handler uses `JP_GITHUB_TOKEN` (or `GITHUB_TOKEN`) from the environment
//! for authentication, via [`jp_github::Octocrab`]. No global state is held —
//! each fetch builds its own client.
//!
//! ## Filtering
//!
//! ### Diff (`/diff`)
//!
//! The unified diff is filtered by file path before being attached, to keep
//! generated and lockfile noise out of the LLM context. The default exclusion
//! list is opinionated; override via query parameters:
//!
//! - `?exclude=glob1,glob2` — adds patterns on top of the defaults.
//! - `?no_defaults=true` — drops the built-in defaults; only `?exclude` patterns
//!   apply.
//!
//! ### Reviews (`/reviews`)
//!
//! Outdated comments — those GitHub has marked as no longer matching the
//! current diff — are skipped by default. The header reports how many were
//! hidden. Comments are fetched via GraphQL
//! (`pullRequest.reviewThreads`), so each carries the canonical `outdated`
//! flag and a reliable line / side anchor even when REST returns null.
//!
//! - `?include_outdated=true` — include outdated comments. They render with
//!   their original anchor (`original_line` etc.) and an `(outdated)` marker.

use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
};

use async_trait::async_trait;
use camino::Utf8Path;
use glob::Pattern;
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, distributed_slice, linkme, typetag,
};
use jp_github::{
    Octocrab,
    models::pulls::{PullRequest, Review, ReviewComment, Side},
};
use jp_mcp::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};
use url::Url;

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static GH_HANDLER: fn() -> BoxedHandler = handler;

fn handler() -> BoxedHandler {
    (Box::new(GithubAttachment::default()) as Box<dyn Handler>).into()
}

const DEFAULT_EXCLUDES: &[&str] = &[
    "**/*.snap",
    "**/snapshots/**",
    "**/fixtures/**",
    "Cargo.lock",
    "**/Cargo.lock",
    "package-lock.json",
    "**/package-lock.json",
    "yarn.lock",
    "**/yarn.lock",
    "pnpm-lock.yaml",
    "**/pnpm-lock.yaml",
    "**/*.min.js",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct GithubAttachment {
    urls: BTreeSet<Url>,
}

#[typetag::serde(name = "github")]
#[async_trait]
impl Handler for GithubAttachment {
    fn scheme(&self) -> &'static str {
        "gh"
    }

    async fn add(
        &mut self,
        uri: &Url,
        _cwd: &Utf8Path,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Validate the URI shape now so a typo fails fast at attach time
        // rather than at the next conversation turn.
        parse_uri(uri)?;
        self.urls.insert(uri.clone());
        Ok(())
    }

    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.urls.remove(uri);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
        Ok(self.urls.iter().cloned().collect())
    }

    async fn get(
        &self,
        _: &Utf8Path,
        _: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        debug!(id = "gh", "Fetching GitHub attachments.");

        let mut attachments = Vec::with_capacity(self.urls.len());
        for url in &self.urls {
            match fetch(url).await {
                Ok(att) => attachments.push(att),
                Err(error) => {
                    // Surface the failure instead of silently dropping the
                    // attachment. Workflows like `just pr-review` rely on
                    // `gh:pull/N/diff` being present — a silent skip would
                    // let the LLM produce a "review" with no diff in
                    // context, with the prompt still claiming the diff is
                    // attached. Better to fail loudly so the user can
                    // investigate (token, network, rate limit) and retry.
                    error!(uri = %url, %error, "Failed to fetch GitHub attachment.");
                    return Err(format!("failed to fetch GitHub attachment {url}: {error}").into());
                }
            }
        }
        Ok(attachments)
    }
}

/// What kind of GitHub resource is referenced by a `gh://` URI.
#[derive(Debug, Clone)]
struct ParsedUri {
    owner: String,
    repo: String,
    kind: ResourceKind,
    excludes: Vec<String>,
    no_defaults: bool,
    include_outdated: bool,
}

#[derive(Debug, Clone)]
enum ResourceKind {
    PullDiff { number: u64 },
    PullReviews { number: u64 },
}

/// Owner/repo used by the shortform `gh:pull/N/diff`.
///
/// The shortform is project-rooted: anyone using `gh:pull/N/diff` in this
/// workspace means "pull #N of the JP project." Hardcoded to match the
/// rest of the project-specific tooling that already targets `dcdpr/jp`.
const SHORTFORM_OWNER: &str = "dcdpr";
const SHORTFORM_REPO: &str = "jp";

fn parse_uri(uri: &Url) -> Result<ParsedUri, Box<dyn Error + Send + Sync>> {
    if uri.scheme() != "gh" {
        return Err(format!("expected `gh` scheme, got `{}`", uri.scheme()).into());
    }

    // `gh://owner/repo/...` parses with a host; `gh:pull/...` is opaque
    // (no `//`) and has no host — in that case, fall back to the
    // project-rooted defaults.
    let (owner, segments) = if let Some(host) = uri.host_str() {
        let segments: Vec<&str> = uri
            .path_segments()
            .ok_or("missing path in gh URI")?
            .filter(|s| !s.is_empty())
            .collect();
        (host.to_owned(), segments)
    } else {
        // Opaque form: `gh:pull/N/diff`. The whole tail is in `path()`.
        let segments: Vec<&str> = uri.path().split('/').filter(|s| !s.is_empty()).collect();
        (SHORTFORM_OWNER.to_owned(), segments)
    };

    // Canonical: REPO/pull/NUMBER/{diff|reviews}
    // Shortform: pull/NUMBER/{diff|reviews}
    let (repo, kind) = match segments.as_slice() {
        [repo, "pull", number, "diff"] => {
            let n: u64 = number
                .parse()
                .map_err(|_| format!("invalid PR number `{number}`"))?;
            ((*repo).to_owned(), ResourceKind::PullDiff { number: n })
        }
        ["pull", number, "diff"] => {
            let n: u64 = number
                .parse()
                .map_err(|_| format!("invalid PR number `{number}`"))?;
            (SHORTFORM_REPO.to_owned(), ResourceKind::PullDiff {
                number: n,
            })
        }
        [repo, "pull", number, "reviews"] => {
            let n: u64 = number
                .parse()
                .map_err(|_| format!("invalid PR number `{number}`"))?;
            ((*repo).to_owned(), ResourceKind::PullReviews { number: n })
        }
        ["pull", number, "reviews"] => {
            let n: u64 = number
                .parse()
                .map_err(|_| format!("invalid PR number `{number}`"))?;
            (SHORTFORM_REPO.to_owned(), ResourceKind::PullReviews {
                number: n,
            })
        }
        _ => {
            return Err(format!(
                "unsupported gh URI shape; expected one of \
                 `gh://OWNER/REPO/pull/N/{{diff|reviews}}` or `gh:pull/N/{{diff|reviews}}`, got \
                 `{uri}`"
            )
            .into());
        }
    };

    let mut excludes = Vec::new();
    let mut no_defaults = false;
    let mut include_outdated = false;
    for (key, value) in uri.query_pairs() {
        match &*key {
            "exclude" => {
                for pat in value.split(',') {
                    let p = pat.trim();
                    if !p.is_empty() {
                        excludes.push(p.to_owned());
                    }
                }
            }
            "no_defaults" => {
                no_defaults = matches!(&*value, "true" | "1" | "yes");
            }
            "include_outdated" => {
                include_outdated = matches!(&*value, "true" | "1" | "yes");
            }
            other => {
                return Err(format!("unknown query param `{other}` in gh URI").into());
            }
        }
    }

    Ok(ParsedUri {
        owner,
        repo,
        kind,
        excludes,
        no_defaults,
        include_outdated,
    })
}

async fn fetch(uri: &Url) -> Result<Attachment, Box<dyn Error + Send + Sync>> {
    let parsed = parse_uri(uri)?;
    match &parsed.kind {
        ResourceKind::PullDiff { number } => fetch_pr_diff(uri, &parsed, *number).await,
        ResourceKind::PullReviews { number } => fetch_pr_reviews(uri, &parsed, *number).await,
    }
}

/// Build a fresh `Octocrab` client using `JP_GITHUB_TOKEN` (or `GITHUB_TOKEN`)
/// from the environment. The attachment handler runs in `jp_cli`'s main
/// process and does not share the global `jp_github::instance()` set up by
/// the tools subprocess, so we build a local client.
fn build_client() -> Result<Octocrab, Box<dyn Error + Send + Sync>> {
    let token = std::env::var("JP_GITHUB_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .ok();

    let mut builder = Octocrab::builder();
    if let Some(t) = token {
        builder = builder.personal_token(t);
    }
    builder.build().map_err(Into::into)
}

async fn fetch_pr_diff(
    uri: &Url,
    parsed: &ParsedUri,
    number: u64,
) -> Result<Attachment, Box<dyn Error + Send + Sync>> {
    let client = build_client()?;
    let pulls = client.pulls(&parsed.owner, &parsed.repo);

    // Metadata first (title, description, refs).
    let pr = pulls.get(number).await?;

    // Then the unified diff via custom Accept header.
    let diff_text = pulls.diff(number).await?;

    let patterns = compile_excludes(&parsed.excludes, parsed.no_defaults)?;
    let (filtered, included, excluded) = filter_diff(&diff_text, &patterns);

    let header = render_diff_header(uri, &pr, included, excluded, parsed);
    let body = format!("{header}\n\n{filtered}");

    Ok(Attachment::text(uri.to_string(), body))
}

async fn fetch_pr_reviews(
    uri: &Url,
    parsed: &ParsedUri,
    number: u64,
) -> Result<Attachment, Box<dyn Error + Send + Sync>> {
    let client = build_client()?;
    let pulls = client.pulls(&parsed.owner, &parsed.repo);

    // Review summaries via REST (state + body). GraphQL has the same data
    // but REST is enough here — we only need state info, no anchors.
    let reviews_page = pulls.list_reviews(number).await?;
    let reviews: Vec<Review> = client.all_pages(reviews_page).await?;

    // Inline comments come exclusively from GraphQL: it returns reliable
    // line/side anchors and the canonical `outdated` flag, even for
    // pending review comments where REST nulls those fields out.
    let mut comments = pulls.fetch_review_comments(number).await?;

    let outdated_hidden = apply_outdated_filter(parsed, &mut comments);
    let body = render_reviews(uri, number, &reviews, &comments, outdated_hidden);
    Ok(Attachment::text(uri.to_string(), body))
}

/// Drop outdated comments unless the URI opted into them.
///
/// Returns the number of comments hidden — zero when `include_outdated` is
/// set, otherwise the count that was removed. The caller passes that count
/// into the renderer so the header can surface it.
///
/// `outdated` is read directly from each comment, populated from GitHub's
/// canonical GraphQL `reviewThreads.isOutdated` flag at fetch time.
fn apply_outdated_filter(parsed: &ParsedUri, comments: &mut Vec<ReviewComment>) -> usize {
    let outdated_total = comments.iter().filter(|c| c.outdated).count();
    if parsed.include_outdated {
        0
    } else {
        comments.retain(|c| !c.outdated);
        outdated_total
    }
}

fn compile_excludes(
    user: &[String],
    no_defaults: bool,
) -> Result<Vec<Pattern>, Box<dyn Error + Send + Sync>> {
    let mut patterns = Vec::new();
    if !no_defaults {
        for p in DEFAULT_EXCLUDES {
            patterns.push(Pattern::new(p).map_err(|e| format!("invalid default pattern: {e}"))?);
        }
    }
    for p in user {
        patterns.push(Pattern::new(p).map_err(|e| format!("invalid exclude `{p}`: {e}"))?);
    }
    Ok(patterns)
}

/// Splits a unified diff into per-file sections, filters by path, and
/// returns the kept text plus counts of included/excluded files.
///
/// File boundary is the `diff --git a/PATH b/PATH` line. Anything before
/// the first such line is preserved as-is (rare; usually empty).
fn filter_diff(diff: &str, excludes: &[Pattern]) -> (String, usize, usize) {
    let mut included = 0_usize;
    let mut excluded = 0_usize;
    let mut out = String::with_capacity(diff.len());

    let mut current: Option<(String, String)> = None; // (path, accumulated_text)
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            // Flush previous section.
            if let Some((path, text)) = current.take() {
                if path_matches_any(&path, excludes) {
                    excluded += 1;
                } else {
                    out.push_str(&text);
                    if !text.ends_with('\n') {
                        out.push('\n');
                    }
                    included += 1;
                }
            }

            // New section.
            let path = rest.split(" b/").next().unwrap_or("").to_owned();
            let mut text = String::new();
            text.push_str(line);
            text.push('\n');
            current = Some((path, text));
        } else if let Some((_, text)) = current.as_mut() {
            text.push_str(line);
            text.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    if let Some((path, text)) = current {
        if path_matches_any(&path, excludes) {
            excluded += 1;
        } else {
            out.push_str(&text);
            included += 1;
        }
    }

    (out, included, excluded)
}

fn path_matches_any(path: &str, patterns: &[Pattern]) -> bool {
    patterns.iter().any(|p| p.matches(path))
}

fn render_diff_header(
    uri: &Url,
    pr: &PullRequest,
    included: usize,
    excluded: usize,
    parsed: &ParsedUri,
) -> String {
    let title = pr.title.as_deref().unwrap_or("(no title)");
    let author = pr.user.as_ref().map_or("(unknown)", |u| u.login.as_str());
    let html_url = pr
        .html_url
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    let head = pr
        .head
        .as_ref()
        .map_or_else(|| "(unknown)".to_owned(), format_git_ref);
    let base = pr
        .base
        .as_ref()
        .map_or_else(|| "(unknown)".to_owned(), format_git_ref);
    let state = if pr.merged_at.is_some() {
        "merged"
    } else if pr.closed_at.is_some() {
        "closed"
    } else {
        "open"
    };

    let body_section = match pr.body.as_deref().map(str::trim) {
        Some(b) if !b.is_empty() => format!("\n\n---\n\n{b}"),
        _ => String::new(),
    };

    let filter_line = if parsed.excludes.is_empty() && !parsed.no_defaults {
        format!("Files included: {included}, excluded by default filter: {excluded}.")
    } else if parsed.no_defaults {
        format!(
            "Files included: {included}, excluded by `?exclude` only: {excluded} (defaults \
             disabled)."
        )
    } else {
        format!("Files included: {included}, excluded by defaults + `?exclude`: {excluded}.")
    };

    format!(
        "PR #{number}: {title}\n\nSource: {uri}\nURL:    {html_url}\nAuthor: {author}\nState:  \
         {state}\nBase:   {base}\nHead:   {head}\n\n{filter_line}{body_section}",
        number = pr.number,
    )
}

fn format_git_ref(r: &jp_github::models::pulls::GitRef) -> String {
    let sha = if r.sha.len() >= 7 {
        &r.sha[..7]
    } else {
        &r.sha
    };
    match &r.ref_ {
        Some(ref_) => format!("{ref_} ({sha})"),
        None => sha.to_owned(),
    }
}

/// Render all reviews and inline comments into one markdown attachment.
///
/// Top-level layout:
///
/// 1. Header with counts.
/// 2. "Reviews" section: per-review summary line, sorted by `submitted_at`.
/// 3. "Inline comments by file" section: comments grouped by file, then by
///    anchor (line range + side). Replies nest under their parent.
#[allow(
    clippy::too_many_lines,
    reason = "linear render, splitting hurts readability"
)]
fn render_reviews(
    uri: &Url,
    pr_number: u64,
    reviews: &[Review],
    comments: &[ReviewComment],
    outdated_hidden: usize,
) -> String {
    let pending_count = reviews.iter().filter(|r| is_pending(r)).count();
    let submitted_count = reviews.len() - pending_count;

    let mut out = String::new();
    out.push_str(&format!(
        "PR #{pr_number} reviews: {submitted_count} submitted"
    ));
    if pending_count > 0 {
        out.push_str(&format!(", {pending_count} pending (yours)"));
    }
    if outdated_hidden > 0 {
        out.push_str(&format!(
            "\n{outdated_hidden} outdated comment(s) hidden; pass `?include_outdated=true` to \
             include them."
        ));
    }
    out.push_str(&format!("\n\nSource: {uri}\n"));

    if reviews.is_empty() && comments.is_empty() {
        out.push_str("\n---\n\nNo reviews or inline comments yet.\n");
        return out;
    }

    // Reviews section.
    out.push_str("\n---\n\nReviews:\n\n");
    if reviews.is_empty() {
        out.push_str("(no review summaries)\n");
    } else {
        let mut sorted: Vec<&Review> = reviews.iter().collect();
        // Sort by submission time, with id as a stable tiebreaker.
        sorted.sort_by_key(|r| r.id);
        for r in sorted {
            let user = if is_pending(r) {
                "you".to_owned()
            } else {
                r.user
                    .as_ref()
                    .map_or_else(|| "(unknown)".to_owned(), |u| u.login.clone())
            };
            let state = format_review_state(r);
            let body = r.body.as_deref().map(str::trim).filter(|s| !s.is_empty());
            match body {
                Some(b) if b.contains('\n') => {
                    out.push_str(&format!("- **{user}** ({state}):\n"));
                    for line in b.lines() {
                        out.push_str(&format!("  > {line}\n"));
                    }
                }
                Some(b) => {
                    out.push_str(&format!("- **{user}** ({state}): {b}\n"));
                }
                None => {
                    out.push_str(&format!("- **{user}** ({state}): (no summary)\n"));
                }
            }
        }
    }

    // Inline comments section.
    out.push_str("\n---\n\nInline comments by file:\n");
    if comments.is_empty() {
        out.push_str("\n(no inline comments)\n");
        return out;
    }

    // Map review id → review for state lookup when labeling each comment.
    let by_review: HashMap<u64, &Review> = reviews.iter().map(|r| (r.id, r)).collect();

    // Replies are flat in the API: each comment carries `in_reply_to_id`.
    // Group by parent_id so we can nest them under the top-level comment.
    let mut replies_by_parent: HashMap<u64, Vec<&ReviewComment>> = HashMap::new();
    for c in comments {
        if let Some(parent_id) = c.in_reply_to_id {
            replies_by_parent.entry(parent_id).or_default().push(c);
        }
    }

    // Top-level comments, sorted by (path, line, id) for stable rendering.
    let mut top_level: Vec<&ReviewComment> = comments
        .iter()
        .filter(|c| c.in_reply_to_id.is_none())
        .collect();
    top_level.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.line.unwrap_or(0).cmp(&b.line.unwrap_or(0)))
            .then(a.id.cmp(&b.id))
    });

    let mut current_path: Option<&str> = None;
    let mut current_anchor: Option<String> = None;

    for c in &top_level {
        if current_path != Some(c.path.as_str()) {
            current_path = Some(c.path.as_str());
            current_anchor = None;
            out.push_str(&format!("\n## {}\n", c.path));
        }

        let anchor = format_anchor(c);
        if current_anchor.as_deref() != Some(&anchor) {
            current_anchor = Some(anchor.clone());
            out.push_str(&format!("\n### {anchor}\n\n"));
        }

        render_comment(&mut out, c, &by_review, false);

        if let Some(replies) = replies_by_parent.get(&c.id) {
            let mut sorted: Vec<&&ReviewComment> = replies.iter().collect();
            sorted.sort_by_key(|r| r.id);
            for r in sorted {
                render_comment(&mut out, r, &by_review, true);
            }
        }
    }

    out
}

fn render_comment(
    out: &mut String,
    c: &ReviewComment,
    by_review: &std::collections::HashMap<u64, &Review>,
    is_reply: bool,
) {
    let parent_review = c.pull_request_review_id.and_then(|id| by_review.get(&id));
    let pending = parent_review.is_some_and(|r| is_pending(r));

    let user = if pending {
        "you".to_owned()
    } else {
        c.user
            .as_ref()
            .map_or_else(|| "(unknown)".to_owned(), |u| u.login.clone())
    };

    let label = match (is_reply, pending, parent_review) {
        (true, true, _) => "reply, pending".to_owned(),
        (true, false, Some(r)) => format!("reply, {}", format_review_state(r)),
        (true, false, None) => "reply".to_owned(),
        (false, true, _) => "pending".to_owned(),
        (false, false, Some(r)) => format_review_state(r),
        (false, false, None) => "submitted".to_owned(),
    };

    let bullet = if is_reply { "  -" } else { "-" };
    let body_lines: Vec<&str> = c.body.lines().collect();

    if body_lines.len() <= 1 {
        let body = c.body.trim();
        out.push_str(&format!("{bullet} **{user}** ({label}): {body}\n"));
    } else {
        out.push_str(&format!("{bullet} **{user}** ({label}):\n"));
        let indent = if is_reply { "    > " } else { "  > " };
        for line in &body_lines {
            out.push_str(&format!("{indent}{line}\n"));
        }
    }
}

/// Render a comment's anchor: `Line N (SIDE)` or `Lines A-B (SIDE)`,
/// occasionally with a mixed-side multi-line marker. Outdated comments fall
/// back to their `original_*` anchor and pick up an `(outdated)` suffix.
fn format_anchor(c: &ReviewComment) -> String {
    // Prefer live fields, fall back to historical (`original_*`) when the
    // live ones are missing — typically the case for outdated comments
    // where GitHub clears `line` but keeps `original_line` populated.
    let line = c.line.or(c.original_line);
    let start_line = c.start_line.or(c.original_start_line);
    let side = c.side.or(c.original_side).map_or("RIGHT", side_str);
    let start_side = c.start_side.or(c.original_start_side);

    let outdated_suffix = if c.outdated { ", outdated" } else { "" };

    match (start_line, line) {
        (Some(start), Some(end)) if start != end => match start_side.map(side_str) {
            Some(s) if s != side => {
                format!("Lines {start}({s})-{end}({side}{outdated_suffix})")
            }
            _ => format!("Lines {start}-{end} ({side}{outdated_suffix})"),
        },
        (_, Some(line)) => format!("Line {line} ({side}{outdated_suffix})"),
        _ => format!("(no anchor) on path {}", c.path),
    }
}

fn side_str(side: Side) -> &'static str {
    match side {
        Side::Right => "RIGHT",
        Side::Left => "LEFT",
    }
}

fn format_review_state(r: &Review) -> String {
    use jp_github::models::pulls::ReviewState;
    match r.state {
        ReviewState::Pending => "pending".to_owned(),
        ReviewState::Commented => "submitted, comment".to_owned(),
        ReviewState::Approved => "submitted, approved".to_owned(),
        ReviewState::ChangesRequested => "submitted, changes requested".to_owned(),
        ReviewState::Dismissed => "dismissed".to_owned(),
        ReviewState::Unknown => "submitted".to_owned(),
    }
}

fn is_pending(r: &Review) -> bool {
    matches!(r.state, jp_github::models::pulls::ReviewState::Pending)
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
