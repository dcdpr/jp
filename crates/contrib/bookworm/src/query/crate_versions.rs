use serde::Serialize;
use serde_json::Value;

use crate::{error::Error, query::GLOBAL_CLIENT};

#[derive(Serialize)]
pub struct CrateVersion {
    pub num: String,
    pub created_at: String,
    pub downloads: u64,
    pub yanked: bool,
    pub msrv: Option<String>,
}

/// Fetch latest versions of a crate.
pub async fn crate_versions(name: &str) -> Result<Vec<CrateVersion>, Error> {
    let url = format!("https://crates.io/api/v1/crates/{name}/versions");

    let json: Value = GLOBAL_CLIENT
        .http_client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let results = json
        .get("versions")
        .and_then(Value::as_array)
        .ok_or(Error::InvalidResponse)?;

    let mut versions = vec![];
    for version in results {
        let Some(num) = version.get("num").and_then(Value::as_str) else {
            return Err(Error::InvalidResponse);
        };
        let Some(created_at) = version.get("created_at").and_then(Value::as_str) else {
            return Err(Error::InvalidResponse);
        };
        let Some(downloads) = version.get("downloads").and_then(Value::as_u64) else {
            return Err(Error::InvalidResponse);
        };

        let yanked = version
            .get("yanked")
            .and_then(Value::as_bool)
            .unwrap_or_default();

        let msrv = version
            .get("rust_version")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);

        versions.push(CrateVersion {
            num: num.to_owned(),
            created_at: created_at.to_owned(),
            downloads,
            yanked,
            msrv,
        });
    }

    Ok(versions)
}
