use crate::{Error, Tool, Workspace};

pub(crate) mod create_issue_bug;
pub(crate) mod create_issue_enhancement;
pub(crate) mod issues;
pub(crate) mod pulls;
pub(crate) mod repo;

use create_issue_bug::github_create_issue_bug;
use create_issue_enhancement::github_create_issue_enhancement;
use issues::github_issues;
use pulls::github_pulls;
use repo::{github_code_search, github_read_file};

const ORG: &str = "dcdpr";
const REPO: &str = "jp";

pub async fn run(_: Workspace, t: Tool) -> std::result::Result<String, Error> {
    match t.name.trim_start_matches("github_") {
        "issues" => github_issues(t.opt("number")?).await,
        "create_issue_bug" => {
            github_create_issue_bug(
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
        }
        "create_issue_enhancement" => {
            github_create_issue_enhancement(
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
        }
        "pulls" => github_pulls(t.opt("number")?, t.opt("state")?, t.opt("file_diffs")?).await,
        "code_search" => github_code_search(t.opt("repository")?, t.req("query")?).await,
        "read_file" => github_read_file(t.opt("repository")?, t.req("path")?).await,
        _ => Err(format!("Unknown tool '{}'", t.name).into()),
    }
}

async fn auth() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| duct::cmd!("gh", "auth", "token").unchecked().read())
        .map_err(|err| format!("unable to get auth token: {err:#}"))?;

    let octocrab = octocrab::Octocrab::builder()
        .personal_token(token)
        .build()
        .map_err(|err| format!("unable to create github client: {err:#}"))?;

    octocrab::initialise(octocrab);

    // FIXME: If `GITHUB_TOKEN` is set, then we should do a check using
    // `octocrab::user()` to ensure that the token is valid. If not, we try `gh
    // auth token` once, to see if that solves the problem.

    Ok(())
}

fn handle_404(error: octocrab::Error, msg: impl Into<String>) -> Error {
    match error {
        octocrab::Error::GitHub { source, .. } if source.status_code.as_u16() == 404 => {
            msg.into().into()
        }
        _ => Box::new(error) as Error,
    }
}
