use std::fs;

use camino_tempfile::Utf8TempDir;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn a_typed_path_is_made_relative_to_the_root() {
    let root = Utf8Path::new("/work");

    assert_eq!(
        relative(root, root, "docs/a.md").as_deref(),
        Some("docs/a.md")
    );
    assert_eq!(
        relative(root, Utf8Path::new("/work/docs"), "./a.md").as_deref(),
        Some("docs/a.md")
    );
    assert_eq!(
        relative(root, Utf8Path::new("/work/docs"), "../src/../README.md").as_deref(),
        Some("README.md")
    );
    assert_eq!(
        relative(root, Utf8Path::new("/elsewhere"), "/work/docs/a.md").as_deref(),
        Some("docs/a.md")
    );
}

#[test]
fn a_path_outside_the_root_is_none() {
    let root = Utf8Path::new("/work");

    assert_eq!(relative(root, root, "../other/a.md"), None);
    assert_eq!(relative(root, root, "/other/a.md"), None);
    assert_eq!(relative(root, root, "."), None);
    assert_eq!(relative(root, Utf8Path::new("/"), "../../a.md"), None);
}

#[test]
fn an_existing_file_is_found_from_the_current_directory_first() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs")).unwrap();
    fs::write(root.path().join("docs/a.md"), "").unwrap();
    fs::write(root.path().join("a.md"), "").unwrap();
    let docs = root.path().join("docs");

    assert_eq!(existing(root.path(), &docs, "a.md").unwrap(), "docs/a.md");
    assert_eq!(
        existing(root.path(), &docs, "docs/a.md").unwrap(),
        "docs/a.md"
    );
    assert_eq!(existing(root.path(), root.path(), "a.md").unwrap(), "a.md");
}

#[test]
fn a_missing_or_outside_file_is_refused() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs")).unwrap();

    assert_eq!(
        existing(root.path(), root.path(), "missing.md").unwrap_err(),
        "No file at missing.md."
    );
    assert_eq!(
        existing(root.path(), root.path(), "docs").unwrap_err(),
        "No file at docs."
    );
    assert_eq!(
        existing(root.path(), root.path(), "/elsewhere/a.md").unwrap_err(),
        "/elsewhere/a.md is outside the workspace."
    );
}

/// A tracked file needn't exist: it may be absent from the working tree.
#[test]
fn a_tracked_path_is_found_from_either_directory() {
    let root = Utf8Path::new("/work");
    let docs = Utf8Path::new("/work/docs");
    let files = ["docs/a.md".to_owned()];

    assert_eq!(
        tracked(root, docs, &files, "a.md").as_deref(),
        Some("docs/a.md")
    );
    assert_eq!(
        tracked(root, docs, &files, "docs/a.md").as_deref(),
        Some("docs/a.md")
    );
    assert_eq!(tracked(root, docs, &files, "b.md"), None);
}
