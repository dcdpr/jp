use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{error::Error, query::GLOBAL_CLIENT};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateMetadata {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<Url>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(flatten)]
    pub version: CrateVersion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateVersion {
    pub num: String,
    pub created_at: DateTime<Utc>,
    pub downloads: u64,
    pub license: Option<String>,
    pub published_by: Option<String>,
    pub yanked: bool,
    pub msrv: Option<String>,
}

// crates.io v1 API response shapes — only the fields we consume.

#[derive(Deserialize)]
struct CrateResponse {
    #[serde(rename = "crate")]
    crate_data: CrateData,
    versions: Vec<RawVersion>,
    keywords: Vec<RawKeyword>,
    categories: Vec<RawCategory>,
}

#[derive(Deserialize)]
struct CrateData {
    name: String,
    description: Option<String>,
    homepage: Option<String>,
    documentation: Option<String>,
    repository: Option<String>,
}

#[derive(Deserialize)]
struct RawVersion {
    num: String,
    created_at: DateTime<Utc>,
    downloads: u64,
    license: Option<String>,
    yanked: bool,
    rust_version: Option<String>,
    published_by: Option<RawPublisher>,
}

#[derive(Deserialize)]
struct RawPublisher {
    login: String,
}

#[derive(Deserialize)]
struct RawKeyword {
    keyword: String,
}

#[derive(Deserialize)]
struct RawCategory {
    category: String,
}

/// Search for crates on crates.io.
pub async fn crate_metadata(crate_name: &str, crate_version: &str) -> Result<CrateMetadata, Error> {
    let url = format!("https://crates.io/api/v1/crates/{crate_name}");
    let response: CrateResponse = GLOBAL_CLIENT
        .http_client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let version = response
        .versions
        .into_iter()
        .find(|v| v.num == crate_version)
        .ok_or_else(|| Error::VersionNotFound {
            crate_name: crate_name.to_string(),
            version: crate_version.to_string(),
        })?;

    Ok(CrateMetadata {
        name: response.crate_data.name,
        description: response.crate_data.description,
        homepage: response
            .crate_data
            .homepage
            .map(|v| v.parse())
            .transpose()?,
        documentation: response
            .crate_data
            .documentation
            .map(|v| v.parse())
            .transpose()?,
        repository: response
            .crate_data
            .repository
            .map(|v| v.parse())
            .transpose()?,
        keywords: response.keywords.into_iter().map(|k| k.keyword).collect(),
        categories: response
            .categories
            .into_iter()
            .map(|c| c.category)
            .collect(),
        version: CrateVersion {
            num: version.num,
            created_at: version.created_at,
            downloads: version.downloads,
            license: version.license,
            published_by: version.published_by.map(|u| u.login),
            yanked: version.yanked,
            msrv: version.rust_version,
        },
    })
}
