use std::{collections::BTreeSet, error::Error, fs, path::Path};

use async_trait::async_trait;
use glob::Pattern;
use ignore::{overrides::OverrideBuilder, WalkBuilder, WalkState};
use jp_attachment::{
    distributed_slice, linkme, typetag, Attachment, BoxedHandler, Handler, HANDLERS,
};
use jp_mcp::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace, warn};
use url::Url;

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HANDLER: fn() -> BoxedHandler = handler;

fn handler() -> BoxedHandler {
    (Box::new(FileContent::default()) as Box<dyn Handler>).into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FileContent {
    #[serde(with = "pat")]
    includes: BTreeSet<Pattern>,

    #[serde(with = "pat")]
    excludes: BTreeSet<Pattern>,
}

#[typetag::serde(name = "file_content")]
#[async_trait]
impl Handler for FileContent {
    fn scheme(&self) -> &'static str {
        "file"
    }

    async fn add(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        let pattern = uri_to_pattern(uri)?;

        if uri.query_pairs().any(|(k, _)| k == "exclude") {
            self.includes.remove(&pattern);
            self.excludes.insert(pattern);
        } else {
            self.excludes.remove(&pattern);
            self.includes.insert(pattern);
        }

        Ok(())
    }

    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        let pattern = uri_to_pattern(uri)?;

        self.excludes.remove(&pattern);
        self.includes.remove(&pattern);

        Ok(())
    }

    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
        let mut uris = vec![];

        for pattern in &self.includes {
            uris.push(pattern_to_uri(pattern)?);
        }

        for pattern in &self.excludes {
            let mut uri = pattern_to_uri(pattern)?;
            uri.query_pairs_mut().append_pair("exclude", "");
            uris.push(uri);
        }

        Ok(uris)
    }

    async fn get(
        &self,
        cwd: &Path,
        _: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        debug!(id = self.scheme(), "Getting file attachment contents.");

        if self.includes.is_empty() {
            trace!("No include patterns specified, returning empty content.");
            return Ok(vec![]);
        }

        let mut builder = OverrideBuilder::new(cwd);
        for pattern in &self.includes {
            let pattern = pattern.as_str();
            // We are hiding hidden files or directories by default (see
            // `hidden(true)`). If you were to add a pattern such as
            // `/.foo/bar.txt`, then that file would still be ignored, because
            // `/.foo` is a hidden directory and is not recursed. To fix this,
            // we need to explicitly add the initial hidden directory, e.g.
            // `/.foo`.
            //
            // The alternative is to never hide hidden files and directories by
            // default, but that causes significant performance issues as we
            // would traverse `.git` and other large hidden directories.
            //
            // We could just keep track of a large list of "potentially large"
            // hidden directory patterns such as `.git` or `.hg`, but that would
            // require ongoing maintenance and would be a bit of a hack.
            //
            // This too is a hack, but a more dynamic one that should hopefully
            // work in most common cases, but either way, hopefully we can find
            // a better solution in the future.
            if (pattern.starts_with('.') || pattern.starts_with("/."))
                // We only want to add the initial hidden directory if it's
                // actually a directory, not a file.
                && let Some((dir, _)) = pattern.split_once('.').and_then(|(_, dir)| dir.split_once('/'))
            {
                builder.add(&format!(".{dir}/"))?;
            }

            builder.add(pattern)?;
        }
        for p in &self.excludes {
            builder.add(&format!("!{p}"))?;
        }
        let overrides = builder.build()?;

        let (tx, rx) = crossbeam_channel::unbounded();
        WalkBuilder::new(cwd)
            .standard_filters(false)
            .hidden(true)
            .overrides(overrides)
            .follow_links(false)
            .build_parallel()
            .run(|| {
                let tx = tx.clone();
                Box::new(move |entry| {
                    let Ok(entry) = entry else {
                        return WalkState::Continue;
                    };
                    let path = entry.path();
                    if path.is_dir() {
                        return WalkState::Continue;
                    }

                    let Ok(rel) = path.strip_prefix(cwd) else {
                        warn!(
                            path = %path.display(),
                            "Attachment path outside of working directory, skipping."
                        );

                        return WalkState::Continue;
                    };

                    let content = match fs::read_to_string(path) {
                        Ok(content) => content,
                        Err(error) => {
                            warn!(path = %rel.display(), %error, "Failed to read attachment.");
                            return WalkState::Continue;
                        }
                    };

                    let _result = tx.send(Attachment {
                        source: rel.to_string_lossy().to_string(),
                        content,
                        ..Default::default()
                    });

                    WalkState::Continue
                })
            });

        drop(tx);
        return Ok(rx.into_iter().collect());
    }
}

