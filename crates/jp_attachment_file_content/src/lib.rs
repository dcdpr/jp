use std::{borrow::Cow, collections::BTreeSet, error::Error, fs};

use async_trait::async_trait;
use camino::Utf8Path;
use glob::Pattern;
use ignore::{WalkBuilder, WalkState, overrides::OverrideBuilder};
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, distributed_slice, linkme, typetag,
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

    async fn add(&mut self, uri: &Url, cwd: &Utf8Path) -> Result<(), Box<dyn Error + Send + Sync>> {
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
        cwd: &Utf8Path,
        _: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
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
        let (includes, paths): (Vec<_>, Vec<_>) = self
            .includes
            .iter()
            .partition(|p| p.as_str().contains(['*', '?', '[']) || cwd.join(p.as_str()).is_dir());

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
        return Ok(attachments);
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

/// 10 MiB. Fits comfortably under every provider's inline limit (Anthropic
/// 32 MB, OpenAI 20 MB, Gemini 20 MB) even after base64 expansion (~33%
/// overhead).
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
    use camino_tempfile::tempdir;
    use glob::Pattern;
    use indexmap::IndexMap;
    use url::Url;

    use super::*;

    #[tokio::test]
    #[test_log::test]
    async fn test_file_add_include() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();

        // Paths are relative, so the following sets are equivalent.
        let cwd = Utf8Path::new("/");
        handler
            .add(&Url::parse("file:path/include.txt")?, cwd)
            .await?;
        handler
            .add(&Url::parse("file:/path/include.txt")?, cwd)
            .await?;
        handler
            .add(&Url::parse("file://path/include.txt")?, cwd)
            .await?;
        handler
            .add(&Url::parse("file:///path/include.txt")?, cwd)
            .await?;

        handler.add(&Url::parse("file:**/*.md")?, cwd).await?;
        handler.add(&Url::parse("file:/**/*.md")?, cwd).await?;
        handler.add(&Url::parse("file://**/*.md")?, cwd).await?;
        handler.add(&Url::parse("file:///**/*.md")?, cwd).await?;

        assert_eq!(handler.includes.len(), 2);
        assert_eq!(handler.includes.iter().collect::<Vec<_>>(), vec![
            &Pattern::new("/**/*.md")?,
            &Pattern::new("/path/include.txt")?
        ]);
        assert!(handler.excludes.is_empty());

        Ok(())
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_file_add_exclude() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();
        handler
            .add(
                &Url::parse("file://path/**/exclude.txt?exclude")?,
                Utf8Path::new("/"),
            )
            .await?;

        assert_eq!(handler.excludes.len(), 1);
        assert_eq!(handler.excludes.iter().collect::<Vec<_>>(), vec![
            &Pattern::new("/path/**/exclude.txt")?
        ]);
        assert!(handler.includes.is_empty());

        Ok(())
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_file_add_switches_include_exclude() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();
        let uri_include = Url::parse("file:/path/to/file.txt")?;
        let uri_exclude = Url::parse("file:/path/to/file.txt?exclude")?;

        // Add as include
        let cwd = Utf8Path::new("/");
        handler.add(&uri_include, cwd).await?;
        assert!(
            handler
                .includes
                .contains(&Pattern::new("/path/to/file.txt")?)
        );
        assert!(
            !handler
                .excludes
                .contains(&Pattern::new("/path/to/file.txt")?)
        );

        // Add same path as exclude
        handler.add(&uri_exclude, cwd).await?;
        assert!(
            !handler
                .includes
                .contains(&Pattern::new("/path/to/file.txt")?)
        );
        assert!(
            handler
                .excludes
                .contains(&Pattern::new("/path/to/file.txt")?)
        );

        Ok(())
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_file_remove() -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut handler = FileContent::default();
        let uri1 = Url::parse("file:/path/to/file1.txt")?;
        let uri2 = Url::parse("file:/path/to/file2.txt?exclude")?;
        handler.add(&uri1, Utf8Path::new("/")).await?;
        handler.add(&uri2, Utf8Path::new("/")).await?;

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
    #[test_log::test]
    async fn test_file_get() -> Result<(), Box<dyn Error + Send + Sync>> {
        let tmp = tempdir()?;
        let path = tmp.path().join("file.txt");
        fs::write(&path, "content")?;

        let mut handler = FileContent::default();
        handler
            .add(&Url::parse("file:/file.txt")?, tmp.path())
            .await?;

        let client = Client::new(IndexMap::default());
        let attachments = handler.get(tmp.path(), client).await?;
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].source, "file.txt");
        assert_eq!(attachments[0].as_text(), Some("content"));

        Ok(())
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_file_get_image_png() -> Result<(), Box<dyn Error + Send + Sync>> {
        let tmp = tempdir()?;
        // Full PNG magic signature
        let png_bytes: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        fs::write(tmp.path().join("screenshot.png"), &png_bytes)?;

        let mut handler = FileContent::default();
        handler
            .add(&Url::parse("file:/screenshot.png")?, tmp.path())
            .await?;

        let client = Client::new(IndexMap::default());
        let attachments = handler.get(tmp.path(), client).await?;
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].source, "screenshot.png");
        assert!(attachments[0].is_binary());
        assert!(attachments[0].as_text().is_none());

        match &attachments[0].content {
            jp_attachment::AttachmentContent::Binary { data, media_type } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, &png_bytes);
            }
            jp_attachment::AttachmentContent::Text(_) => panic!("expected binary attachment"),
        }

        Ok(())
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_file_get_image_jpeg() -> Result<(), Box<dyn Error + Send + Sync>> {
        let tmp = tempdir()?;
        let jpg_bytes: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        fs::write(tmp.path().join("photo.jpg"), &jpg_bytes)?;

        let mut handler = FileContent::default();
        handler
            .add(&Url::parse("file:/photo.jpg")?, tmp.path())
            .await?;

        let client = Client::new(IndexMap::default());
        let attachments = handler.get(tmp.path(), client).await?;
        assert_eq!(attachments.len(), 1);

        match &attachments[0].content {
            jp_attachment::AttachmentContent::Binary { media_type, .. } => {
                assert_eq!(media_type, "image/jpeg");
            }
            jp_attachment::AttachmentContent::Text(_) => panic!("expected binary attachment"),
        }

        Ok(())
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_file_get_pdf() -> Result<(), Box<dyn Error + Send + Sync>> {
        let tmp = tempdir()?;
        // Minimal PDF: magic header followed by enough bytes for `infer` to
        // match.
        let pdf_bytes = b"%PDF-1.4 minimal";
        fs::write(tmp.path().join("doc.pdf"), pdf_bytes)?;

        let mut handler = FileContent::default();
        handler
            .add(&Url::parse("file:/doc.pdf")?, tmp.path())
            .await?;

        let client = Client::new(IndexMap::default());
        let attachments = handler.get(tmp.path(), client).await?;
        assert_eq!(attachments.len(), 1);
        assert!(attachments[0].is_binary());

        match &attachments[0].content {
            jp_attachment::AttachmentContent::Binary { media_type, .. } => {
                assert_eq!(media_type, "application/pdf");
            }
            jp_attachment::AttachmentContent::Text(_) => panic!("expected binary attachment"),
        }

        Ok(())
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_file_get_mixed_text_and_binary() -> Result<(), Box<dyn Error + Send + Sync>> {
        let tmp = tempdir()?;
        fs::write(tmp.path().join("readme.md"), "# Hello")?;
        // RIFF....WEBP magic bytes
        fs::write(tmp.path().join("logo.webp"), [
            0x52, 0x49, 0x46, 0x46, // RIFF
            0x00, 0x00, 0x00, 0x00, // file size (don't care)
            0x57, 0x45, 0x42, 0x50, // WEBP
        ])?;

        let mut handler = FileContent::default();
        handler
            .add(&Url::parse("file:/readme.md")?, tmp.path())
            .await?;
        handler
            .add(&Url::parse("file:/logo.webp")?, tmp.path())
            .await?;

        let client = Client::new(IndexMap::default());
        let attachments = handler.get(tmp.path(), client).await?;
        assert_eq!(attachments.len(), 2);

        let text_count = attachments.iter().filter(|a| a.is_text()).count();
        let binary_count = attachments.iter().filter(|a| a.is_binary()).count();
        assert_eq!(text_count, 1);
        assert_eq!(binary_count, 1);

        Ok(())
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_file_get_wrong_extension_detected_by_magic_bytes()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let tmp = tempdir()?;
        // PNG magic bytes, but with a .txt extension
        let png_bytes: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        fs::write(tmp.path().join("not_text.txt"), &png_bytes)?;

        let mut handler = FileContent::default();
        handler
            .add(&Url::parse("file:/not_text.txt")?, tmp.path())
            .await?;

        let client = Client::new(IndexMap::default());
        let attachments = handler.get(tmp.path(), client).await?;
        assert_eq!(attachments.len(), 1);
        assert!(attachments[0].is_binary());

        match &attachments[0].content {
            jp_attachment::AttachmentContent::Binary { media_type, .. } => {
                assert_eq!(media_type, "image/png");
            }
            jp_attachment::AttachmentContent::Text(_) => {
                panic!("expected binary attachment")
            }
        }

        Ok(())
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_file_add_rejects_oversized_binary() -> Result<(), Box<dyn Error + Send + Sync>> {
        let tmp = tempdir()?;

        // PNG magic bytes followed by zeroes to exceed the limit.
        let mut oversized = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        #[expect(clippy::cast_possible_truncation)]
        oversized.resize(MAX_BINARY_SIZE as usize + 1, 0);
        fs::write(tmp.path().join("huge.png"), &oversized)?;

        let mut handler = FileContent::default();
        let err = handler
            .add(&Url::parse("file:/huge.png")?, tmp.path())
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("too large"),
            "expected size error, got: {err}"
        );

        Ok(())
    }
}
