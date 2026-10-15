use camino_tempfile::Utf8TempDir;

use super::*;

fn owned(path: &str, contents: &str) -> Owned {
    Owned {
        path: Utf8PathBuf::from(path),
        contents: contents.as_bytes().to_vec(),
    }
}

fn write(root: &Utf8TempDir, path: &str, contents: &str) {
    let path = root.path().join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn paths(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .map(|change| match change {
            Change::Write(owned) => format!("write {}", owned.path),
            Change::Remove(path) => format!("remove {path}"),
        })
        .collect()
}

const OWNED: &str = "docs/.vitepress/jp-vite";

#[test]
fn a_fresh_project_gets_every_file() {
    let root = Utf8TempDir::new().unwrap();

    let changes = plan(
        root.path(),
        vec![
            owned("docs/rfd/index.md", "Index"),
            owned("docs/.vitepress/jp-vite/config.mjs", "Config"),
        ],
        Utf8Path::new(OWNED),
    )
    .unwrap();

    assert_eq!(paths(&changes), [
        "write docs/.vitepress/jp-vite/config.mjs",
        "write docs/rfd/index.md",
    ]);
}

#[test]
fn a_file_already_written_is_left_alone() {
    let root = Utf8TempDir::new().unwrap();
    write(&root, "docs/.vitepress/jp-vite/config.mjs", "Config");
    write(&root, "docs/rfd/index.md", "Old index");

    let changes = plan(
        root.path(),
        vec![
            owned("docs/rfd/index.md", "Index"),
            owned("docs/.vitepress/jp-vite/config.mjs", "Config"),
        ],
        Utf8Path::new(OWNED),
    )
    .unwrap();

    assert_eq!(paths(&changes), ["write docs/rfd/index.md"]);
}

/// The owned directory is the binary's whole: a file a newer release dropped
/// goes, and what an editor leaves behind stays.
#[test]
fn a_file_the_binary_no_longer_has_is_removed() {
    let root = Utf8TempDir::new().unwrap();
    write(&root, "docs/.vitepress/jp-vite/config.mjs", "Config");
    write(&root, "docs/.vitepress/jp-vite/loaders/old.mjs", "Old");
    write(&root, "docs/.vitepress/jp-vite/.DS_Store", "Junk");
    write(&root, "docs/.vitepress/config.mts", "The project's own");

    let changes = plan(
        root.path(),
        vec![owned("docs/.vitepress/jp-vite/config.mjs", "Config")],
        Utf8Path::new(OWNED),
    )
    .unwrap();

    assert_eq!(paths(&changes), [
        "remove docs/.vitepress/jp-vite/loaders/old.mjs"
    ]);
}

#[test]
fn applying_the_plan_writes_and_removes() {
    let root = Utf8TempDir::new().unwrap();
    write(&root, "docs/.vitepress/jp-vite/loaders/old.mjs", "Old");
    write(&root, "docs/rfd/index.md", "Old index");

    let changes = plan(
        root.path(),
        vec![
            owned("docs/rfd/index.md", "Index"),
            owned("docs/rfd/drafts/index.md", "Drafts"),
            owned("docs/.vitepress/jp-vite/config.mjs", "Config"),
        ],
        Utf8Path::new(OWNED),
    )
    .unwrap();
    apply(root.path(), &changes, Utf8Path::new(OWNED)).unwrap();

    let read = |path: &str| fs::read_to_string(root.path().join(path)).unwrap();
    assert_eq!(read("docs/rfd/index.md"), "Index");
    assert_eq!(read("docs/rfd/drafts/index.md"), "Drafts");
    assert_eq!(read("docs/.vitepress/jp-vite/config.mjs"), "Config");
    assert!(!root.path().join("docs/.vitepress/jp-vite/loaders").exists());
    assert!(root.path().join("docs/.vitepress/jp-vite").is_dir());

    let again = plan(
        root.path(),
        vec![
            owned("docs/rfd/index.md", "Index"),
            owned("docs/rfd/drafts/index.md", "Drafts"),
            owned("docs/.vitepress/jp-vite/config.mjs", "Config"),
        ],
        Utf8Path::new(OWNED),
    )
    .unwrap();
    assert_eq!(again, []);
}
