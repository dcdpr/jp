use super::*;

// Id rendering and parsing live in `id_tests.rs`.

#[test]
fn statuses_round_trip() {
    for status in [Status::Todo, Status::InProgress, Status::Done] {
        assert_eq!(status.to_string().parse(), Ok(status));
    }
}

#[test]
fn statuses_parse_leniently() {
    assert_eq!("todo".parse(), Ok(Status::Todo));
    assert_eq!("in progress".parse(), Ok(Status::InProgress));
    assert_eq!("In-Progress".parse(), Ok(Status::InProgress));
    assert_eq!("in_progress".parse(), Ok(Status::InProgress));
    assert_eq!("DONE".parse(), Ok(Status::Done));
    assert_eq!(
        "Blocked".parse::<Status>(),
        Err(ParseError::InvalidValue {
            field: "Status",
            value: "Blocked".to_owned(),
        })
    );
}

/// A ticket whose title, description, and one comment each name a different
/// word, so a test can tell which part a match came from.
fn searchable() -> Ticket {
    parse::document(
        "# Header misaligned\n\n- **Status**: Todo\n- **Authors**: john\n- **Date**: \
         2026-08-05\n- **Label**: client=cli\n\nThe wrap width ignores the gutter.\n\n## \
         Comments\n\n-----\n\n- **From**: john\n- **Date**: 2026-08-05T14:03:11Z\n\nReproduced at \
         72 columns.\n",
    )
    .unwrap()
}

#[test]
fn a_ticket_mentions_what_its_title_description_or_comments_say() {
    let ticket = searchable();

    assert!(ticket.mentions("misaligned"));
    assert!(ticket.mentions("GUTTER"));
    assert!(ticket.mentions("72 columns"));
    assert!(!ticket.mentions("80 columns"));
}

/// Labels and status are read through their own filters, so a word that only
/// appears in the metadata block is not a mention.
#[test]
fn the_metadata_block_is_not_searched() {
    let ticket = searchable();

    assert!(!ticket.mentions("client=cli"));
    assert!(!ticket.mentions("Todo"));
}
