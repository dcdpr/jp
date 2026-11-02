use camino_tempfile::Utf8TempDir;
use jp_tool::FsRule;
use serde_json::json;

use super::*;
use crate::options::Options;

/// A workspace with two RFDs, a draft, and a ticket.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    for (path, text) in [
        (
            "docs/rfd/042-ordered.md",
            "# RFD 042: Ordered\n\n- **Status**: Accepted (2026-08-05)\n",
        ),
        (
            "docs/rfd/033-old.md",
            "# RFD 033: Old\n\n- **Status**: Superseded\n",
        ),
        ("docs/rfd/000-design-template.md", "# RFD NNN: TITLE\n"),
        (
            "docs/rfd/drafts/D05-sketch.md",
            "# RFD D05: Sketch\n\n- **Status**: Draft\n",
        ),
        (
            "docs/ticket/02wt0kx-a-ticket.md",
            "# A ticket\n\n- **Status**: Todo\n",
        ),
    ] {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    root
}

fn load(root: &Utf8Path, access: &Access) -> Library {
    let paths = Paths::new(root, &json!({}), &Options::default()).unwrap();

    Library::load(&paths, access)
}

#[test]
fn every_document_is_read_with_its_title_and_status() {
    let root = workspace();

    let library = load(root.path(), &Access::Unrestricted);

    assert_eq!(library.rfds.keys().collect::<Vec<_>>(), [
        "033", "042", "D05"
    ]);
    let Lookup::Found(rfd) = library.rfd("042") else {
        panic!("RFD 042 is there");
    };
    assert_eq!(rfd.title, "Ordered");
    assert_eq!(rfd.status.as_deref(), Some("Accepted"));
    assert!(
        matches!(library.ticket("T-02wt0kx"), Lookup::Found(ticket) if ticket.title == "A ticket")
    );
    assert!(matches!(library.rfd("043"), Lookup::Missing));
    assert_eq!(library.unread().count(), 0);
}

/// A document the policy keeps from the call exists, under its file name, and
/// its text is never read.
#[test]
fn a_document_the_policy_hides_is_known_by_its_file_name() {
    let root = workspace();
    let access = Access::granting(root.path(), vec![
        FsRule::new("docs").with_read(true),
        FsRule::new("docs/rfd/042-ordered.md").with_read(false),
    ]);

    let library = load(root.path(), &access);

    let Lookup::Found(rfd) = library.rfd("042") else {
        panic!("RFD 042 is there");
    };
    assert_eq!(rfd.title, "042-ordered");
    assert_eq!(rfd.status, None);
    assert_eq!(rfd.source, None);
    assert_eq!(library.unread().collect::<Vec<_>>(), [root
        .path()
        .join("docs/rfd/042-ordered.md")]);
}

/// A directory the policy doesn't let the call list leaves its documents
/// unknown, which a check tells apart from missing ones.
#[test]
fn a_directory_the_policy_hides_leaves_its_documents_unknown() {
    let root = workspace();
    let access = Access::granting(root.path(), vec![FsRule::new("docs/rfd").with_read(true)]);

    let library = load(root.path(), &access);

    assert!(matches!(library.rfd("042"), Lookup::Found(_)));
    assert!(matches!(library.rfd("043"), Lookup::Missing));
    assert!(matches!(library.ticket("T-02wt0kx"), Lookup::Unknown));
    assert_eq!(library.unlisted, [root.path().join("docs/ticket")]);
}
