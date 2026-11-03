use camino_tempfile::Utf8TempDir;
use serde_json::json;

use super::*;

#[test]
fn directories_resolve_against_the_root_unless_absolute() {
    let paths = Paths::new(
        Utf8Path::new("/repo"),
        Utf8Path::new("docs/rfd"),
        Utf8Path::new("/elsewhere/tickets"),
    );

    assert_eq!(paths.rfds, "/repo/docs/rfd");
    assert_eq!(paths.drafts(), "/repo/docs/rfd/drafts");
    assert_eq!(paths.tickets, "/elsewhere/tickets");
    assert_eq!(
        paths.show(&paths.rfds.join("042-x.md")),
        "docs/rfd/042-x.md"
    );
}

fn paths(rfds: &str, tickets: &str) -> Paths {
    Paths::new(
        Utf8Path::new("/repo"),
        Utf8Path::new(rfds),
        Utf8Path::new(tickets),
    )
}

/// Documents link to an RFD from the tree its directory sits in, which in JP's
/// layout is `docs/`.
/// A rewrite never takes the whole workspace for that tree.
#[test]
fn the_docs_tree_is_the_rfd_directorys_parent_below_the_root() {
    assert_eq!(paths("docs/rfd", "docs/ticket").docs(), "/repo/docs");
    assert_eq!(paths("rfd", "docs/ticket").docs(), "/repo/rfd");
    assert_eq!(
        paths("/elsewhere/rfd", "docs/ticket").docs(),
        "/elsewhere/rfd"
    );
}

#[test]
fn links_are_looked_for_in_the_ticket_directory_outside_the_docs_tree() {
    assert_eq!(paths("docs/rfd", "docs/ticket").linking(), ["/repo/docs"]);
    assert_eq!(paths("docs/rfd", "tickets").linking(), [
        "/repo/docs",
        "/repo/tickets"
    ]);
}

#[test]
fn create_refuses_a_file_that_exists() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join("a.md");
    let paths = Paths::new(dir.path(), Utf8Path::new("rfd"), Utf8Path::new("ticket"));

    paths.create(&path, "first\n").unwrap();
    let error = paths.create(&path, "second\n").unwrap_err();

    assert_eq!(error, format!("{path} already exists."));
    assert_eq!(read(&path).unwrap(), "first\n");
}

/// The RFDs, the tickets, and the wiki pages each follow their own plugin's
/// `format_on_write`: a command here rewrites links in all three.
#[test]
fn writes_format_the_kinds_the_workspace_formats() {
    let root = Utf8TempDir::new().unwrap();
    for dir in ["docs/rfd", "docs/ticket", "docs/wiki"] {
        fs::create_dir_all(root.path().join(dir)).unwrap();
    }
    let config = json!({ "plugins": { "command": {
        "rfd": { "options": { "format_on_write": true } },
        "wiki": { "options": { "format_on_write": true } },
    } } });
    let paths = Paths::new(
        root.path(),
        Utf8Path::new("docs/rfd"),
        Utf8Path::new("docs/ticket"),
    )
    .formatted(&config)
    .unwrap();
    let text = "First sentence. Second sentence.\n";

    for file in [
        "docs/rfd/042-x.md",
        "docs/ticket/02wt0kx-x.md",
        "docs/wiki/0a1b2c3-x.md",
    ] {
        paths.write(&root.path().join(file), text).unwrap();
    }

    let read = |file: &str| read(&root.path().join(file)).unwrap();
    assert_eq!(
        read("docs/rfd/042-x.md"),
        "First sentence.\nSecond sentence.\n"
    );
    assert_eq!(read("docs/ticket/02wt0kx-x.md"), text);
    assert_eq!(
        read("docs/wiki/0a1b2c3-x.md"),
        "First sentence.\nSecond sentence.\n"
    );
}
