use std::{borrow::Cow, collections::BTreeSet, error::Error, fs};

use camino::Utf8Path;
use glob::Pattern;
use ignore::{WalkBuilder, WalkState, overrides::OverrideBuilder};
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, HandlerFuture, distributed_slice, linkme,
    percent_decode_str, typetag,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, trace, warn};
use url::{Position, Url};

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

    fn canonical(&self, uri: &Url) -> Result<Url, Box<dyn Error + Send + Sync>> {
        let mut url = pattern_to_uri(&uri_to_pattern(uri)?)?;
        if uri.query_pairs().any(|(k, _)| k == "exclude") {
            url.query_pairs_mut().append_pair("exclude", "");
        }

        Ok(url)
    }

    /// `file:///<path>`, keeping `path` out of the URL host, where the parser
    /// would lowercase and IDNA-encode its first segment.
    fn table_url(&self, path: &str) -> Result<Url, Box<dyn Error + Send + Sync>> {
        let mut url = Url::parse("file:///")?;
        url.set_path(path);
        Ok(url)
    }

    fn add<'a>(&'a mut self, uri: &'a Url, cwd: &'a Utf8Path) -> HandlerFuture<'a, ()> {
        Box::pin(async move {
            let pattern = uri_to_pattern(uri)?;
            let is_exclude = uri.query_pairs().any(|(k, _)| k == "exclude");

            // For concrete file paths (no globs), validate size upfront so the
            // user gets an immediate error rather than a silent skip at query time.
            if !is_exclude && !pattern.as_str().contains(['*', '?', '[']) {
                let rel = pattern.as_str().trim_start_matches('/');
                let path = cwd.join(rel);
                if path.is_file() {
                    let size = std::fs::metadata(&path)?.len();
                    if size > MAX_BINARY_SIZE {
                        return Err(format!(
                            "File '{rel}' is too large ({size} bytes, limit is {MAX_BINARY_SIZE} \
                             bytes)."
                        )
                        .into());
                    }
                }
            }

            if is_exclude {
                self.includes.remove(&pattern);
                self.excludes.insert(pattern);
            } else {
                self.excludes.remove(&pattern);
                self.includes.insert(pattern);
            }

            Ok(())
        })
    }

    fn remove<'a>(&'a mut self, uri: &'a Url) -> HandlerFuture<'a, ()> {
        Box::pin(async move {
            let pattern = uri_to_pattern(uri)?;

            self.excludes.remove(&pattern);
            self.includes.remove(&pattern);

            Ok(())
        })
    }

    fn list(&self) -> HandlerFuture<'_, Vec<Url>> {
        Box::pin(async move {
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
        })
    }

    fn get<'a>(&'a self, cwd: &'a Utf8Path) -> HandlerFuture<'a, Vec<Attachment>> {
        Box::pin(async move {
            debug!(id = self.scheme(), "Getting file attachment contents.");

            if self.includes.is_empty() {
                trace!("No include patterns specified, returning empty content.");
                return Ok(vec![]);
            }

            let mut attachments = Vec::with_capacity(self.includes.len());

            // Any includes that are paths without glob patterns are always
            // included, so we can skip any expensive tree walking for `.ignore`
            // files nor do we care about `self.excludes`.
            //
            // We also check if the path is a directory, as those will have to use
            // globbing and respect `.ignore` files.
            let (includes, paths): (Vec<_>, Vec<_>) = self.includes.iter().partition(|p| {
                p.as_str().contains(['*', '?', '[']) || cwd.join(p.as_str()).is_dir()
            });

            attachments.extend(paths.into_iter().filter_map(|pattern| {
                let pattern = sanitize_pattern(pattern.as_str(), cwd);
                let path = cwd.join(pattern.as_ref());
                build_attachment(&path, cwd)
            }));

            if includes.is_empty() {
                return Ok(attachments);
            }

            let mut builder = OverrideBuilder::new(cwd);
            for pattern in includes {
                let pattern = sanitize_pattern(pattern.as_str(), cwd);

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
                if pattern.starts_with('.')
                // We only want to add the initial hidden directory if it's
                // actually a directory, not a file.
                && let Some((dir, _)) = pattern.split_once('.').and_then(|(_, dir)| dir.split_once('/'))
                {
                    builder.add(&format!(".{dir}/"))?;
                }

                builder.add(pattern.as_ref())?;
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
                        let Some(path) = Utf8Path::from_path(entry.path()) else {
                            return WalkState::Continue;
                        };
                        if path.is_dir() {
                            return WalkState::Continue;
                        }

                        let Some(attachment) = build_attachment(path, cwd) else {
                            return WalkState::Continue;
                        };

                        let _result = tx.send(attachment);

                        WalkState::Continue
                    })
                });

            drop(tx);
            attachments.extend(rx);
            Ok(attachments)
        })
    }
}

