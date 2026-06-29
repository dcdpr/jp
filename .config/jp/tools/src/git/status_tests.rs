use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn parses_porcelain_entries() {
    let stdout = " M src/foo.rs\n?? new.txt\nA  staged.rs\nR  old.rs -> new.rs\n";
    let entries = parse_status(stdout);

    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0], StatusEntry {
        code: " M".into(),
        path: "src/foo.rs".into(),
    });
    assert_eq!(entries[1], StatusEntry {
        code: "??".into(),
        path: "new.txt".into(),
    });
    assert_eq!(entries[2], StatusEntry {
        code: "A ".into(),
        path: "staged.rs".into(),
    });
    assert_eq!(entries[3], StatusEntry {
        code: "R ".into(),
        path: "old.rs -> new.rs".into(),
    });
}

#[test]
fn keeps_leading_space_in_path() {
    // Porcelain is a fixed `XY<space>PATH`; only the one separator after the
    // status code is stripped, so a path that itself begins with spaces must
    // survive intact (the dirty-tree check relies on the real path).
    let entries = parse_status("??   leading.rs\n");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0], StatusEntry {
        code: "??".into(),
        path: "  leading.rs".into(),
    });
}

#[test]
fn describes_codes() {
    assert_eq!(describe("??"), "untracked");
    assert_eq!(describe(" M"), "modified, unstaged");
    assert_eq!(describe("M "), "modified, staged");
    assert_eq!(describe("MM"), "modified, staged; modified, unstaged");
    assert_eq!(describe("A "), "added, staged");
    assert_eq!(describe("D "), "deleted, staged");
    assert_eq!(describe("R "), "renamed, staged");
}

#[test]
fn formats_clean_tree() {
    assert_eq!(format_status(&[]), "Working tree clean.");
}

#[test]
fn basic_status() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(" M src/foo.rs\n?? new.txt\n");

    let content = git_status_impl(dir.path(), &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();

    assert!(content.contains("- src/foo.rs (modified, unstaged)"));
    assert!(content.contains("- new.txt (untracked)"));
}

#[test]
fn requests_all_untracked_files() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "-c",
            "core.quotePath=false",
            "status",
            "--porcelain",
            "--untracked-files=all",
        ])
        .returns_success("");

    let _outcome = git_status_impl(dir.path(), &runner, &[]).unwrap();
}

#[test]
fn caps_untracked_but_always_shows_tracked() {
    // A large untracked directory must not bury the tracked edits the guard
    // exists to surface: tracked changes are shown in full, untracked are
    // capped with a count of the remainder.
    let mut entries = vec![StatusEntry {
        code: " M".into(),
        path: "src/keep.rs".into(),
    }];
    for i in 0..(MAX_UNTRACKED + 50) {
        entries.push(StatusEntry {
            code: "??".into(),
            path: format!("junk/{i}.tmp"),
        });
    }

    let out = format_status(&entries);

    assert!(
        out.contains("- src/keep.rs (modified, unstaged)"),
        "tracked change must always show"
    );
    assert!(out.contains("and 50 more untracked files not shown"));
    assert_eq!(out.matches("(untracked)").count(), MAX_UNTRACKED);
}

#[test]
fn clean_tree_via_runner() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success("");

    let content = git_status_impl(dir.path(), &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();

    assert_eq!(content, "Working tree clean.");
}

#[test]
fn status_git_error() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::error("fatal: not a git repository");

    let outcome = git_status_impl(dir.path(), &runner, &[]).unwrap();
    assert!(outcome.into_content().is_none(), "expected error outcome");
}
