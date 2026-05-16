use std::{
    collections::HashSet,
    env, fs,
    future::Future,
    io,
    path::{Path, PathBuf},
};

use reqwest::header::ETAG;
use url::Url;
use zip::ZipArchive;

use crate::error::Error;

const DOCS_RS: &str = "https://docs.rs";

#[derive(Default)]
pub struct Config {
    pub root: Option<PathBuf>,
    pub crate_name: String,
    pub version: Option<String>,
    pub client: reqwest::Client,
}

impl TryFrom<&Url> for Config {
    type Error = Error;

    fn try_from(uri: &Url) -> Result<Self, Self::Error> {
        if uri.scheme() != "crate" {
            return Err(Error::Config(format!(
                "Invalid URI scheme: {}, expected 'crate'",
                uri.scheme()
            )));
        }

        let Some(name) = uri.host_str() else {
            return Err(Error::Config("Missing crate name in URI".to_string()));
        };

        let Some(version) = uri.path_segments().into_iter().flatten().next() else {
            return Err(Error::Config("Missing version in URI".to_string()));
        };

        if version != "latest" {
            semver::Version::parse(version)
                .map_err(|e| Error::Config(format!("invalid version format: {e}")))?;
        }

        Ok(Config {
            crate_name: name.to_string(),
            version: Some(version.to_string()),
            ..Default::default()
        })
    }
}

impl Config {
    #[must_use]
    pub fn crate_name(mut self, name: impl Into<String>) -> Self {
        self.crate_name = name.into();
        self
    }

    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    #[must_use]
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    #[must_use]
    pub fn client(mut self, client: impl Into<reqwest::Client>) -> Self {
        self.client = client.into();
        self
    }
}

pub async fn download(config: Config) -> Result<PathBuf, Error> {
    let version = config.version.unwrap_or_else(|| "latest".to_owned());
    let url = format!(
        "{}/crate/{}/{}/download",
        DOCS_RS, config.crate_name, version
    );

    let head = config.client.head(&url).send().await?;
    let etag = head
        .headers()
        .get(ETAG)
        .map(|h| h.to_str().unwrap_or_default())
        .unwrap_or_default()
        .replace('"', "");

    let destination = config
        .root
        .unwrap_or_else(env::temp_dir)
        .join(format!("{}/{version}/{etag}", config.crate_name));

    if destination.is_dir() {
        return Ok(destination);
    }

    let bytes = config
        .client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    unzip(&bytes, &destination)?;
    sanitize(&destination, &config.crate_name)?;
    rewrite_urls(&destination, &config.client).await?;

    Ok(destination)
}

fn unzip(bytes: &[u8], destination: &Path) -> Result<(), Error> {
    let cursor = io::Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor)?;

    for i in 0..archive.len() {
        let mut src = archive.by_index(i)?;
        if !src.is_file() {
            continue;
        }

        let Some(out) = src.enclosed_name().map(|p| destination.join(p)) else {
            continue;
        };

        if let Some(p) = out.parent()
            && !p.exists()
        {
            fs::create_dir_all(p)?;
        }

        let mut dest = fs::File::create(&out)?;
        io::copy(&mut src, &mut dest)?;
    }

    Ok(())
}

fn sanitize(path: &Path, crate_name: &str) -> Result<(), Error> {
    // Some generated docsets contain more than the default platform. For now,
    // it is OK to only parse the "main" platform and remove all the others
    for item in path.read_dir()? {
        let item = item?;
        if item.path().is_dir()
            && ![crate_name, "src", "implementors"]
                .contains(&item.file_name().to_string_lossy().as_ref())
        {
            fs::remove_dir_all(item.path())?;
        }
    }

    Ok(())
}

async fn rewrite_urls(root: &Path, client: &reqwest::Client) -> Result<(), Error> {
    walk_dirs(root, |file| async move {
        if file.path().extension().is_none_or(|ext| ext != "html") {
            return Ok(());
        }

        let mut data = fs::read_to_string(file.path())?;
        fs::create_dir_all(root.join("-/rustdoc.static"))?;

        let matches = data
            .match_indices(r#""/-/rustdoc.static/"#)
            .chain(data.match_indices(r#"data-search-js="search-"#))
            .chain(data.match_indices(r#"data-settings-js="settings-"#));

        let mut paths = HashSet::new();
        for (index, _) in matches {
            // Start after the opening quote.
            let Some(start) = data[index..].find('"').map(|start| index + start + 1) else {
                continue;
            };

            // Get until end of quoted path, or ignore match if no closing
            // quote.
            let Some(end) = data[start..].find('"').map(|end| start + end) else {
                continue;
            };

            let path = &data[start..end];

            // relative path to static file, without leading `/`.
            let path = path.strip_prefix('/').unwrap_or(path).to_owned();
            paths.insert(path);
        }

        for mut path in paths {
            if path.starts_with("search-") || path.starts_with("settings-") {
                path = format!("-/rustdoc.static/{path}");
            }

            // Only download missing files of known types.
            if !root.join(&path).exists()
                && Path::new(&path)
                    .extension()
                    .is_some_and(|ext| ext == "js" || ext == "css" || ext == "svg" || ext == "png")
            {
                let response = client
                    .get(format!("{}/{}", DOCS_RS, &path))
                    .send()
                    .await?
                    .error_for_status()?;
                let bytes = response.bytes().await?;
                fs::write(root.join(&path), bytes)?;
            }

            if !path.starts_with("-/rustdoc.static/search-")
                && !path.starts_with("-/rustdoc.static/settings-")
            {
                let mut i: usize = 0;
                let file_path = file.path();
                let ancestors = file_path.ancestors();
                for p in ancestors {
                    if p == root {
                        break;
                    }
                    i += 1;
                }

                let path_til_root =
                    (0..i.saturating_sub(1)).fold(String::new(), |acc, _| format!("../{acc}"));

                data = data.replace(
                    &format!(r#""/{}""#, &path),
                    &format!("{path_til_root}/{path}"),
                );
            }
        }

        fs::write(file.path(), data.as_bytes())?;

        Ok(())
    })
    .await?;

    Ok(())
}

async fn walk_dirs<F, Fut>(path: &Path, on_file: F) -> Result<(), Error>
where
    F: FnOnce(fs::DirEntry) -> Fut + Copy,
    Fut: Future<Output = Result<(), Error>>,
{
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.path().is_dir() {
            Box::pin(walk_dirs(&entry.path(), on_file)).await?;
        } else {
            on_file(entry).await?;
        }
    }

    Ok(())
}
