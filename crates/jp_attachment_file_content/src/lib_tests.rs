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
