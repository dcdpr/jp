use camino_tempfile::Utf8TempDir;

use super::*;
use crate::Kind;

const DATE: &str = "2026-08-05";
const STAMP: &str = "2026-08-05T14:03:11Z";

/// A ticket with everything but the title fixed, so a test only spells out what
/// it cares about.
fn draft<'a>(title: &'a str, labels: &'a [Label]) -> NewTicket<'a> {
    NewTicket {
        kind: Kind::Bug,
        title,
        authors: "john",
        date: DATE,
        implements: None,
        labels,
        description: "Description.",
    }
}

fn new_ticket(dir: &Utf8TempDir, title: &str) -> TicketId {
    create(dir.path(), &draft(title, &[])).unwrap().0
}

/// A vocabulary written into a board, so label writes have something to check
/// against.
fn write_vocabulary(dir: &Utf8TempDir) {
    fs::write(
        dir.path().join(labels::FILE),
        r#"{
            "active": {"app/macos": "The macOS app.", "config": "Configuration."},
            "retired": {"legacy-ui": "The old UI."}
        }"#,
    )
    .unwrap();
}

fn owned(labels: &[&str]) -> Vec<String> {
    labels.iter().map(|label| (*label).to_owned()).collect()
}

#[test]
fn create_writes_a_file_named_for_its_id() {
    let dir = Utf8TempDir::new().unwrap();

    let (id, path) = create(dir.path(), &NewTicket {
        kind: Kind::Bug,
        title: "Tool call header misaligned",
        authors: "John Doe",
        date: DATE,
        implements: None,
        labels: &[],
        description: "The header renders one column left of the body.",
    })
    .unwrap();

    assert_eq!(
        path.file_name(),
        Some(format!("{}tool-call-header-misaligned.md", id.file_prefix()).as_str())
    );

    let ticket = parse::document(&fs::read_to_string(&path).unwrap()).unwrap();
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
        render::ticket(&draft("Future", &[])),
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
        render::ticket(&draft("Second", &[])),
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
    assert_eq!(ticket.comments[1].re.as_deref(), Some("#1"));
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
        render::ticket(&draft("Second", &[])),
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
    assert_eq!(ticket.title, "Tool call header misaligned");
    assert_eq!(ticket.description, "Description.");
}

/// Reassigning is a rename: the document is untouched, so a pre-RFD-102 heading
/// survives verbatim and stripping it is the migration's job.
#[test]
fn reassign_renames_a_legacy_file_without_touching_it() {
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

    let source = fs::read_to_string(&done.path).unwrap();
    assert!(source.starts_with("# T0005: Old ticket\n"), "{source}");

    let stripped = render::strip_ids(&source, &done.old);
    let ticket = parse::document(&stripped).unwrap();
    assert_eq!(ticket.title, "Old ticket");
    assert_eq!(ticket.description, "Body.");
}

/// A title that merely contains a colon is not an id prefix, and a reply naming
/// another ticket is not this one's to rewrite.
#[test]
fn stripping_ids_leaves_unrelated_colons_and_replies_alone() {
    let document = "# Fix: the wrap calculation\n\n- **Status**: Todo\n\n- **Re**: T0009#2\n";

    assert_eq!(render::strip_ids(document, "T0005"), document);
}

/// The old format embedded the id in the reply target too, so a migration has
/// to convert both or the document goes on naming itself.
#[test]
fn stripping_ids_converts_the_heading_and_its_own_replies() {
    let document = "# T0005: Old ticket\n\n- **Status**: Todo\n\n-----\n\n- **From**: jp\n- \
                    **Re**: T0005#1\n\nBody.\n";

    assert_eq!(
        render::strip_ids(document, "T0005"),
        "# Old ticket\n\n- **Status**: Todo\n\n-----\n\n- **From**: jp\n- **Re**: #1\n\nBody.\n"
    );
}

/// A board that hasn't started using labels reads fine; it just defines none.
#[test]
fn a_board_without_a_vocabulary_file_defines_no_labels() {
    let dir = Utf8TempDir::new().unwrap();

    assert!(vocabulary(dir.path()).unwrap().is_empty());
}

/// A vocabulary that is present but broken must not read as "no labels": every
/// write would then be refused with the caller blamed for the typo.
#[test]
fn a_malformed_vocabulary_file_is_an_error() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join(labels::FILE), "not json").unwrap();

    assert!(matches!(
        vocabulary(dir.path()),
        Err(Error::Labels(labels::Error::Malformed(_)))
    ));
}

#[test]
fn create_writes_the_labels_it_was_given() {
    let dir = Utf8TempDir::new().unwrap();
    write_vocabulary(&dir);

    let resolved = vocabulary(dir.path())
        .unwrap()
        .resolve(&owned(&["config", "app/macos"]))
        .unwrap();
    let (_, path) = create(dir.path(), &draft("Labelled", &resolved)).unwrap();

    let source = fs::read_to_string(&path).unwrap();
    assert!(
        source.contains("- **Labels**: app/macos, config\n"),
        "{source}"
    );
    assert_eq!(parse::document(&source).unwrap().metadata.labels, [
        "app/macos",
        "config"
    ]);
}

