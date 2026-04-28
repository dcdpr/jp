use std::fs;

use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

/// Compute the expected ID for a hunk so test fixtures stay readable
/// instead of hardcoding hex strings.
fn id_for(hunk: &str) -> String {
    super::super::hunk::hunk_id(hunk)
}

#[test]
fn multiple_hunks() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();
    let filename = "test_script.rs";

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

    // Index (original) content — context comes from here, not the working tree.
    let index_content = "fn main() {\n    {};\n    println!(\"Hello\");\n}\n";

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            filename,
        ])
        .returns_success(mock_diff)
        .expect("git")
        .args(&["show", ":test_script.rs"])
        .returns_success(index_content);

    let content =
        git_list_patches_impl(root, Some(vec![filename.to_string()].into()), &runner, &[])
            .unwrap()
            .into_content()
            .unwrap();

    let id_0 = id_for("@@ -1 +1 @@\n-fn main() {\n+fn main() -> () {");
    let id_1 = id_for("@@ -3 +3 @@\n-    println!(\"Hello\");\n+    println!(\"Hello World\");");

    let expected = format!(
        "<patches>
    <patch>
        <path>test_script.rs</path>
        <id>{id_0}</id>
        <diff>
            [0] -fn main() {{
            [1] +fn main() -> () {{
                     {{}};
                     println!(\"Hello\");
                 }}
        </diff>
    </patch>
    <patch>
        <path>test_script.rs</path>
        <id>{id_1}</id>
        <diff>
                 fn main() {{
                     {{}};
            [0] -    println!(\"Hello\");
            [1] +    println!(\"Hello World\");
                 }}
        </diff>
    </patch>
</patches>"
    );

    assert_eq!(content, expected);
}

#[test]
fn single_hunk() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();
    let filename = "simple.rs";

    let mock_diff = indoc::indoc! {r"
        diff --git a/simple.rs b/simple.rs
        index abc123..def456 100644
        --- a/simple.rs
        +++ b/simple.rs
        @@ -2 +2 @@
        -    0
        +    42
    "};

    let index_content = "fn foo() -> i32 {\n    0\n}\n";

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            filename,
        ])
        .returns_success(mock_diff)
        .expect("git")
        .args(&["show", ":simple.rs"])
        .returns_success(index_content);

    let content =
        git_list_patches_impl(root, Some(vec![filename.to_string()].into()), &runner, &[])
            .unwrap()
            .into_content()
            .unwrap();

    let id = id_for("@@ -2 +2 @@\n-    0\n+    42");
    let expected = format!(
        "<patches>
    <patch>
        <path>simple.rs</path>
        <id>{id}</id>
        <diff>
                 fn foo() -> i32 {{
            [0] -    0
            [1] +    42
                 }}
        </diff>
    </patch>
</patches>"
    );

    assert_eq!(content, expected);
}

#[test]
fn no_changes() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();
    let filename = "unchanged.rs";

    fs::write(root.join(filename), "fn main() {}\n").unwrap();

    // Diff returns empty, so git show is never called.
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            filename,
        ])
        .returns_success("");

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

    let index_content = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";

    let mock_diff = indoc::indoc! {r"
        diff --git a/context.rs b/context.rs
        index abc..def 100644
        --- a/context.rs
        +++ b/context.rs
        @@ -5 +5 @@
        -line5
        +MODIFIED
    "};

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            filename,
        ])
        .returns_success(mock_diff)
        .expect("git")
        .args(&["show", ":context.rs"])
        .returns_success(index_content);

    let content =
        git_list_patches_impl(root, Some(vec![filename.to_string()].into()), &runner, &[])
            .unwrap()
            .into_content()
            .unwrap();

    let id = id_for("@@ -5 +5 @@\n-line5\n+MODIFIED");
    // Context lines get `[N] ` (4 chars) + ` ` = 5 spaces of padding,
    // aligning the content with what follows `-`/`+` on diff lines.
    let expected = format!(
        "<patches>
    <patch>
        <path>context.rs</path>
        <id>{id}</id>
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
</patches>"
    );

    assert_eq!(content, expected);
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
    // exist on disk → warning. exists.rs: returns actual diff + index content.
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
        .returns_success(mock_diff)
        .expect("git")
        .args(&["show", ":exists.rs"])
        .returns_success("fn main() {}\n");

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

#[test]
fn pure_insertion_context_from_index() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();
    let filename = "insert.rs";

    // Index has 4 lines, working tree inserts a new line after line 2.
    let index_content = "aaa\nbbb\nccc\nddd\n";

    let mock_diff = indoc::indoc! {r"
        diff --git a/insert.rs b/insert.rs
        index abc..def 100644
        --- a/insert.rs
        +++ b/insert.rs
        @@ -2,0 +3 @@
        +NEW
    "};

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            filename,
        ])
        .returns_success(mock_diff)
        .expect("git")
        .args(&["show", ":insert.rs"])
        .returns_success(index_content);

    let content =
        git_list_patches_impl(root, Some(vec![filename.to_string()].into()), &runner, &[])
            .unwrap()
            .into_content()
            .unwrap();

    let id = id_for("@@ -2,0 +3 @@\n+NEW");
    // Pure insertion after line 2: context before includes lines 1-2,
    // context after includes lines 3-4. The NEW line goes between them.
    let expected = format!(
        "<patches>
    <patch>
        <path>insert.rs</path>
        <id>{id}</id>
        <diff>
                 aaa
                 bbb
            [0] +NEW
                 ccc
                 ddd
        </diff>
    </patch>
</patches>"
    );

    assert_eq!(content, expected);
}

#[test]
fn ids_are_stable_across_unrelated_index_mutations() {
    // The point of content-addressed IDs: staging hunk A should not
    // renumber hunk B. We verify this by listing the same diff twice and
    // confirming each hunk's ID is invariant.
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path();
    let filename = "f.rs";

    let mock_diff = indoc::indoc! {r"
        diff --git a/f.rs b/f.rs
        --- a/f.rs
        +++ b/f.rs
        @@ -1 +1 @@
        -a
        +A
        @@ -5 +5 @@
        -e
        +E
    "};

    let index_content = "a\nb\nc\nd\ne\nf\n";

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            filename,
        ])
        .returns_success(mock_diff)
        .expect("git")
        .args(&["show", ":f.rs"])
        .returns_success(index_content);

    let content =
        git_list_patches_impl(root, Some(vec![filename.to_string()].into()), &runner, &[])
            .unwrap()
            .into_content()
            .unwrap();

    let expected_id_first = id_for("@@ -1 +1 @@\n-a\n+A");
    let expected_id_second = id_for("@@ -5 +5 @@\n-e\n+E");

    assert!(
        content.contains(&format!("<id>{expected_id_first}</id>")),
        "got: {content}"
    );
    assert!(
        content.contains(&format!("<id>{expected_id_second}</id>")),
        "got: {content}"
    );
    // IDs are obviously different from each other.
    assert_ne!(expected_id_first, expected_id_second);
}
