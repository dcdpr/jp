use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::trace;
use url::Url;

use crate::{error::Error, query::GLOBAL_CLIENT};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateInfo {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub downloads: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<Url>,
}

/// Search for crates on crates.io.
pub async fn search_crates(query: &str) -> Result<Vec<CrateInfo>, Error> {
    let url = format!("https://crates.io/api/v1/crates?q={query}&per_page=10");

    let json: Value = GLOBAL_CLIENT
        .http_client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    trace!(%json, "Received crates.io search results.");

    let results = json
        .get("crates")
        .and_then(Value::as_array)
        .ok_or(Error::InvalidResponse)?;

    if results.is_empty() {
        return Ok(vec![]);
    }

    let mut crates = vec![];

    for crate_data in results {
        let Some(name) = crate_data.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(version) = crate_data.get("max_version").and_then(Value::as_str) else {
            continue;
        };
        let Some(downloads) = crate_data.get("downloads").and_then(Value::as_u64) else {
            continue;
        };

        let description = crate_data
            .get("description")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);

        let homepage = crate_data
            .get("homepage")
            .and_then(Value::as_str)
            .map(Url::parse)
            .transpose()
            .ok()
            .flatten();

        let documentation = crate_data
            .get("documentation")
            .and_then(Value::as_str)
            .map(Url::parse)
            .transpose()
            .ok()
            .flatten();

        let repository = crate_data
            .get("repository")
            .and_then(Value::as_str)
            .map(Url::parse)
            .transpose()
            .ok()
            .flatten();

        crates.push(CrateInfo {
            name: name.to_string(),
            version: version.to_string(),
            description,
            downloads,
            homepage,
            documentation,
            repository,
        });
    }

    Ok(crates)
}
