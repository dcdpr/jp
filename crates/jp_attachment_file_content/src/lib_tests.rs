#[cfg(unix)]
use std::os::unix::fs::symlink;

use assert_matches::assert_matches;
use camino::Utf8PathBuf;
use camino_tempfile::{Utf8TempDir, tempdir};
use glob::Pattern;
use jp_tool::FsAccessError;
use url::Url;

use super::*;

/// A workspace `ws`, beside a directory `outside` holding `secret.txt`.
fn workspace_beside_a_secret() -> (Utf8TempDir, Utf8PathBuf) {
    let tmp = tempdir().unwrap();
    let ws = tmp.path().join("ws");
    fs::create_dir(&ws).unwrap();
    fs::create_dir(tmp.path().join("outside")).unwrap();
    fs::write(tmp.path().join("outside/secret.txt"), "secret").unwrap();
    (tmp, ws)
}

fn path_error(error: Box<dyn Error + Send + Sync>) -> PathError {
    *error.downcast::<PathError>().unwrap()
}

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

/// `file://notes.md` parses as host `notes.md` with path `/`; the host alone is
/// the file name.
#[tokio::test]
#[test_log::test]
async fn a_host_with_an_empty_path_names_a_file() -> Result<(), Box<dyn Error + Send + Sync>> {
    let tmp = tempdir()?;
    fs::write(tmp.path().join("notes.md"), "notes")?;

    let mut handler = FileContent::default();
    handler
        .add(&Url::parse("file://notes.md")?, tmp.path())
        .await?;

    assert_eq!(handler.includes.iter().collect::<Vec<_>>(), vec![
        &Pattern::new("/notes.md")?
    ]);

    let attachments = handler.get(tmp.path()).await?;
    assert_eq!(attachments, vec![Attachment::text("notes.md", "notes")]);

    Ok(())
}

#[test]
fn the_host_form_suggests_the_lossless_spelling() {
    let suggest = |url: &str| host_form_spelling(&Url::parse(url).unwrap());

    assert_eq!(suggest("file://notes.md"), Some("file:notes.md".to_owned()));
    // The parser lowercased `Docs`; the suggestion can only carry what it kept.
    assert_eq!(
        suggest("file://Docs/Notes.md"),
        Some("file:docs/Notes.md".to_owned())
    );
    assert_eq!(
        suggest("file://path/x.txt?exclude"),
        Some("file:path/x.txt?exclude".to_owned())
    );
    assert_eq!(suggest("file:docs/notes.md"), None);
    assert_eq!(suggest("file:///docs/notes.md"), None);
}

#[test]
fn a_table_path_goes_in_the_url_path() -> Result<(), Box<dyn Error + Send + Sync>> {
    let handler = FileContent::default();

    assert_eq!(
        handler.table_url("Docs/Notes.md")?.as_str(),
        "file:///Docs/Notes.md"
    );
    assert_eq!(handler.table_url("**/*.md")?.as_str(), "file:///**/*.md");
    assert_eq!(
        handler.table_url("café/my notes.md")?.as_str(),
        "file:///caf%C3%A9/my%20notes.md"
    );

    Ok(())
}

/// A table's `path` reaches the handler as written, including its case and
/// characters a URL percent-encodes.
#[tokio::test]
#[test_log::test]
async fn a_table_path_attaches_that_exact_file() -> Result<(), Box<dyn Error + Send + Sync>> {
    let tmp = tempdir()?;
    fs::create_dir(tmp.path().join("Docs"))?;
    fs::write(tmp.path().join("Docs/Café Notes.md"), "notes")?;

    let mut handler = FileContent::default();
    let url = handler.table_url("Docs/Café Notes.md")?;
    handler.add(&url, tmp.path()).await?;

    let attachments = handler.get(tmp.path()).await?;
    assert_eq!(attachments, vec![Attachment::text(
        "Docs/Café Notes.md",
        "notes"
    )]);

    Ok(())
}

/// A `%` in a table's `path` is part of the file name, not the start of a
/// percent-encoded character.
#[tokio::test]
#[test_log::test]
async fn a_table_path_with_a_percent_sign_attaches_that_exact_file()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let tmp = tempdir()?;
    fs::write(tmp.path().join("hello%20world.txt"), "literal")?;
    fs::write(tmp.path().join("hello world.txt"), "decoded")?;

    let mut handler = FileContent::default();
    let url = handler.table_url("hello%20world.txt")?;
    handler.add(&url, tmp.path()).await?;

    let attachments = handler.get(tmp.path()).await?;
    assert_eq!(attachments, vec![Attachment::text(
        "hello%20world.txt",
        "literal"
    )]);

    Ok(())
}

/// The URLs `list` returns name the patterns they were built from.
#[tokio::test]
#[test_log::test]
async fn listed_urls_round_trip_their_patterns() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut handler = FileContent::default();
    let cwd = Utf8Path::new("/");
    handler
        .add(&handler.table_url("100%25 done.md")?, cwd)
        .await?;

    let mut listed = FileContent::default();
    for url in handler.list().await? {
        listed.add(&url, cwd).await?;
    }

    assert_eq!(listed.includes.iter().collect::<Vec<_>>(), vec![
        &Pattern::new("/100%25 done.md")?
    ]);

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

    let attachments = handler.get(tmp.path()).await?;
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

    let attachments = handler.get(tmp.path()).await?;
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

    let attachments = handler.get(tmp.path()).await?;
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

    let attachments = handler.get(tmp.path()).await?;
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

    let attachments = handler.get(tmp.path()).await?;
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

    let attachments = handler.get(tmp.path()).await?;
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

