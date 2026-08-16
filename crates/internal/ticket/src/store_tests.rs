use camino_tempfile::Utf8TempDir;

use super::*;

const DATE: &str = "2026-08-05";
const STAMP: &str = "2026-08-05T14:03:11Z";

fn new_ticket(dir: &Utf8TempDir, title: &str) -> TicketId {
    create(
        dir.path(),
        Kind::Bug,
        title,
        "john",
        DATE,
        None,
        "Description.",
    )
    .unwrap()
    .0
}

#[test]
fn create_writes_a_file_named_for_its_id() {
    let dir = Utf8TempDir::new().unwrap();

    let (id, path) = create(
        dir.path(),
        Kind::Bug,
        "Tool call header misaligned",
        "John Doe",
        DATE,
        None,
        "The header renders one column left of the body.",
    )
    .unwrap();

    assert_eq!(
        path.file_name(),
        Some(format!("{}tool-call-header-misaligned.md", id.file_prefix()).as_str())
    );

    let ticket = parse::document(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(ticket.id, id);
    assert_eq!(ticket.title, "Tool call header misaligned");
    assert_eq!(ticket.metadata.status, Status::Todo);
    assert_eq!(ticket.metadata.kind, Kind::Bug);
}

/// Tickets created back to back land in one bucket, where the tail increments
/// rather than being redrawn.
/// A ticket that references an earlier one must never sort above it.
#[test]
fn ids_created_in_sequence_stay_ordered() {
    let dir = Utf8TempDir::new().unwrap();

    let first = new_ticket(&dir, "First");
    let second = new_ticket(&dir, "Second");
    let third = new_ticket(&dir, "Third");

    assert!(first < second, "{first} !< {second}");
    assert!(second < third, "{second} !< {third}");

    // Same bucket means the tail did the ordering, which is the case the
    // increment rule exists for. A test run straddling a bucket boundary is
    // still ordered, just by the time component.
    if first.bucket() == third.bucket() {
        assert_eq!(second.tail(), first.tail() + 1);
        assert_eq!(third.tail(), second.tail() + 1);
    }
}

/// Nothing records ids after their file is gone, so a deleted id can come back.
/// [RFD 102] accepts that in exchange for allocation needing no coordination.
///
/// [RFD 102]: ../../../../docs/rfd/102-collision-resistant-ticket-identifiers.md
#[test]
fn deleting_a_ticket_frees_its_id_within_the_bucket() {
    let dir = Utf8TempDir::new().unwrap();

    let first = new_ticket(&dir, "First");
    let second = new_ticket(&dir, "Second");
    fs::remove_file(locate(dir.path(), second).unwrap()).unwrap();
    let third = new_ticket(&dir, "Third");

    if first.bucket() == third.bucket() {
        assert_eq!(third, second, "the freed tail was not handed out again");
    }
}

/// An id from a machine with a fast clock must not drag later local ids into
/// its bucket, or one bad clock skews every timestamp after it.
#[test]
fn an_id_in_a_future_bucket_is_ignored() {
    let dir = Utf8TempDir::new().unwrap();
    let present = new_ticket(&dir, "Present");

    let future = TicketId::new(present.bucket() + 10_000, 5).unwrap();
    fs::write(
        dir.path()
            .join(format!("{}future.md", future.file_prefix())),
        render::ticket(future, "Future", Kind::Bug, "john", DATE, None, ""),
    )
    .unwrap();

    let next = new_ticket(&dir, "Next");

    assert!(next.bucket() < future.bucket());
    assert!(next > present);
}

/// Two files claiming one id leaves every reference to it ambiguous, so the
/// board refuses to render rather than pick one.
#[test]
fn list_rejects_a_duplicated_id() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "First");

    fs::write(
        dir.path().join(format!("{}second.md", id.file_prefix())),
        render::ticket(id, "Second", Kind::Bug, "john", DATE, None, ""),
    )
    .unwrap();

    let error = list(dir.path()).unwrap_err();

    assert!(
        error
            .to_string()
            .starts_with(&format!("{id} is claimed by more than one file: ")),
        "{error}"
    );
}

#[test]
fn comments_append_and_number_from_one() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "Tool call header misaligned");

    let first = append_comment(dir.path(), id, "john", STAMP, None, "Reproduced at 72.").unwrap();
    let second = append_comment(dir.path(), id, "jp", STAMP, Some(1), "The wrap is off.").unwrap();

    assert_eq!(first, 1);
    assert_eq!(second, 2);

    let path = locate(dir.path(), id).unwrap();
    let ticket = parse::document(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(ticket.comments.len(), 2);
    assert_eq!(ticket.comments[0].from, "john");
    assert_eq!(ticket.comments[1].re, Some(format!("{id}#1")));
}

#[test]
fn replies_must_reference_an_existing_comment() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "Tool call header misaligned");

    let error = append_comment(dir.path(), id, "jp", STAMP, Some(1), "Reply.").unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("{id} has no comment #1 to reply to.")
    );
}