/// We need to do some extra work to get the path relative to the
/// workspace.
///
/// - For `file:**/*.md`, the `**` part is marked as the domain, so we
///   need to merge it back.
/// - A file URI is *almost always absolute, but there's also a way to make it
///   relative:
///
///   - <file:path/to/file.txt> -> absolute
///   - <file:/path/to/file.txt> -> absolute
///   - <file://path/to/file.txt> -> relative (host is `path`)
///   - <file:///path/to/file.txt> -> absolute
///
/// To manage this, we make *all* paths absolute. This is okay, because paths
/// are always relative to the workspace root, so `path/to/file.txt` and
/// `/path/to/file.txt` both point to `{workspace}/path/to/file.txt`.
fn uri_to_pattern(uri: &Url) -> Result<Pattern, Box<dyn Error + Send + Sync>> {
    let domain = uri.host_str().unwrap_or("");
    let mut path = format!("{domain}{}", uri.path());
    if !path.starts_with('/') {
        path = format!("/{path}");
    }

    Ok(Pattern::new(&path).map_err(|e| e.to_string())?)
}

fn pattern_to_uri(pattern: &Pattern) -> Result<Url, Box<dyn Error + Send + Sync>> {
    let mut uri = Url::parse("file://")?;
    uri.set_path(pattern.as_str());
    Ok(uri)
}

mod pat {
    use std::collections::BTreeSet;

    use glob::Pattern;
    use serde::{Deserialize as _, Deserializer, Serialize as _, Serializer};

    pub fn serialize<S>(patterns: &BTreeSet<Pattern>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let vec: Vec<&str> = patterns.iter().map(Pattern::as_str).collect();

        vec.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BTreeSet<Pattern>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec: Vec<String> = Vec::deserialize(deserializer)?;
        let patterns = vec
            .into_iter()
            .map(|s| Pattern::new(&s).map_err(serde::de::Error::custom))
            .collect::<Result<BTreeSet<Pattern>, D::Error>>()?;

        Ok(patterns)
    }
}

#[cfg(test)]
mod tests {
    use glob::Pattern;
    use indexmap::IndexMap;
    use tempfile::tempdir;
    use url::Url;

    use super::*;

    #[tokio::test]
    async fn test_file_add_include() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();

        // Paths are relative, so the following sets are equivalent.
        handler.add(&Url::parse("file:path/include.txt")?).await?;
        handler.add(&Url::parse("file:/path/include.txt")?).await?;
        handler.add(&Url::parse("file://path/include.txt")?).await?;
        handler
            .add(&Url::parse("file:///path/include.txt")?)
            .await?;

        handler.add(&Url::parse("file:**/*.md")?).await?;
        handler.add(&Url::parse("file:/**/*.md")?).await?;
        handler.add(&Url::parse("file://**/*.md")?).await?;
        handler.add(&Url::parse("file:///**/*.md")?).await?;

        assert_eq!(handler.includes.len(), 2);
        assert_eq!(handler.includes.iter().collect::<Vec<_>>(), vec![
            &Pattern::new("/**/*.md")?,
            &Pattern::new("/path/include.txt")?
        ]);
        assert!(handler.excludes.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_file_add_exclude() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();
        handler
            .add(&Url::parse("file://path/**/exclude.txt?exclude")?)
            .await?;

        assert_eq!(handler.excludes.len(), 1);
        assert_eq!(handler.excludes.iter().collect::<Vec<_>>(), vec![
            &Pattern::new("/path/**/exclude.txt")?
        ]);
        assert!(handler.includes.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_file_add_switches_include_exclude() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();
        let uri_include = Url::parse("file:/path/to/file.txt")?;
        let uri_exclude = Url::parse("file:/path/to/file.txt?exclude")?;

        // Add as include
        handler.add(&uri_include).await?;
        assert!(handler
            .includes
            .contains(&Pattern::new("/path/to/file.txt")?));
        assert!(!handler
            .excludes
            .contains(&Pattern::new("/path/to/file.txt")?));

        // Add same path as exclude
        handler.add(&uri_exclude).await?;
        assert!(!handler
            .includes
            .contains(&Pattern::new("/path/to/file.txt")?));
        assert!(handler
            .excludes
            .contains(&Pattern::new("/path/to/file.txt")?));

        Ok(())
    }

    #[tokio::test]
    async fn test_file_remove() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();
        let uri1 = Url::parse("file:/path/to/file1.txt")?;
        let uri2 = Url::parse("file:/path/to/file2.txt?exclude")?;
        handler.add(&uri1).await?;
        handler.add(&uri2).await?;

        assert_eq!(handler.includes.len(), 1);
        assert_eq!(handler.excludes.len(), 1);

        // Remove file1 (was include)
        handler.remove(&uri1).await?;
        assert!(handler.includes.is_empty());
        assert_eq!(handler.excludes.len(), 1);

        // Remove file2 (was exclude)
        handler.remove(&uri2).await?;
        assert!(handler.includes.is_empty());
        assert!(handler.excludes.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_file_get() -> Result<(), Box<dyn Error + Send + Sync>> {
        let tmp = tempdir()?;
        let path = tmp.path().join("file.txt");
        fs::write(&path, "content")?;

        let mut handler = FileContent::default();
        handler.add(&Url::parse("file:/file.txt")?).await?;

        let client = Client::new(IndexMap::default());
        let attachments = handler.get(tmp.path(), client).await?;
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].source, "file.txt");
        assert_eq!(attachments[0].content, "content");

        Ok(())
    }
}
