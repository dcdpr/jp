use crate::{error::Error, query::GLOBAL_CLIENT, util::html_to_markdown};

/// Fetch a crate's README and convert it to markdown.
pub async fn crate_readme(name: &str, version: &str) -> Result<String, Error> {
    let url = format!("https://crates.io/api/v1/crates/{name}/{version}/readme");

    let html = GLOBAL_CLIENT
        .http_client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    html_to_markdown(&html)
}
