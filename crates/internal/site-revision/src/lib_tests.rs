use std::fs;

use camino_tempfile::Utf8TempDir;

use super::*;

/// A site of two files, one of them nested.
fn site() -> Utf8TempDir {
    let dir = Utf8TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("loaders")).unwrap();
    fs::write(dir.path().join("config.mjs"), "export default {}\n").unwrap();
    fs::write(dir.path().join("loaders/rfds.data.js"), "load()\n").unwrap();

    dir
}

fn revision_of(dir: &Utf8TempDir) -> String {
    revision(dir.path().as_std_path()).unwrap()
}

#[test]
fn files_are_listed_by_path_below_the_site() {
    let dir = site();

    let paths: Vec<String> = files(dir.path().as_std_path())
        .unwrap()
        .into_iter()
        .map(|file| file.path)
        .collect();

    assert_eq!(paths, ["config.mjs", "loaders/rfds.data.js"]);
}

#[test]
fn the_revision_is_sixteen_hex_digits() {
    let revision = revision_of(&site());

    assert_eq!(revision.len(), 16);
    assert!(
        revision.chars().all(|c| c.is_ascii_hexdigit()),
        "{revision}"
    );
}

#[test]
fn the_same_files_give_the_same_revision() {
    assert_eq!(revision_of(&site()), revision_of(&site()));
}

#[test]
fn a_changed_file_changes_the_revision() {
    let dir = site();
    let before = revision_of(&dir);

    fs::write(dir.path().join("loaders/rfds.data.js"), "load(true)\n").unwrap();

    assert_ne!(revision_of(&dir), before);
}

/// Moving content between files is a different site, even when the bytes add up
/// to the same.
#[test]
fn a_renamed_file_changes_the_revision() {
    let dir = site();
    let before = revision_of(&dir);

    fs::rename(
        dir.path().join("config.mjs"),
        dir.path().join("fragment.mjs"),
    )
    .unwrap();

    assert_ne!(revision_of(&dir), before);
}

#[test]
fn an_added_file_changes_the_revision() {
    let dir = site();
    let before = revision_of(&dir);

    fs::write(dir.path().join("style.css"), "").unwrap();

    assert_ne!(revision_of(&dir), before);
}

/// What a file manager leaves behind is not part of the site.
#[test]
fn dotfiles_are_not_site_files() {
    let dir = site();
    let before = revision_of(&dir);

    fs::write(dir.path().join(".DS_Store"), "junk").unwrap();
    fs::create_dir_all(dir.path().join(".cache")).unwrap();
    fs::write(dir.path().join(".cache/entry"), "junk").unwrap();

    assert_eq!(revision_of(&dir), before);
    assert_eq!(files(dir.path().as_std_path()).unwrap().len(), 2);
}

#[test]
fn a_windows_checkout_agrees_with_a_unix_one() {
    let unix = site();
    let windows = site();
    fs::write(windows.path().join("config.mjs"), "export default {}\r\n").unwrap();

    assert_eq!(revision_of(&windows), revision_of(&unix));
}
