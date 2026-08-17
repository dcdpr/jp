use std::fs;

use camino_tempfile::{Utf8TempDir, tempdir};
use jp_tool::{Action, Outcome};
use pretty_assertions::assert_eq;

use super::*;
use crate::util::runner::{ExitCode, MockProcessRunner, ProcessOutput};

/// The flags every invocation carries, in the order the tool builds them.
const BASE_ARGS: &[&str] = &[
    "--list-changed",
    "--format-markdown",
    "--reference-links",
    "--prune-reference-links",
    "--language",
    "markdown",
];

fn ctx() -> (Utf8TempDir, Context) {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    (dir, ctx)
}

fn paths(paths: &[&str]) -> OneOrMany<String> {
    OneOrMany::Many(paths.iter().map(|p| (*p).to_owned()).collect())
}

#[test]
fn no_paths_formats_the_whole_workspace() {
    let (_dir, ctx) = ctx();
    let expected_args = [BASE_ARGS, &["--workspace"]].concat();
    let runner = MockProcessRunner::builder()
        .expect("comfort")
        .args(&expected_args)
        .returns_success("");

    let result = markdown_format_impl(&ctx, None, &runner).unwrap();
    assert_eq!(result.unwrap_content(), "No files to format.");
}

#[test]
fn explicit_paths_replace_the_workspace_scope() {
    let (_dir, ctx) = ctx();
    let expected_args = [BASE_ARGS, &["docs/rfd/drafts/D01-knowledge-base.md"]].concat();
    let runner = MockProcessRunner::builder()
        .expect("comfort")
        .args(&expected_args)
        .returns_success("");

    let result = markdown_format_impl(
        &ctx,
        Some(paths(&["docs/rfd/drafts/D01-knowledge-base.md"])),
        &runner,
    )
    .unwrap();
    assert_eq!(result.unwrap_content(), "No files to format.");
}

#[test]
fn a_directory_is_passed_through_for_comfort_to_walk() {
    let (dir, ctx) = ctx();
    fs::create_dir_all(dir.path().join("docs/rfd")).unwrap();
    let expected_args = [BASE_ARGS, &["docs/rfd"]].concat();
    let runner = MockProcessRunner::builder()
        .expect("comfort")
        .args(&expected_args)
        .returns_success("");

    let result = markdown_format_impl(&ctx, Some(paths(&["docs/rfd"])), &runner).unwrap();
    assert_eq!(result.unwrap_content(), "No files to format.");
}

#[test]
fn changed_files_are_root_stripped_deduplicated_and_sorted() {
    let (_dir, ctx) = ctx();
    let stdout = format!(
        "{root}/docs/usage.md\n{root}/README.md\n{root}/docs/usage.md\n",
        root = ctx.root
    );
    let runner = MockProcessRunner::builder()
        .expect("comfort")
        .returns_success(stdout);

    let result = markdown_format_impl(&ctx, None, &runner).unwrap();
    assert_eq!(
        result.unwrap_content(),
        "Formatted files:\n- README.md\n- docs/usage.md"
    );
}

#[test]
fn a_non_markdown_file_is_refused_without_running_comfort() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let result =
        markdown_format_impl(&ctx, Some(paths(&["crates/jp_cli/src/main.rs"])), &runner).unwrap();
    match result {
        Outcome::Error { message, .. } => assert_eq!(
            message,
            "'crates/jp_cli/src/main.rs' is not a Markdown file or a directory. Only `.md` and \
             `.markdown` files are formatted."
        ),
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn a_path_outside_the_workspace_is_refused_without_running_comfort() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let result =
        markdown_format_impl(&ctx, Some(paths(&["../elsewhere/notes.md"])), &runner).unwrap();
    match result {
        Outcome::Error { message, .. } => {
            assert_eq!(message, "Path must not escape the workspace root.");
        }
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn one_bad_path_refuses_the_whole_request() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let result = markdown_format_impl(
        &ctx,
        Some(paths(&["docs/usage.md", "crates/jp_cli/src/main.rs"])),
        &runner,
    )
    .unwrap();
    match result {
        Outcome::Error { message, .. } => assert!(
            message.starts_with("'crates/jp_cli/src/main.rs' is not a Markdown file"),
            "got: {message}"
        ),
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn comfort_failure_is_reported() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("comfort")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "comfort: parse error".to_owned(),
            status: ExitCode::from_code(2),
        });

    let result = markdown_format_impl(&ctx, None, &runner).unwrap();
    match result {
        Outcome::Error { message, .. } => {
            assert_eq!(message, "comfort failed: comfort: parse error");
        }
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn formatted_file_listing_is_bounded() {
    let (_dir, ctx) = ctx();
    let stdout = (0..4_000)
        .map(|i| format!("{root}/docs/generated/file_{i}.md", root = ctx.root))
        .collect::<Vec<_>>()
        .join("\n");
    let runner = MockProcessRunner::builder()
        .expect("comfort")
        .returns_success(stdout);

    let content = markdown_format_impl(&ctx, None, &runner)
        .unwrap()
        .unwrap_content();

    assert!(
        content.len() < MAX_LISTING_BYTES + 200,
        "listing grew to {} bytes",
        content.len()
    );
    assert!(
        content.contains("[Truncated: showing"),
        "got tail: {}",
        &content[content.len() - 100..]
    );
}
