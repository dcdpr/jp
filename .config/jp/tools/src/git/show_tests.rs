use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

fn sample_show_output() -> String {
    let header = "abc123full\0abc123\0Alice\x002024-06-01T10:00:00+00:00\0feat: add \
                  widget\n\nSplit widget into separate module.";
    let stats = "\n42\t10\tsrc/widget.rs\n5\t3\tsrc/lib.rs\n";

    format!("{header}{STAT_SEPARATOR}{stats}")
}

#[test]
fn parses_show_output() {
    let show = parse_show_output(&sample_show_output()).unwrap();

    assert_eq!(show.short_hash, "abc123");
    assert_eq!(show.author, "Alice");
    assert_eq!(
        show.message,
        "feat: add widget\n\nSplit widget into separate module."
    );
    assert_eq!(show.files.len(), 2);
    assert_eq!(show.files[0], FileStat {
        path: "src/widget.rs".into(),
        insertions: "42".into(),
        deletions: "10".into(),
    });
    assert_eq!(show.files[1], FileStat {
        path: "src/lib.rs".into(),
        insertions: "5".into(),
        deletions: "3".into(),
    });
}

#[test]
fn parses_binary_file_stat() {
    let header = "abc\0abc\0A\x002024-01-01\0msg";
    let stats = "\n-\t-\timage.png\n";
    let raw = format!("{header}{STAT_SEPARATOR}{stats}");

    let show = parse_show_output(&raw).unwrap();
    assert_eq!(show.files.len(), 1);
    assert_eq!(show.files[0].path, "image.png");
    assert_eq!(show.files[0].insertions, "-");
    assert_eq!(show.files[0].deletions, "-");
}

#[test]
fn parses_no_file_changes() {
    let header = "abc\0abc\0A\x002024-01-01\0empty commit";
    let raw = format!("{header}{STAT_SEPARATOR}\n");

    let show = parse_show_output(&raw).unwrap();
    assert!(show.files.is_empty());
}

#[test]
fn basic_show() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(sample_show_output());

    let content = git_show_impl(dir.path(), "abc123", &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();

    assert!(content.contains("  <short_hash>abc123</short_hash>"));
    assert!(content.contains("feat: add widget"));
    assert!(content.contains("    - src/widget.rs (+42,-10)"));
    assert!(content.contains("    - src/lib.rs (+5,-3)"));
}

#[test]
fn show_git_error() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::error("fatal: bad object abc");

    let outcome = git_show_impl(dir.path(), "abc", &runner, &[]).unwrap();
    assert!(outcome.into_content().is_none(), "expected error outcome");
}

#[test]
fn show_missing_separator_errors() {
    let result = parse_show_output("some garbage output without separator");
    assert!(result.is_err());
}

#[test]
fn file_stat_format_insertions_only() {
    let stat = FileStat {
        path: "foo.rs".into(),
        insertions: "5".into(),
        deletions: "0".into(),
    };
    assert_eq!(stat.to_string(), "- foo.rs (+5)");
}

#[test]
fn file_stat_format_deletions_only() {
    let stat = FileStat {
        path: "foo.rs".into(),
        insertions: "0".into(),
        deletions: "3".into(),
    };
    assert_eq!(stat.to_string(), "- foo.rs (-3)");
}

#[test]
fn file_stat_format_both() {
    let stat = FileStat {
        path: "foo.rs".into(),
        insertions: "10".into(),
        deletions: "2".into(),
    };
    assert_eq!(stat.to_string(), "- foo.rs (+10,-2)");
}

#[test]
fn file_stat_format_binary() {
    let stat = FileStat {
        path: "img.png".into(),
        insertions: "-".into(),
        deletions: "-".into(),
    };
    assert_eq!(stat.to_string(), "- img.png (binary)");
}
