use camino_tempfile::Utf8TempDir;

use super::*;

const DATE: &str = "2026-08-05";
const STAMP: &str = "2026-08-05T14:03:11Z";

fn new_ticket(dir: &Utf8TempDir, title: &str) -> TicketId {
    create(
        dir.path(),
        Kind::Bug,
        title,
        "jean",
        DATE,
        None,
        "Description.",
    )
    .unwrap()
    .0
}

#[test]
fn create_writes_a_numbered_file() {
    let dir = Utf8TempDir::new().unwrap();

    let (id, path) = create(
        dir.path(),
        Kind::Bug,
        "Tool call header misaligned",
        "Jean Mertz",
        DATE,
        None,
        "The header renders one column left of the body.",
    )
    .unwrap();

    assert_eq!(id.to_string(), "T0001");
    assert_eq!(
        path.file_name(),
        Some("0001-tool-call-header-misaligned.md")
    );

    let ticket = parse::document(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(ticket.id, id);
    assert_eq!(ticket.title, "Tool call header misaligned");
    assert_eq!(ticket.metadata.status, Status::Todo);
    assert_eq!(ticket.metadata.kind, Kind::Bug);
}

#[test]
fn ids_increment() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(new_ticket(&dir, "First").number(), 1);
    assert_eq!(new_ticket(&dir, "Second").number(), 2);
    assert_eq!(new_ticket(&dir, "Third").number(), 3);
}

/// The counter is the authority, so a deleted ticket's number stays retired.
#[test]
fn deleting_a_ticket_does_not_free_its_id() {
    let dir = Utf8TempDir::new().unwrap();

    new_ticket(&dir, "First");
    let second = locate(dir.path(), new_ticket(&dir, "Second")).unwrap();
    fs::remove_file(second).unwrap();

    assert_eq!(new_ticket(&dir, "Third").number(), 3);
}

/// A counter that lost an increment (a bad merge, a hand-created file) must
/// still not collide with what is on disk.
#[test]
fn a_stale_counter_does_not_collide() {
    let dir = Utf8TempDir::new().unwrap();

    new_ticket(&dir, "First");
    new_ticket(&dir, "Second");
    fs::write(dir.path().join(COUNTER), "1\n").unwrap();

    assert_eq!(new_ticket(&dir, "Third").number(), 3);
}

#[test]
fn comments_append_and_number_from_one() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "Tool call header misaligned");

    let first = append_comment(dir.path(), id, "jean", STAMP, None, "Reproduced at 72.").unwrap();
    let second = append_comment(dir.path(), id, "jp", STAMP, Some(1), "The wrap is off.").unwrap();

    assert_eq!(first, 1);
    assert_eq!(second, 2);

    let path = locate(dir.path(), id).unwrap();
    let ticket = parse::document(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(ticket.comments.len(), 2);
    assert_eq!(ticket.comments[0].from, "jean");
    assert_eq!(ticket.comments[1].re.as_deref(), Some("T0001#1"));
}

#[test]
fn replies_must_reference_an_existing_comment() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "Tool call header misaligned");

    let error = append_comment(dir.path(), id, "jp", STAMP, Some(1), "Reply.").unwrap_err();

    assert_eq!(error.to_string(), "T0001 has no comment #1 to reply to.");
}

#[test]
fn commenting_on_a_missing_ticket_fails() {
    let dir = Utf8TempDir::new().unwrap();

    let error = append_comment(dir.path(), TicketId::new(9), "jp", STAMP, None, "Hi.").unwrap_err();

    assert_eq!(error.to_string(), "No ticket T0009.");
}

#[test]
fn close_moves_the_ticket_to_done() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "Tool call header misaligned");

    let (path, previous) = close(dir.path(), id).unwrap();

    assert_eq!(previous, Status::Todo);
    let ticket = parse::document(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(ticket.metadata.status, Status::Done);
}

#[test]
fn closing_twice_is_a_no_op() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "Tool call header misaligned");

    close(dir.path(), id).unwrap();
    let before = fs::read_to_string(locate(dir.path(), id).unwrap()).unwrap();
    let (_, previous) = close(dir.path(), id).unwrap();
    let after = fs::read_to_string(locate(dir.path(), id).unwrap()).unwrap();

    assert_eq!(previous, Status::Done);
    assert_eq!(before, after);
}

#[test]
fn list_is_ordered_by_id_and_skips_other_files() {
    let dir = Utf8TempDir::new().unwrap();
    new_ticket(&dir, "First");
    new_ticket(&dir, "Second");
    fs::write(dir.path().join("notes.txt"), "not a ticket").unwrap();
    fs::write(dir.path().join("index.md"), "# Index\n").unwrap();

    let entries = list(dir.path()).unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].ticket.as_ref().unwrap().title, "First");
    assert_eq!(entries[1].ticket.as_ref().unwrap().title, "Second");
}

