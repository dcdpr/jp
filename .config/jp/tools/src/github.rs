mod issues;
mod pulls;

pub(crate) use issues::github_issues as issues;
pub(crate) use pulls::{github_pulls as pulls, State};

const ORG: &str = "dcdpr";
const REPO: &str = "jp";

async fn auth() -> mcp_attr::Result<()> {
    let token = std::env::var("GITHUB_TOKEN")
        .ok()
        .or_else(|| duct::cmd!("gh", "auth", "token").unchecked().read().ok())
        .ok_or(mcp_attr::ErrorCode::INTERNAL_ERROR)?;

    let octocrab = octocrab::Octocrab::builder()
        .personal_token(token)
        .build()?;

    octocrab::initialise(octocrab);
    Ok(())
}