/// `%2F` decodes to a separator, so an encoded `..` segment is caught after
/// decoding, before anything outside the workspace is read or stat'ed.
#[tokio::test]
#[test_log::test]
async fn an_encoded_parent_segment_cannot_leave_the_workspace() {
    let (_tmp, ws) = workspace_beside_a_secret();

    let mut handler = FileContent::default();
    let url = Url::parse("file:a%2F..%2F..%2Foutside%2Fsecret.txt").unwrap();
    let error = path_error(handler.add(&url, &ws).await.unwrap_err());

    assert_eq!(error.path, "a/../../outside/secret.txt");
    assert_matches!(error.reason, FsAccessError::InputEscape(_));
    assert!(handler.includes.is_empty());
}

#[cfg(unix)]
#[tokio::test]
#[test_log::test]
async fn a_named_symlink_out_of_the_workspace_is_refused() {
    let (tmp, ws) = workspace_beside_a_secret();
    symlink(tmp.path().join("outside/secret.txt"), ws.join("link.txt")).unwrap();

    let mut handler = FileContent::default();
    let url = Url::parse("file:link.txt").unwrap();
    let error = path_error(handler.add(&url, &ws).await.unwrap_err());

    assert_eq!(error.path, "link.txt");
    assert_matches!(error.reason, FsAccessError::Escape(_));
}

/// The read itself is confined, not only `add`: a handler restored from a
/// stored conversation reads its includes without going through `add`.
#[cfg(unix)]
#[tokio::test]
#[test_log::test]
async fn a_stored_include_out_of_the_workspace_is_refused_when_read() {
    let (tmp, ws) = workspace_beside_a_secret();
    symlink(tmp.path().join("outside/secret.txt"), ws.join("link.txt")).unwrap();

    let handler = FileContent {
        includes: BTreeSet::from([Pattern::new("/link.txt").unwrap()]),
        excludes: BTreeSet::new(),
    };
    let error = path_error(handler.get(&ws).await.unwrap_err());

    assert_eq!(error.path, "link.txt");
    assert_matches!(error.reason, FsAccessError::Escape(_));
}

/// A glob skips a symlink that leaves the workspace rather than failing, and
/// keeps the files beside it.
#[cfg(unix)]
#[tokio::test]
#[test_log::test]
async fn a_glob_skips_symlinks_out_of_the_workspace() {
    let (tmp, ws) = workspace_beside_a_secret();
    fs::create_dir(ws.join("docs")).unwrap();
    fs::write(ws.join("docs/real.md"), "real").unwrap();
    symlink(
        tmp.path().join("outside/secret.txt"),
        ws.join("docs/link.md"),
    )
    .unwrap();

    let mut handler = FileContent::default();
    handler
        .add(&Url::parse("file:docs/*.md").unwrap(), &ws)
        .await
        .unwrap();

    let attachments = handler.get(&ws).await.unwrap();
    assert_eq!(attachments, vec![Attachment::text("docs/real.md", "real")]);
}

/// A symlink to a file inside the workspace attaches under the name it was
/// given.
#[cfg(unix)]
#[tokio::test]
#[test_log::test]
async fn a_symlink_inside_the_workspace_attaches() {
    let (_tmp, ws) = workspace_beside_a_secret();
    fs::write(ws.join("real.md"), "real").unwrap();
    symlink("real.md", ws.join("alias.md")).unwrap();

    let mut handler = FileContent::default();
    handler
        .add(&Url::parse("file:alias.md").unwrap(), &ws)
        .await
        .unwrap();

    let attachments = handler.get(&ws).await.unwrap();
    assert_eq!(attachments, vec![Attachment::text("alias.md", "real")]);
}

#[tokio::test]
#[test_log::test]
async fn a_named_directory_attaches_the_files_under_it() {
    let (_tmp, ws) = workspace_beside_a_secret();
    fs::create_dir_all(ws.join("docs/nested")).unwrap();
    fs::write(ws.join("docs/nested/a.md"), "a").unwrap();

    let mut handler = FileContent::default();
    handler
        .add(&Url::parse("file:docs").unwrap(), &ws)
        .await
        .unwrap();

    let attachments = handler.get(&ws).await.unwrap();
    assert_eq!(attachments, vec![Attachment::text("docs/nested/a.md", "a")]);
}

fn canonical(uri: &str) -> Result<String, String> {
    FileContent::default()
        .canonical(&Url::parse(uri).unwrap())
        .map(|url| url.to_string())
        .map_err(|error| error.to_string())
}

#[test]
fn test_canonical_collapses_every_spelling_of_one_path() {
    for spelling in [
        "file:docs/notes.md",
        "file:/docs/notes.md",
        "file://docs/notes.md",
        "file:///docs/notes.md",
    ] {
        assert_eq!(
            canonical(spelling).as_deref(),
            Ok("file:///docs/notes.md"),
            "{spelling}"
        );
    }
}

#[test]
fn test_canonical_keeps_an_exclude() {
    assert_eq!(
        canonical("file:notes.md?exclude=true").as_deref(),
        Ok("file:///notes.md?exclude=")
    );
}

#[test]
fn test_canonical_rejects_an_invalid_pattern() {
    assert!(canonical("file:[").is_err());
}