/// One mangled file shouldn't hide the rest of the board.
#[test]
fn list_reports_unreadable_tickets_alongside_the_rest() {
    let dir = Utf8TempDir::new().unwrap();
    new_ticket(&dir, "Readable");
    fs::write(dir.path().join("0009-mangled.md"), "no heading here\n").unwrap();

    let entries = list(dir.path()).unwrap();

    assert_eq!(entries.len(), 2);
    assert!(entries[0].ticket.is_ok());
    assert_eq!(entries[1].ticket, Err(ParseError::MissingTitle));
}

#[test]
fn list_of_a_missing_directory_is_empty() {
    let dir = Utf8TempDir::new().unwrap();

    assert!(list(&dir.path().join("nope")).unwrap().is_empty());
}

#[test]
fn an_import_creates_a_ticket_linked_to_its_issue() {
    let dir = Utf8TempDir::new().unwrap();

    let imported = import(dir.path(), &Import {
        number: 123,
        title: "Crash on empty input",
        description: "Running `jp q` with no argument panics.",
        comments: vec![Comment {
            from: "gh:someone".to_owned(),
            date: "2026-08-05T00:00:00Z".to_owned(),
            re: None,
            body: "Still happens on main.".to_owned(),
        }],
        kind: Kind::Bug,
        authors: "gh:someone",
        date: "2026-08-05",
    })
    .unwrap();

    assert!(imported.created);
    assert_eq!(imported.comments, 1);

    let ticket = parse::document(&fs::read_to_string(&imported.path).unwrap()).unwrap();
    assert_eq!(ticket.title, "Crash on empty input");
    assert_eq!(ticket.metadata.github.as_deref(), Some("#123"));
    assert_eq!(ticket.metadata.kind, Kind::Bug);
    assert_eq!(ticket.comments.len(), 1);
    assert_eq!(ticket.comments[0].from, "gh:someone");
}

/// GitHub owns the content; the repository owns the metadata.
/// A second import refreshes one and leaves the other alone.
#[test]
fn re_importing_replaces_content_and_keeps_triage() {
    let dir = Utf8TempDir::new().unwrap();
    let upstream = |title: &'static str, body: &'static str| Import {
        number: 123,
        title,
        description: body,
        comments: vec![],
        kind: Kind::Bug,
        authors: "gh:someone",
        date: "2026-08-05",
    };

    let first = import(dir.path(), &upstream("Crash", "First telling.")).unwrap();

    // Local triage between imports.
    let source = fs::read_to_string(&first.path).unwrap();
    let triaged = render::set_metadata(&source, "Status", "In Progress").unwrap();
    let triaged = render::set_metadata(&triaged, "Implements", "095").unwrap();
    fs::write(&first.path, triaged).unwrap();

    let second = import(dir.path(), &upstream("Crash, revised", "Second telling.")).unwrap();

    assert!(!second.created);
    assert_eq!(second.id, first.id);

    let ticket = parse::document(&fs::read_to_string(&second.path).unwrap()).unwrap();
    assert_eq!(ticket.title, "Crash, revised");
    assert_eq!(ticket.description, "Second telling.");
    assert_eq!(ticket.metadata.status, Status::InProgress);
    assert_eq!(ticket.metadata.implements.as_deref(), Some("095"));
    assert_eq!(ticket.metadata.github.as_deref(), Some("#123"));
}

#[test]
fn imported_content_is_escaped_on_the_way_in() {
    let dir = Utf8TempDir::new().unwrap();

    let imported = import(dir.path(), &Import {
        number: 7,
        title: "<script>x</script>",
        description: "Uses {{ interpolation }}.",
        comments: vec![],
        kind: Kind::Bug,
        authors: "gh:someone",
        date: "2026-08-05",
    })
    .unwrap();

    let source = fs::read_to_string(&imported.path).unwrap();
    assert!(!source.contains("<script"), "{source}");
    assert!(!source.contains("{{"), "{source}");
}

#[test]
fn slugs_are_lowercase_and_hyphenated() {
    assert_eq!(
        slug("Tool call header misaligned"),
        "tool-call-header-misaligned"
    );
    assert_eq!(slug("`jp query --new` panics!"), "jp-query-new-panics");
    assert_eq!(slug("  Spaced  out  "), "spaced-out");
    assert_eq!(slug("!!!"), "untitled");
    assert_eq!(slug(&"long ".repeat(20)).len(), 59);
}
