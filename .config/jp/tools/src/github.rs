use crate::{
    Context, Error, Result, Tool,
    util::{ToolResult, unknown_tool},
};

mod create_issue_bug;
mod create_issue_enhancement;
mod create_issue_rfd_tracking;
mod issues;
mod pulls;
mod repo;
mod review;

use create_issue_bug::github_create_issue_bug;
use create_issue_enhancement::github_create_issue_enhancement;
use create_issue_rfd_tracking::github_create_issue_rfd_tracking;
use issues::github_issues;
use pulls::github_pulls;
use repo::{github_code_search, github_list_files, github_read_file};
use review::{github_pr_review_add_comment, github_pr_review_add_reply};

const ORG: &str = "dcdpr";
const REPO: &str = "jp";

/// Parse a `repository` argument of the form `owner/repo`, defaulting to
/// the project's own repo when unset.
pub(crate) fn parse_repo(repository: Option<String>) -> Result<(String, String)> {
    let repository = repository.unwrap_or_else(|| format!("{ORG}/{REPO}"));
    let (owner, repo) = repository
        .split_once('/')
        .ok_or("`repository` must be in the form of <owner>/<repo>")?;
    Ok((owner.to_owned(), repo.to_owned()))
}

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("github_") {
        "issues" => github_issues(
            t.opt("repository")?,
            t.opt_or_empty("number")?,
            t.opt("page")?,
        )
        .await
        .map(Into::into),

        "create_issue_bug" => github_create_issue_bug(
            t.req("title")?,
            t.req("description")?,
            t.req("expected_behavior")?,
            t.req("actual_behavior")?,
            t.req("complexity")?,
            t.opt("reproduce")?,
            t.opt("proposed_solution")?,
            t.opt("tasks")?,
            t.opt("resource_links")?,
            t.opt("labels")?,
            t.opt("assignees")?,
        )
        .await
        .map(Into::into),

        "create_issue_enhancement" => github_create_issue_enhancement(
            t.req("title")?,
            t.req("description")?,
            t.req("context")?,
            t.req("complexity")?,
            t.opt("alternatives")?,
            t.opt("proposed_implementation")?,
            t.opt("tasks")?,
            t.opt("resource_links")?,
            t.opt("labels")?,
            t.opt("assignees")?,
        )
        .await
        .map(Into::into),

        "create_issue_rfd_tracking" => github_create_issue_rfd_tracking(
            t.req("rfd_number")?,
            t.req("rfd_title")?,
            t.req("rfd_slug")?,
            t.req("tasks")?,
        )
        .await
        .map(Into::into),

        "pulls" => github_pulls(
            t.opt("repository")?,
            t.opt("number")?,
            t.opt("state")?,
            t.opt("file_diffs")?,
            t.opt("page")?,
        )
        .await
        .map(Into::into),

        "pr_review_add_comment" => {
            github_pr_review_add_comment(
                ctx,
                t.req("pull_number")?,
                t.req("path")?,
                t.req("line")?,
                t.req("body")?,
                t.opt("side")?,
                t.opt("start_line")?,
                t.opt("start_side")?,
            )
            .await
        }

        "pr_review_add_reply" => {
            github_pr_review_add_reply(
                ctx,
                t.req("pull_number")?,
                t.req("comment_id")?,
                t.req("body")?,
            )
            .await
        }

        "code_search" => github_code_search(t.opt("repository")?, t.req("query")?)
            .await
            .map(Into::into),

        "read_file" => github_read_file(
            t.opt("repository")?,
            t.opt("ref")?,
            t.req("path")?,
            t.opt("start_line")?,
            t.opt("end_line")?,
        )
        .await
        .map(Into::into),

        "list_files" => github_list_files(t.opt("repository")?, t.opt("ref")?, t.opt("path")?)
            .await
            .map(Into::into),

        _ => unknown_tool(t),
    }
}

/// Initialize the GitHub client with a token, verifying it works.
///
/// Use this for tools that modify state (`create_issue_*`,
/// `pr_review_*`) or that hit endpoints which always require auth
/// (GraphQL, search).
async fn auth() -> Result<()> {
    let token = read_token().ok_or(
        "unable to get auth token. Set `JP_GITHUB_TOKEN` or `GITHUB_TOKEN` to a valid token.",
    )?;

    let octocrab = jp_github::Octocrab::builder()
        .personal_token(token)
        .build()
        .map_err(|err| format!("unable to create github client: {err:#}"))?;

    jp_github::initialise(octocrab);

    if jp_github::instance().current().user().await.is_err() {
        return Err(
            "Unable to authenticate with github. This might be because the token is expired. \
             Either set `JP_GITHUB_TOKEN` or `GITHUB_TOKEN` to a valid token."
                .into(),
        );
    }

    Ok(())
}

/// Initialize the GitHub client, falling back to anonymous access when no
/// token is set.
///
/// Use this for read-only tools that work against public repos without
/// auth. Anonymous requests get GitHub's 60-req/hour rate limit; the
/// authenticated limit is 5000/hour. We do not verify the token here —
/// the real request will surface a 401 if the token is bad, which is a
/// better signal than a generic "can't authenticate" message.
async fn auth_optional() -> Result<()> {
    let mut builder = jp_github::Octocrab::builder();
    if let Some(token) = read_token() {
        builder = builder.personal_token(token);
    }

    let octocrab = builder
        .build()
        .map_err(|err| format!("unable to create github client: {err:#}"))?;

    jp_github::initialise(octocrab);
    Ok(())
}

fn read_token() -> Option<String> {
    // Filter each variable individually so an empty primary value doesn't
    // shadow a valid fallback. CI configs that set `JP_GITHUB_TOKEN=""`
    // alongside a real `GITHUB_TOKEN` should still authenticate.
    fn non_empty(name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|t| !t.is_empty())
    }

    non_empty("JP_GITHUB_TOKEN").or_else(|| non_empty("GITHUB_TOKEN"))
}

fn handle_404(error: jp_github::Error, msg: impl Into<String>) -> Error {
    match error {
        jp_github::Error::GitHub { source, .. } if source.status_code.as_u16() == 404 => {
            msg.into().into()
        }
        _ => Box::new(error) as Error,
    }
}