#[test]
fn commenting_on_a_missing_ticket_fails() {
    let dir = Utf8TempDir::new().unwrap();

    let missing: TicketId = "T-zzzzzzz".parse().unwrap();

    let error = append_comment(dir.path(), missing, "jp", STAMP, None, "Hi.").unwrap_err();

    assert_eq!(error.to_string(), "No ticket T-zzzzzzz.");
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
    // Sorts last, so the readable ticket stays at index zero.
    fs::write(dir.path().join("zzzzzzz-mangled.md"), "no heading here\n").unwrap();

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

/// An edit rewrites the content and leaves everything else standing.
#[test]
fn editing_keeps_metadata_and_comments() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "Original title");
    append_comment(dir.path(), id, "john", STAMP, None, "Still relevant.").unwrap();
    set_field(dir.path(), id, "Status", "In Progress").unwrap();

    let path = edit(dir.path(), id, Some("Revised title"), Some("Revised body.")).unwrap();

    let ticket = parse::document(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(ticket.title, "Revised title");
    assert_eq!(ticket.description, "Revised body.");
    assert_eq!(ticket.metadata.status, Status::InProgress);
    assert_eq!(ticket.comments.len(), 1);
    assert_eq!(ticket.comments[0].body, "Still relevant.");
}

#[test]
fn editing_one_part_leaves_the_other() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "Original title");

    let path = edit(dir.path(), id, Some("Revised title"), None).unwrap();

    let ticket = parse::document(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(ticket.title, "Revised title");
    assert_eq!(ticket.description, "Description.");
}

#[test]
fn deleting_removes_the_file() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "Mistaken");

    let path = delete(dir.path(), id).unwrap();

    assert!(!path.exists());
    assert!(list(dir.path()).unwrap().is_empty());
    assert_eq!(
        delete(dir.path(), id).unwrap_err().to_string(),
        format!("No ticket {id}.")
    );
}

/// Two files claiming one id makes every write by that id ambiguous, so the
/// lookup refuses rather than landing on whichever the directory yielded first.
#[test]
fn a_write_by_a_duplicated_id_is_refused() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "First");

    fs::write(
        dir.path().join(format!("{}second.md", id.file_prefix())),
        render::ticket(id, "Second", Kind::Bug, "john", DATE, None, ""),
    )
    .unwrap();

    let error = close(dir.path(), id).unwrap_err();

    assert!(
        error
            .to_string()
            .starts_with(&format!("{id} is claimed by more than one file: ")),
        "{error}"
    );
}

/// `reassign` writes into the ticket directory and deletes the source, so a
/// path that merely parses as a heading must not reach either step.
#[test]
fn reassign_refuses_a_path_outside_the_ticket_directory() {
    let dir = Utf8TempDir::new().unwrap();
    let outside = dir.path().join("elsewhere");
    fs::create_dir_all(&outside).unwrap();

    // An RFD heading splits on `:` exactly like a ticket's does.
    let rfd = outside.join("102-collision-resistant-ticket-identifiers.md");
    fs::write(&rfd, "# RFD 102: Collision-Resistant Ticket Identifiers\n").unwrap();

    let error = reassign(dir.path(), &rfd, 4_200).unwrap_err();

    assert!(
        error
            .to_string()
            .ends_with("is not a ticket file in the ticket directory."),
        "{error}"
    );
    assert!(rfd.exists(), "the source file was removed");
}

/// A reassigned ticket keeps its slug and its content; only the id moves.
#[test]
fn reassign_renames_the_file_and_rewrites_the_heading() {
    let dir = Utf8TempDir::new().unwrap();
    let id = new_ticket(&dir, "Tool call header misaligned");
    let path = locate(dir.path(), id).unwrap();

    let done = reassign(dir.path(), &path, id.bucket() + 1).unwrap();

    assert_eq!(done.old, id.to_string());
    assert_ne!(done.new, id);
    assert!(!path.exists());
    assert_eq!(
        done.path.file_name(),
        Some(format!("{}tool-call-header-misaligned.md", done.new.file_prefix()).as_str())
    );

    let ticket = parse::document(&fs::read_to_string(&done.path).unwrap()).unwrap();
    assert_eq!(ticket.id, done.new);
    assert_eq!(ticket.title, "Tool call header misaligned");
    assert_eq!(ticket.description, "Description.");
}

/// A ticket written before the id format changed carries a heading no parser
/// accepts, so reassigning it must not go through one.
#[test]
fn reassign_converts_a_ticket_from_the_old_format() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join("0005-old-ticket.md");
    fs::write(
        &path,
        "# T0005: Old ticket\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Authors**: john\n- \
         **Date**: 2026-08-05\n\nBody.\n",
    )
    .unwrap();

    let done = reassign(dir.path(), &path, 4_200).unwrap();

    assert_eq!(done.old, "T0005");
    assert_eq!(done.new.bucket(), 4_200);
    assert!(!path.exists());

    let ticket = parse::document(&fs::read_to_string(&done.path).unwrap()).unwrap();
    assert_eq!(ticket.id, done.new);
    assert_eq!(ticket.title, "Old ticket");
    assert_eq!(ticket.description, "Body.");
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
