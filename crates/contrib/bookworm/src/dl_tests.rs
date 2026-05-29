use std::fs;

use camino_tempfile::tempdir;

use super::*;

/// Build a fake docs.rs extraction layout under `root`.
///
/// Each `(dir, has_index)` tuple creates `root/<dir>/` and, when `has_index` is
/// true, also creates `root/<dir>/index.html` with a placeholder body.
/// Used by the sanitize tests to assert which kinds of directories survive
/// based purely on the presence of `index.html`.
fn populate(root: &Path, entries: &[(&str, bool)]) {
    for (dir, has_index) in entries {
        let path = root.join(dir);
        fs::create_dir_all(&path).expect("create dir");
        if *has_index {
            fs::write(path.join("index.html"), b"<html></html>").expect("write index.html");
        }
    }
}

#[test]
fn keeps_crate_docs_directory_with_index_html() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().as_std_path();
    populate(root, &[("serde_json", true)]);

    sanitize(root).expect("sanitize");

    assert!(root.join("serde_json").is_dir());
    assert!(root.join("serde_json/index.html").is_file());
}

#[test]
fn keeps_hyphenated_crate_docs_directory() {
    // Regression: `ra-ap-rustc_lexer` has docs under `ra_ap_rustc_lexer/`
    // because cargo replaces `-` with `_` in the lib name. The old
    // name-based sanitize deleted this directory.
    let dir = tempdir().expect("tempdir");
    let root = dir.path().as_std_path();
    populate(root, &[("ra_ap_rustc_lexer", true)]);

    sanitize(root).expect("sanitize");

    assert!(
        root.join("ra_ap_rustc_lexer").is_dir(),
        "hyphenated-crate docs directory must be preserved"
    );
}

#[test]
fn keeps_directory_with_custom_lib_name() {
    // A crate published as `foo` may declare `[lib] name = "fooz"`, in which
    // case rustdoc emits its docs under `fooz/`. The structural detection
    // doesn't care about the crates.io name, only about `index.html`.
    let dir = tempdir().expect("tempdir");
    let root = dir.path().as_std_path();
    populate(root, &[("fooz", true)]);

    sanitize(root).expect("sanitize");

    assert!(root.join("fooz").is_dir());
}

#[test]
fn keeps_src_and_implementors_without_index_html() {
    // `src/` and `implementors/` are part of rustdoc's output layout but
    // do not have a top-level `index.html`. They are kept by allow-list.
    let dir = tempdir().expect("tempdir");
    let root = dir.path().as_std_path();
    populate(root, &[("src", false), ("implementors", false)]);

    sanitize(root).expect("sanitize");

    assert!(root.join("src").is_dir());
    assert!(root.join("implementors").is_dir());
}

#[test]
fn removes_target_triple_directory_without_index_html() {
    // Multi-platform docsets nest each platform's full layout under a
    // target-triple directory. The triple directory itself has no direct
    // `index.html` (its child crate directory does), so it is removed.
    let dir = tempdir().expect("tempdir");
    let root = dir.path().as_std_path();
    populate(root, &[
        ("serde_json", true),
        ("x86_64-unknown-linux-gnu", false),
        ("x86_64-unknown-linux-gnu/serde_json", true),
    ]);

    sanitize(root).expect("sanitize");

    assert!(
        root.join("serde_json").is_dir(),
        "default-platform docs kept"
    );
    assert!(
        !root.join("x86_64-unknown-linux-gnu").exists(),
        "target-triple directory removed"
    );
}

#[test]
fn removes_arbitrary_directory_without_index_html() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().as_std_path();
    populate(root, &[("nuisance", false)]);

    sanitize(root).expect("sanitize");

    assert!(!root.join("nuisance").exists());
}

#[test]
fn leaves_top_level_files_alone() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().as_std_path();
    fs::write(root.join("help.html"), b"help").expect("write file");
    fs::write(root.join("settings.html"), b"settings").expect("write file");

    sanitize(root).expect("sanitize");

    assert!(root.join("help.html").is_file());
    assert!(root.join("settings.html").is_file());
}

#[test]
fn realistic_docs_rs_layout() {
    // End-to-end: a directory structure resembling what `unzip` produces
    // from a real docs.rs archive for a hyphenated crate. Default-platform
    // docs, `src/`, files, and a multi-platform wrapper.
    let dir = tempdir().expect("tempdir");
    let root = dir.path().as_std_path();
    populate(root, &[
        ("ra_ap_rustc_lexer", true),
        ("src", false),
        ("implementors", false),
        ("wasm32-unknown-unknown", false),
        ("wasm32-unknown-unknown/ra_ap_rustc_lexer", true),
    ]);
    fs::write(root.join("help.html"), b"help").expect("write file");
    fs::write(root.join("settings.html"), b"settings").expect("write file");

    sanitize(root).expect("sanitize");

    assert!(root.join("ra_ap_rustc_lexer/index.html").is_file());
    assert!(root.join("src").is_dir());
    assert!(root.join("implementors").is_dir());
    assert!(!root.join("wasm32-unknown-unknown").exists());
    assert!(root.join("help.html").is_file());
    assert!(root.join("settings.html").is_file());
}