/// If the pattern is a directory, add it recursively.
fn sanitize_pattern<'a>(mut pattern: &'a str, cwd: &Utf8Path) -> Cow<'a, str> {
    if pattern.starts_with('/') {
        pattern = &pattern[1..];
    }

    if cwd.join(pattern).is_dir() {
        if pattern.ends_with('/') {
            pattern = &pattern[..pattern.len() - 1];
        }

        Cow::Owned(format!("{pattern}/**/*"))
    } else {
        Cow::Borrowed(pattern)
    }
}

/// 10 MiB.
/// Fits comfortably under every provider's inline limit (Anthropic 32 MB,
/// OpenAI 20 MB, Gemini 20 MB) even after base64 expansion (~33% overhead).
const MAX_BINARY_SIZE: u64 = 10 * 1024 * 1024;

fn build_attachment(path: &Utf8Path, cwd: &Utf8Path) -> Option<Attachment> {
    let Ok(rel) = path.strip_prefix(cwd) else {
        warn!(
            path = %path,
            "Attachment path outside of working directory, skipping."
        );

        return None;
    };

    // Detect binary file types from magic bytes. `get_from_path` only reads the
    // first few KB, so we don't load the entire file into memory just to check
    // the type.
    let inferred = match infer::get_from_path(path) {
        Ok(kind) => kind,
        Err(error) => {
            warn!(path = %rel, %error, "Failed to read attachment for type detection.");
            return None;
        }
    };

    // If `infer` recognizes the file type, treat it as binary. Provider
    // implementations are responsible for filtering to the MIME types they
    // support and warning on unsupported ones.
    if let Some(kind) = inferred {
        let size = match fs::metadata(path) {
            Ok(m) => m.len(),
            Err(error) => {
                warn!(path = %rel, %error, "Failed to stat binary attachment.");
                return None;
            }
        };

        if size > MAX_BINARY_SIZE {
            warn!(
                path = %rel,
                size,
                max = MAX_BINARY_SIZE,
                "Binary attachment exceeds size limit, skipping."
            );
            return None;
        }

        let data = match fs::read(path) {
            Ok(data) => data,
            Err(error) => {
                warn!(path = %rel, %error, "Failed to read binary attachment.");
                return None;
            }
        };

        return Some(Attachment::binary(rel.to_string(), data, kind.mime_type()));
    }

    // Not a recognized binary type - treat as text.
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            warn!(path = %rel, %error, "Failed to read attachment.");
            return None;
        }
    };

    Some(Attachment::text(rel.to_string(), content))
}

/// The workspace-relative pattern a `file` URL names, rooted at `/`.
///
/// Every path is relative to the workspace root, so `file:path/to/file.txt`,
/// `file:/path/to/file.txt` and `file:///path/to/file.txt` all name
/// `{workspace}/path/to/file.txt`.
/// The path is percent-decoded.
///
/// `file://path/to/file.txt` parses with host `path`, which is read as the
/// first path segment, and `file://file.txt` as the file name alone.
/// The parser has already lowercased and IDNA-encoded that host, so this form
/// logs a warning naming the `file:<path>` spelling.
fn uri_to_pattern(uri: &Url) -> Result<Pattern, Box<dyn Error + Send + Sync>> {
    let path = percent_decode_str(uri.path())?;
    let mut path = match uri.host_str().filter(|host| !host.is_empty()) {
        Some(host) if path == "/" => host.to_owned(),
        Some(host) => format!("{host}{path}"),
        None => path,
    };

    if let Some(spelling) = host_form_spelling(uri) {
        warn!(
            url = %uri,
            %spelling,
            "File attachment URL starts its path in the URL host, which URL parsing lowercases. \
             Write it as `file:<path>`."
        );
    }

    if !path.starts_with('/') {
        path = format!("/{path}");
    }

    Ok(Pattern::new(&path).map_err(|e| e.to_string())?)
}

/// The `file:<path>` spelling of a URL whose path starts in its host, such as
/// `file://docs/notes.md`.
///
/// `None` when the URL has no host.
/// The parser lowercases and IDNA-encodes a host, so the spelling carries the
/// host as parsed, which can differ from what was written.
fn host_form_spelling(uri: &Url) -> Option<String> {
    let host = uri.host_str().filter(|host| !host.is_empty())?;
    let path = match uri.path() {
        "/" => "",
        path => path,
    };

    Some(format!("file:{host}{path}{}", &uri[Position::AfterPath..]))
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
#[path = "lib_tests.rs"]
mod tests;
