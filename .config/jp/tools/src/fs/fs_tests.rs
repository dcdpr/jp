use camino_tempfile::tempdir;
use jp_tool::{Action, Context};
use serde_json::{Map, Value, json};

use super::*;

/// A `list_files` invocation carrying the given `suppress` option value.
fn list_files_with_suppress(root: &camino::Utf8Path, suppress: Value) -> (Context, Tool) {
    let ctx = Context {
        root: root.to_path_buf(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };
    let tool = Tool {
        name: "fs_list_files".to_owned(),
        arguments: Map::new(),
        answers: Map::new(),
        options: Map::from_iter([("suppress".to_owned(), suppress)]),
    };

    (ctx, tool)
}

#[tokio::test]
async fn a_non_array_suppress_option_fails_the_invocation() {
    // The strict parse replaced `option_or`, which turned any unreadable value
    // into an empty list and handed over the very paths the option named. This
    // drives the dispatcher itself, so a regression back to the lenient read is
    // caught here rather than by nobody.
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "").unwrap();

    let (ctx, tool) = list_files_with_suppress(tmp.path(), json!("oops"));
    let err = run(ctx, tool).await.expect_err("expected a hard failure");

    assert!(
        err.to_string()
            .starts_with("Invalid `suppress` option for tool 'fs_list_files'"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn an_object_suppress_option_fails_the_invocation() {
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "").unwrap();

    let (ctx, tool) = list_files_with_suppress(tmp.path(), json!({"paths": [".git/"]}));
    let err = run(ctx, tool).await.expect_err("expected a hard failure");

    assert!(
        err.to_string()
            .starts_with("Invalid `suppress` option for tool 'fs_list_files'"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn an_unparseable_suppress_pattern_fails_the_invocation() {
    // The value deserializes but the glob does not compile: the other half of
    // the same "never silently suppress nothing" guarantee, checked through the
    // dispatcher rather than against `suppress_matcher` directly.
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "").unwrap();

    let (ctx, tool) = list_files_with_suppress(tmp.path(), json!(["{unclosed"]));
    let err = run(ctx, tool).await.expect_err("expected a hard failure");

    assert!(
        err.to_string().contains("{unclosed"),
        "pattern not named: {err}"
    );
}

#[tokio::test]
async fn a_valid_suppress_option_is_accepted() {
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "").unwrap();

    let (ctx, tool) = list_files_with_suppress(tmp.path(), json!([".git/", "**/target/"]));

    assert!(run(ctx, tool).await.is_ok());
}
