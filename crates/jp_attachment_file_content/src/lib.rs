use std::{
    collections::BTreeSet,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use glob::Pattern;
use jp_attachment::{
    distributed_slice, linkme, typetag, Attachment, BoxedHandler, Handler, HANDLERS,
};
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
impl Handler for FileContent {
    fn scheme(&self) -> &'static str {
        "file"
    }

    fn add(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
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

    fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        let pattern = uri_to_pattern(uri)?;

        self.excludes.remove(&pattern);
        self.includes.remove(&pattern);

        Ok(())
    }

    fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
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

    fn get(&self, cwd: &Path) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        debug!(id = self.scheme(), "Getting file attachment contents.");

        if self.includes.is_empty() {
            trace!("No include patterns specified, returning empty content.");
            return Ok(vec![]);
        }

        let mut paths = BTreeSet::new();
        for full_path in files_in_dir(cwd)? {
            let Ok(mut path) = full_path.strip_prefix(cwd).map(PathBuf::from) else {
                warn!(
                    ?full_path,
                    "Attachment path outside of working directory, skipping."
                );
                continue;
            };

            // We add back the root, so that patterns such as `/target/**/*`
            // match as expected.
            if !path.has_root() {
                path = PathBuf::from("/").join(path);
            }

            let opts = glob::MatchOptions {
                case_sensitive: false,
                require_literal_separator: true,
                require_literal_leading_dot: true,
            };

            let excluded = self
                .excludes
                .iter()
                .any(|exclude| exclude.matches_path_with(&path, opts));

            // Skip if excluded.
            if excluded {
                continue;
            }

            let included = self
                .includes
                .iter()
                .any(|include| include.matches_path_with(&path, opts));

            // Skip if not included.
            if !included {
                continue;
            }

            paths.insert(full_path);
        }

        let mut attachments = Vec::new();
        for path in paths {
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(path) = path.strip_prefix(cwd) else {
                continue;
            };

            attachments.push(Attachment {
                source: path.to_string_lossy().to_string(),
                content,
            });
        }

        Ok(attachments)
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

fn files_in_dir(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error + Send + Sync>> {
    let mut files = vec![];

    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(files_in_dir(&path)?);
        } else if path.is_file() {
            files.push(path);
        }
    }

    Ok(files)
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
    use tempfile::tempdir;
    use url::Url;

    use super::*;

    #[test]
    fn test_file_add_include() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();

        // Paths are relative, so the following sets are equivalent.
        handler.add(&Url::parse("file:path/to/include.txt")?)?;
        handler.add(&Url::parse("file:/path/to/include.txt")?)?;
        handler.add(&Url::parse("file://path/to/include.txt")?)?;
        handler.add(&Url::parse("file:///path/to/include.txt")?)?;

        handler.add(&Url::parse("file:**/*.md")?)?;
        handler.add(&Url::parse("file:/**/*.md")?)?;
        handler.add(&Url::parse("file://**/*.md")?)?;
        handler.add(&Url::parse("file:///**/*.md")?)?;

        assert_eq!(handler.includes.len(), 2);
        assert_eq!(handler.includes.iter().collect::<Vec<_>>(), vec![
            &Pattern::new("/**/*.md")?,
            &Pattern::new("/path/to/include.txt")?
        ]);
        assert!(handler.excludes.is_empty());

        Ok(())
    }

    #[test]
    fn test_file_add_exclude() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();
        handler.add(&Url::parse("file://path/**/exclude.txt?exclude")?)?;

        assert_eq!(handler.excludes.len(), 1);
        assert_eq!(handler.excludes.iter().collect::<Vec<_>>(), vec![
            &Pattern::new("/path/**/exclude.txt")?
        ]);
        assert!(handler.includes.is_empty());

        Ok(())
    }

    #[test]
    fn test_file_add_switches_include_exclude() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();
        let uri_include = Url::parse("file:/path/to/file.txt")?;
        let uri_exclude = Url::parse("file:/path/to/file.txt?exclude")?;

        // Add as include
        handler.add(&uri_include)?;
        assert!(handler
            .includes
            .contains(&Pattern::new("/path/to/file.txt")?));
        assert!(!handler
            .excludes
            .contains(&Pattern::new("/path/to/file.txt")?));

        // Add same path as exclude
        handler.add(&uri_exclude)?;
        assert!(!handler
            .includes
            .contains(&Pattern::new("/path/to/file.txt")?));
        assert!(handler
            .excludes
            .contains(&Pattern::new("/path/to/file.txt")?));

        Ok(())
    }

    #[test]
    fn test_file_remove() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();
        let uri1 = Url::parse("file:/path/to/file1.txt")?;
        let uri2 = Url::parse("file:/path/to/file2.txt?exclude")?;
        handler.add(&uri1)?;
        handler.add(&uri2)?;

        assert_eq!(handler.includes.len(), 1);
        assert_eq!(handler.excludes.len(), 1);

        // Remove file1 (was include)
        handler.remove(&uri1)?;
        assert!(handler.includes.is_empty());
        assert_eq!(handler.excludes.len(), 1);

        // Remove file2 (was exclude)
        handler.remove(&uri2)?;
        assert!(handler.includes.is_empty());
        assert!(handler.excludes.is_empty());

        Ok(())
    }

    #[test]
    fn test_file_get() -> Result<(), Box<dyn Error + Send + Sync>> {
        let tmp = tempdir()?;
        let path = tmp.path().join("file.txt");
        fs::write(&path, "content")?;

        let mut handler = FileContent::default();
        handler.add(&Url::parse("file:/file.txt")?)?;

        let attachments = handler.get(tmp.path())?;
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].source, "file.txt");
        assert_eq!(attachments[0].content, "content");

        Ok(())
    }
}
