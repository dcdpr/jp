use camino_tempfile::Utf8TempDir;

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

    create(&path, "first\n").unwrap();
    let error = create(&path, "second\n").unwrap_err();

    assert_eq!(error, format!("{path} already exists."));
    assert_eq!(read(&path).unwrap(), "first\n");
}
