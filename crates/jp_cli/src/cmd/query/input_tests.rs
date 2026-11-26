use assert_matches::assert_matches;

use super::*;

#[test]
fn arg_file_path_recognizes_sigil() {
    assert_eq!(arg_file_path("@notes.md"), Some("notes.md"));
    assert_eq!(arg_file_path("@~/notes.md"), Some("~/notes.md"));
    assert_eq!(arg_file_path("notes.md"), None);
    assert_eq!(arg_file_path(""), None);
}

// A bare `@` is ordinary prose, not a reference to the empty path. Reading it
// as a path is what made `jp q ... drop the @ entirely ...` fail.
#[test]
fn arg_file_path_ignores_bare_sigil() {
    assert_eq!(arg_file_path("@"), None);
    assert_eq!(arg_file_path("@ "), None);
    assert_eq!(arg_file_path("@\t"), None);
}

#[test]
fn query_file_path_only_for_single_value_query() {
    let one = ["@notes.md".to_owned()];
    assert_eq!(query_file_path(&one), Some("notes.md"));

    let trailing = ["hello".to_owned(), "@notes.md".to_owned()];
    assert_eq!(query_file_path(&trailing), None);

    let leading = ["@notes.md".to_owned(), "extra".to_owned()];
    assert_eq!(query_file_path(&leading), None);

    let plain = ["hello".to_owned()];
    assert_eq!(query_file_path(&plain), None);

    assert_eq!(query_file_path(&[]), None);
}

#[test]
fn read_arg_file_returns_file_contents() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Notes\n\nbody\n").unwrap();

    assert_eq!(read_arg_file(path.as_str()).unwrap(), "# Notes\n\nbody\n");
}

#[test]
fn read_arg_file_error_names_the_path() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.md");

    let error = read_arg_file(path.as_str()).unwrap_err();

    assert_matches!(&error, Error::ArgFile { path: p, .. } if p == path.as_str());
    // The OS-supplied cause differs per platform, so pin the part we own: the
    // path is in the message clap renders, which is what "IO error" dropped.
    assert!(
        error
            .to_string()
            .starts_with(&format!("cannot read '{path}': ")),
        "unexpected message: {error}"
    );
}
