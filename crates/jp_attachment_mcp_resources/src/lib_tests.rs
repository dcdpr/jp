use jp_attachment::Handler as _;

use super::*;

fn uri() -> Url {
    Url::parse("mcp+github-mcp-server+repo://owner/name").unwrap()
}

#[tokio::test]
async fn stored_attachment_is_listed_and_removable() {
    let mut handler = McpResources::default();
    handler.add(&uri(), Utf8Path::new("/")).await.unwrap();

    assert_eq!(handler.list().await.unwrap(), vec![uri()]);

    handler.remove(&uri()).await.unwrap();
    assert_eq!(handler.list().await.unwrap(), Vec::<Url>::new());
}

#[tokio::test]
async fn resolving_a_stored_attachment_names_it_and_how_to_remove_it() {
    let mut handler = McpResources::default();
    handler.add(&uri(), Utf8Path::new("/")).await.unwrap();

    let error = handler
        .get(Utf8Path::new("/"))
        .await
        .expect_err("mcp resource attachments no longer resolve");

    assert_eq!(
        error.to_string(),
        "MCP resource attachments are no longer resolved: \
         `mcp+github-mcp-server+repo://owner/name`. Remove it with `jp attachment rm \
         mcp+github-mcp-server+repo://owner/name`."
    );
}

/// A handler registered but never given a URI has nothing to refuse: the
/// scheme's presence in the registry must not fail a query that carries no
/// `mcp` attachment.
#[tokio::test]
async fn an_empty_handler_resolves_to_nothing() {
    let handler = McpResources::default();

    assert_eq!(handler.get(Utf8Path::new("/")).await.unwrap(), vec![]);
}