#[test]
fn set_labels_replaces_the_whole_set() {
    let dir = Utf8TempDir::new().unwrap();
    write_vocabulary(&dir);
    let vocabulary = vocabulary(dir.path()).unwrap();

    let resolved = vocabulary
        .resolve(&owned(&["config", "app/macos"]))
        .unwrap();
    let (id, _) = create(dir.path(), &draft("Labelled", &resolved)).unwrap();

    let (path, applied) = set_labels(dir.path(), id, &vocabulary, &owned(&["config"])).unwrap();

    assert_eq!(labels::join(&applied), "config");
    let ticket = parse::document(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(ticket.metadata.labels, ["config"]);
}

/// Clearing drops the field rather than leaving an empty one behind, so a
/// ticket with no labels looks like one that never had any.
#[test]
fn set_labels_with_nothing_drops_the_field() {
    let dir = Utf8TempDir::new().unwrap();
    write_vocabulary(&dir);
    let vocabulary = vocabulary(dir.path()).unwrap();

    let resolved = vocabulary.resolve(&owned(&["config"])).unwrap();
    let (id, _) = create(dir.path(), &draft("Labelled", &resolved)).unwrap();

    let (path, applied) = set_labels(dir.path(), id, &vocabulary, &[]).unwrap();

    assert!(applied.is_empty());
    let source = fs::read_to_string(&path).unwrap();
    assert!(!source.contains("Labels"), "{source}");
    assert!(parse::document(&source).unwrap().metadata.labels.is_empty());
}

/// Labelling a ticket that was filed without labels adds the field.
#[test]
fn set_labels_adds_the_field_to_a_ticket_without_one() {
    let dir = Utf8TempDir::new().unwrap();
    write_vocabulary(&dir);
    let vocabulary = vocabulary(dir.path()).unwrap();
    let id = new_ticket(&dir, "Unlabelled");

    let (path, _) = set_labels(dir.path(), id, &vocabulary, &owned(&["app/macos"])).unwrap();

    let ticket = parse::document(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(ticket.metadata.labels, ["app/macos"]);
    assert_eq!(ticket.description, "Description.");
}

/// The case the active/retired split exists for: an old ticket carries a label
/// the board has since retired, and adding a new one must not force the retired
/// one off first.
#[test]
fn a_retired_label_already_on_a_ticket_survives_a_relabel() {
    let dir = Utf8TempDir::new().unwrap();
    write_vocabulary(&dir);
    let vocabulary = vocabulary(dir.path()).unwrap();

    // Written by hand: `legacy-ui` can no longer be applied through the API,
    // which is exactly the situation an old ticket is in.
    let id = new_ticket(&dir, "Old ticket");
    let path = locate(dir.path(), id).unwrap();
    let source = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        render::set_metadata(&source, "Labels", "legacy-ui").unwrap(),
    )
    .unwrap();

    let (path, applied) = set_labels(
        dir.path(),
        id,
        &vocabulary,
        &owned(&["legacy-ui", "config"]),
    )
    .unwrap();

    assert_eq!(labels::join(&applied), "config, legacy-ui");
    let ticket = parse::document(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(ticket.metadata.labels, ["config", "legacy-ui"]);
}

#[test]
fn a_retired_label_cannot_be_added_to_a_ticket_without_it() {
    let dir = Utf8TempDir::new().unwrap();
    write_vocabulary(&dir);
    let vocabulary = vocabulary(dir.path()).unwrap();
    let id = new_ticket(&dir, "Fresh");

    let error = set_labels(dir.path(), id, &vocabulary, &owned(&["legacy-ui"])).unwrap_err();

    assert!(
        matches!(&error, Error::Rejected(rejected) if rejected.retired == ["legacy-ui"]),
        "{error}"
    );
}

/// A rejected write leaves the ticket exactly as it was, so a typo in one label
/// doesn't drop the others.
#[test]
fn a_rejected_relabel_writes_nothing() {
    let dir = Utf8TempDir::new().unwrap();
    write_vocabulary(&dir);
    let vocabulary = vocabulary(dir.path()).unwrap();

    let resolved = vocabulary.resolve(&owned(&["config"])).unwrap();
    let (id, path) = create(dir.path(), &draft("Labelled", &resolved)).unwrap();
    let before = fs::read_to_string(&path).unwrap();

    set_labels(dir.path(), id, &vocabulary, &owned(&["app/macos", "nope"])).unwrap_err();

    assert_eq!(fs::read_to_string(&path).unwrap(), before);
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
