use camino_tempfile::Utf8TempDir;
use chrono::TimeZone as _;
use pretty_assertions::assert_eq;

use super::*;
use crate::store::Store;

#[test]
fn the_working_tree_changed_once_a_tracked_file_differs_from_the_latest_version() {
    let root = Utf8TempDir::new().unwrap();
    fs::write(root.path().join("a.md"), "one\n").unwrap();
    let at = Utc.with_ymd_and_hms(2026, 11, 17, 10, 0, 0).unwrap();
    let mut review = Review::new("t".to_owned(), "Jane".to_owned(), at, vec![
        "a.md".to_owned(),
        "b.md".to_owned(),
    ]);
    assert_eq!(changed(root.path(), &review), Ok(true));

    let blobs = Store::new(root.path().join("store")).blobs("R-1h4k2m9".parse().unwrap());
    record(root.path(), &mut review, &blobs, at).unwrap();
    assert_eq!(changed(root.path(), &review), Ok(false));

    fs::write(root.path().join("b.md"), "new\n").unwrap();
    assert_eq!(changed(root.path(), &review), Ok(true));
}

#[test]
fn the_working_tree_records_a_missing_file_as_absent() {
    let root = Utf8TempDir::new().unwrap();
    fs::write(root.path().join("a.md"), "one\n").unwrap();

    let contents = working_tree(root.path(), &["a.md".to_owned(), "gone.md".to_owned()]).unwrap();

    assert_eq!(
        contents,
        Contents::from([
            ("a.md".to_owned(), Some("one\n".to_owned())),
            ("gone.md".to_owned(), None),
        ])
    );
}

#[test]
fn a_file_that_is_not_utf8_is_refused() {
    let root = Utf8TempDir::new().unwrap();
    fs::write(root.path().join("image.png"), [0x89, 0x50, 0xff, 0xfe]).unwrap();

    assert_eq!(
        working_tree(root.path(), &["image.png".to_owned()]).unwrap_err(),
        format!(
            "{} is not UTF-8 text, which a review can't comment on.",
            root.path().join("image.png")
        )
    );
}

#[test]
fn a_base_program_that_cannot_run_is_an_error() {
    let root = Utf8TempDir::new().unwrap();
    let command = ["jp-review-test-no-such-program".to_owned()];

    let error = base(root.path(), &command, &["a.md".to_owned()]).unwrap_err();

    assert!(
        error.starts_with("The `base` command, `jp-review-test-no-such-program`, couldn't run: "),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn the_base_is_what_the_command_prints_for_each_path() {
    let root = Utf8TempDir::new().unwrap();
    fs::write(root.path().join("a.md"), "one\n").unwrap();
    let command = ["cat".to_owned(), "./{path}".to_owned()];

    let contents = base(root.path(), &command, &[
        "a.md".to_owned(),
        "new.md".to_owned(),
    ])
    .unwrap();

    // `cat` exits non-zero for the file it can't find, which has no base.
    assert_eq!(
        contents,
        Contents::from([
            ("a.md".to_owned(), Some("one\n".to_owned())),
            ("new.md".to_owned(), None),
        ])
    );
}
