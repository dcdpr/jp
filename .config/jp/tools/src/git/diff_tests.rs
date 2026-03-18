use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn unstaged_diff() {
    let dir = tempdir().unwrap();

    let diff = indoc::indoc! {"
            diff --git a/test.rs b/test.rs
            index abc123..def456 100644
            --- a/test.rs
            +++ b/test.rs
            @@ -1 +1 @@
            -old line
            +new line"
    };

    let runner = MockProcessRunner::success(diff);
    let content = git_diff_impl(
        dir.path(),
        &["test.rs".to_string()],
        DiffStatus::Unstaged,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert_eq!(content, indoc::indoc! {"
            <git_diff>
                <output>
                    diff --git a/test.rs b/test.rs
                    index abc123..def456 100644
                    --- a/test.rs
                    +++ b/test.rs
                    @@ -1 +1 @@
                    -old line
                    +new line
                </output>
            </git_diff>"});
}

#[test]
fn staged_diff() {
    let dir = tempdir().unwrap();

    let runner = MockProcessRunner::success("no changes");

    let result = git_diff_impl(
        dir.path(),
        &["test.rs".to_string()],
        DiffStatus::Staged,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert_eq!(result, indoc::indoc! {"
            <git_diff>
                <output>no changes</output>
            </git_diff>"
    });
}

#[test]
fn parse_status_valid() {
    assert_eq!(DiffStatus::parse("all").unwrap(), DiffStatus::All);
    assert_eq!(DiffStatus::parse("staged").unwrap(), DiffStatus::Staged);
    assert_eq!(DiffStatus::parse("unstaged").unwrap(), DiffStatus::Unstaged);
}

#[test]
fn parse_status_invalid() {
    let err = DiffStatus::parse("bogus").unwrap_err();
    assert!(err.contains("bogus"), "error should mention the bad value");
}
