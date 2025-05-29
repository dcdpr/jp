mod issues;
mod pulls;
mod repo;

pub(crate) use issues::github_issues as issues;
pub(crate) use pulls::{github_pulls as pulls, State};
pub(crate) use repo::{github_file_contents as file_contents, github_search_code as search_code};

const ORG: &str = "dcdpr";
const REPO: &str = "jp";

async fn auth() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| duct::cmd!("gh", "auth", "token").unchecked().read())
        .map_err(|err| format!("unable to get auth token: {err:#}"))?;

    let octocrab = octocrab::Octocrab::builder()
        .personal_token(token)
        .build()
        .map_err(|err| format!("unable to create github client: {err:#}"))?;

    octocrab::initialise(octocrab);
    Ok(())
}
