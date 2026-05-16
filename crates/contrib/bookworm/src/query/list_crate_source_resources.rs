use std::{fs, path::Path};

use url::Url;

use crate::{dl, error::Error, query::GLOBAL_CLIENT};

/// List all Rust source files for a crate.
pub async fn list_crate_source_resources(
    name: &str,
    version: Option<&str>,
) -> Result<Vec<Url>, Error> {
    let version = version.unwrap_or("latest");
    let dl_cfg = dl::Config::default()
        .crate_name(name)
        .version(version)
        .root(&GLOBAL_CLIENT.crates_path)
        .client(GLOBAL_CLIENT.http_client.clone());

    let root = dl::download(dl_cfg).await?.join("src");

    let mut urls = vec![];
    collect_resources(&root, &mut urls, |file| {
        if file.path().extension().is_none_or(|ext| ext != "html") {
            return Ok(None);
        }

        let path = file.path();
        let Ok(path) = path.strip_prefix(&root) else {
            return Ok(None);
        };

        let Ok(url) = Url::parse(&format!(
            "crate://{name}/{version}/src/{}",
            path.to_string_lossy()
        )) else {
            return Ok(None);
        };

        Ok(Some(url))
    })?;

    Ok(urls)
}

fn collect_resources<F>(path: &Path, urls: &mut Vec<Url>, on_file: F) -> Result<(), Error>
where
    F: FnOnce(fs::DirEntry) -> Result<Option<Url>, Error> + Copy,
{
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.path().is_dir() {
            collect_resources(&entry.path(), urls, on_file)?;
        } else if let Some(url) = on_file(entry)? {
            urls.push(url);
        }
    }

    Ok(())
}
