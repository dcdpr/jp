use crate::Error;

pub(crate) mod issues;
pub(crate) mod pulls;
pub(crate) mod repo;

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

fn handle_404(error: octocrab::Error, msg: impl Into<String>) -> Error {
    match error {
        octocrab::Error::GitHub { source, .. } if source.status_code.as_u16() == 404 => {
            msg.into().into()
        }
        _ => Box::new(error) as Error,
    }
}
