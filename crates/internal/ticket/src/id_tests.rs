use super::*;

// The format itself, its alphabet and its ordering, is tested in `doc::id`;
// these pin what makes an id a ticket's.

/// The bucket for `2026-08-15T12:00:00Z`, five days and twelve hours past the
/// epoch: `(5 * 86_400 + 43_200) / 5`.
const BUCKET: u32 = 95_040;

#[test]
fn renders_with_the_ticket_prefix() {
    let id = TicketId::new(BUCKET, 637).unwrap();

    assert_eq!(id.to_string(), "T-02wt0kx");
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"T-02wt0kx\"");
}

#[test]
fn parses_every_accepted_spelling() {
    let expected = TicketId::new(BUCKET, 637).unwrap();

    for input in ["T-02wt0kx", "T02wt0kx", "02wt0kx", "t-02wt0kx", "T-02WT0KX"] {
        assert_eq!(input.parse::<TicketId>(), Ok(expected), "{input}");
    }
}

#[test]
fn a_string_that_is_not_an_id_is_named() {
    let error = "T0042".parse::<TicketId>().unwrap_err();

    assert_eq!(
        error.to_string(),
        "`T0042` is not a ticket id (try T-02wt0kx)."
    );
}

#[test]
fn reads_the_id_a_ticket_file_name_opens_with() {
    let expected = TicketId::new(BUCKET, 637);

    assert_eq!(TicketId::of_file("02wt0kx-turn-summaries.md"), expected);
    assert_eq!(TicketId::of_file("02wt0kx-x.md"), expected);

    for name in [
        "02wt0kx.md",
        "02wt0kx-.md",
        "02wt0kx-slug.txt",
        "02wt0k-slug.md",
        "index.md",
    ] {
        assert_eq!(TicketId::of_file(name), None, "{name}");
    }
}

#[test]
fn parses_an_id_or_the_path_of_a_ticket_file() {
    let expected = TicketId::new(BUCKET, 637).unwrap();

    for input in [
        "T-02wt0kx",
        "02wt0kx",
        "02wt0kx-turn-summaries.md",
        "docs/ticket/02wt0kx-turn-summaries.md",
        "/abs/docs/ticket/02wt0kx-turn-summaries.md",
    ] {
        assert_eq!(TicketId::parse_id_or_path(input), Ok(expected), "{input}");
    }
}

/// Only a file name ending in `.md` is read as a path, so a mistyped word with
/// a dash isn't taken for the id its first seven characters fold onto.
#[test]
fn refuses_a_path_that_names_no_ticket_file() {
    for input in [
        "docs/ticket/turn-summaries.md",
        "docs/ticket/02wt0kx",
        "comment-foo",
        "docs/ticket/",
    ] {
        assert_eq!(
            TicketId::parse_id_or_path(input).unwrap_err().to_string(),
            format!("`{input}` is not a ticket id (try T-02wt0kx)."),
            "{input}"
        );
    }
}
