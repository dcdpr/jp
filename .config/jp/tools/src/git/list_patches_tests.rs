use std::fs;

use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn multiple_hunks() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();
    let filename = "test_script.rs";

    let modified_content = "fn main() -> () {\n    {};\n    println!(\"Hello World\");\n}\n";
    fs::write(root.join(filename), modified_content).unwrap();

    let mock_diff = indoc::indoc! {r#"
        diff --git a/test_script.rs b/test_script.rs
        index 1234567..abcdefg 100644
        --- a/test_script.rs
        +++ b/test_script.rs
        @@ -1 +1 @@
        -fn main() {
        +fn main() -> () {
        @@ -3 +3 @@
        -    println!("Hello");
        +    println!("Hello World");
    "#};

    let runner = MockProcessRunner::success(mock_diff);
    let content =
        git_list_patches_impl(root, Some(vec![filename.to_string()].into()), &runner, &[])
            .unwrap()
            .into_content()
            .unwrap();

    assert_eq!(content, indoc::indoc! {r#"
        <patches>
            <patch>
                <path>test_script.rs</path>
                <id>0</id>
                <diff>
                    [0] -fn main() {
                    [1] +fn main() -> () {
                             {};
                             println!("Hello World");
                         }
                </diff>
            </patch>
            <patch>
                <path>test_script.rs</path>
                <id>1</id>
                <diff>
                         fn main() -> () {
                             {};
                    [0] -    println!("Hello");
                    [1] +    println!("Hello World");
                         }
                </diff>
            </patch>
        </patches>"#});
}

#[test]
fn single_hunk() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();
    let filename = "simple.rs";

    let content = "fn foo() -> i32 {\n    42\n}\n";
    fs::write(root.join(filename), content).unwrap();

    let mock_diff = indoc::indoc! {r"
        diff --git a/simple.rs b/simple.rs
        index abc123..def456 100644
        --- a/simple.rs
        +++ b/simple.rs
        @@ -2 +2 @@
        -    0
        +    42
    "};

    let runner = MockProcessRunner::success(mock_diff);
    let content = git_list_patches_impl(
        root,
        Some(vec!["simple.rs".to_string()].into()),
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert_eq!(content, indoc::indoc! {"
        <patches>
            <patch>
                <path>simple.rs</path>
                <id>0</id>
                <diff>
                         fn foo() -> i32 {
                    [0] -    0
                    [1] +    42
                         }
                </diff>
            </patch>
        </patches>"});
}

#[test]
fn no_changes() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();
    let filename = "unchanged.rs";

    fs::write(root.join(filename), "fn main() {}\n").unwrap();

    let runner = MockProcessRunner::success("");
    let content =
        git_list_patches_impl(root, Some(vec![filename.to_string()].into()), &runner, &[])
            .unwrap()
            .into_content()
            .unwrap();

    assert_eq!(content, "<patches>\n</patches>");
}

#[test]
fn git_command_fails_produces_warning() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();
    let filename = "error.rs";

    fs::write(root.join(filename), "fn main() {}\n").unwrap();

    let runner = MockProcessRunner::error("fatal: not a git repository");

    let content =
        git_list_patches_impl(root, Some(vec![filename.to_string()].into()), &runner, &[])
            .unwrap()
            .into_content()
            .unwrap();

    assert!(content.contains("Failed to list patches"));
    assert!(content.contains("fatal: not a git repository"));
}

#[test]
fn context_lines_aligned_with_diff_lines() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();
    let filename = "context.rs";

    let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
    fs::write(root.join(filename), content).unwrap();

    let mock_diff = indoc::indoc! {r"
        diff --git a/context.rs b/context.rs
        index abc..def 100644
        --- a/context.rs
        +++ b/context.rs
        @@ -5 +5 @@
        -line5
        +MODIFIED
    "};

    let runner = MockProcessRunner::success(mock_diff);
    let content =
        git_list_patches_impl(root, Some(vec![filename.to_string()].into()), &runner, &[])
            .unwrap()
            .into_content()
            .unwrap();

    // Context lines get `[N] ` (4 chars) + ` ` = 5 spaces of padding,
    // aligning the content with what follows `-`/`+` on diff lines.
    assert_eq!(content, indoc::indoc! {"
        <patches>
            <patch>
                <path>context.rs</path>
                <id>0</id>
                <diff>
                         line2
                         line3
                         line4
                    [0] -line5
                    [1] +MODIFIED
                         line6
                         line7
                         line8
                </diff>
            </patch>
        </patches>"});
}

#[test]
fn missing_file_produces_warning_not_error() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();

    fs::write(root.join("exists.rs"), "fn main() {}\n").unwrap();

    let mock_diff = indoc::indoc! {r"
        diff --git a/exists.rs b/exists.rs
        index abc..def 100644
        --- a/exists.rs
        +++ b/exists.rs
        @@ -1 +1 @@
        -fn main() {}
        +fn main() { todo!() }
    "};

    // missing.rs: git diff-files returns empty (unknown to git), file doesn't
    // exist on disk → warning. exists.rs: returns actual diff.
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            "missing.rs",
        ])
        .returns_success("")
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            "exists.rs",
        ])
        .returns_success(mock_diff);

    let content = git_list_patches_impl(
        root,
        Some(vec!["missing.rs".to_string(), "exists.rs".to_string()].into()),
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("File not found: missing.rs"));
    assert!(content.contains("<path>exists.rs</path>"));
}
