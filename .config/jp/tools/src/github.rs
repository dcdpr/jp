use crate::{
    Context, Error, Tool,
    util::{ToolResult, unknown_tool},
};

mod create_issue_bug;
mod create_issue_enhancement;
mod issues;
mod pulls;
mod repo;

use create_issue_bug::github_create_issue_bug;
use create_issue_enhancement::github_create_issue_enhancement;
use issues::github_issues;
use pulls::github_pulls;
use repo::{github_code_search, github_list_files, github_read_file};

const ORG: &str = "dcdpr";
const REPO: &str = "jp";

pub async fn run(_: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("github_") {
        "issues" => github_issues(t.opt_or_empty("number")?)
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

        "pulls" => github_pulls(t.opt("number")?, t.opt("state")?, t.opt("file_diffs")?)
            .await
            .map(Into::into),

        "code_search" => github_code_search(t.opt("repository")?, t.req("query")?)
            .await
            .map(Into::into),

        "read_file" => github_read_file(t.opt("repository")?, t.opt("ref")?, t.req("path")?)
            .await
            .map(Into::into),

        "list_files" => github_list_files(t.opt("repository")?, t.opt("ref")?, t.opt("path")?)
            .await
            .map(Into::into),

        _ => unknown_tool(t),
    }
}

async fn auth() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let token = std::env::var("JP_GITHUB_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .map_err(|_| {
            "unable to get auth token. Set `JP_GITHUB_TOKEN` or `GITHUB_TOKEN` to a valid token."
        })?;

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

fn handle_404(error: jp_github::Error, msg: impl Into<String>) -> Error {
    match error {
        jp_github::Error::GitHub { source, .. } if source.status_code.as_u16() == 404 => {
            msg.into().into()
        }
        _ => Box::new(error) as Error,
    }
}
