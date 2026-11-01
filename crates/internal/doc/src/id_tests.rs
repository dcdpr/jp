use super::*;

/// A kind shaped like a ticket.
#[derive(Debug)]
enum Ticket {}

impl Kind for Ticket {
    const PREFIX: char = 'T';
    const NOUN: &'static str = "ticket";
    const WRITER: &'static str = "JP-TICKET";
}

/// A kind shaped like a wiki page, whose prefix is a letter of the alphabet.
#[derive(Debug)]
enum Page {}

impl Kind for Page {
    const PREFIX: char = 'W';
    const NOUN: &'static str = "wiki page";
    const WRITER: &'static str = "JP-WIKI";
}

/// The bucket for `2026-08-15T12:00:00Z`, five days and twelve hours past the
/// epoch: `(5 * 86_400 + 43_200) / 5`.
const BUCKET: u32 = 95_040;

#[test]
fn renders_the_canonical_form() {
    let id = Id::<Ticket>::new(BUCKET, 637).unwrap();

    assert_eq!(id.to_string(), "T-02wt0kx");
    assert_eq!(id.as_str(), "02wt0kx");
    assert_eq!(id.file_prefix(), "02wt0kx-");
}

#[test]
fn the_prefix_is_the_kind_s() {
    let id = Id::<Page>::new(BUCKET, 637).unwrap();

    assert_eq!(id.to_string(), "W-02wt0kx");
    assert_eq!(id.file_prefix(), "02wt0kx-");
}

#[test]
fn round_trips_its_components() {
    let id = Id::<Ticket>::new(BUCKET, 637).unwrap();

    assert_eq!(id.bucket(), BUCKET);
    assert_eq!(id.tail(), 637);
}

#[test]
fn pads_a_low_bucket_to_full_width() {
    let id = Id::<Ticket>::new(0, 0).unwrap();

    assert_eq!(id.to_string(), "T-0000000");
}

#[test]
fn renders_the_last_expressible_id() {
    let id = Id::<Ticket>::new(MAX_BUCKET - 1, TAIL_SPACE - 1).unwrap();

    assert_eq!(id.to_string(), "T-zzzzzzz");
}

#[test]
fn refuses_components_beyond_the_format() {
    assert_eq!(Id::<Ticket>::new(MAX_BUCKET, 0), None);
    assert_eq!(Id::<Ticket>::new(0, TAIL_SPACE), None);
}

#[test]
fn parses_every_accepted_spelling() {
    let expected = Id::<Ticket>::new(BUCKET, 637).unwrap();

    for input in ["T-02wt0kx", "T02wt0kx", "02wt0kx", "t-02wt0kx", "T-02WT0KX"] {
        assert_eq!(input.parse::<Id<Ticket>>(), Ok(expected), "{input}");
    }

    let expected = Id::<Page>::new(BUCKET, 637).unwrap();

    for input in ["W-02wt0kx", "W02wt0kx", "02wt0kx", "w-02wt0kx", "W-02WT0KX"] {
        assert_eq!(input.parse::<Id<Page>>(), Ok(expected), "{input}");
    }
}

/// The prefix is what tells one kind's id from another's in prose, so neither
/// reads as the other.
#[test]
fn an_id_of_another_kind_is_refused() {
    assert!("W-02wt0kx".parse::<Id<Ticket>>().is_err());
    assert!("W02wt0kx".parse::<Id<Ticket>>().is_err());
    assert!("T-02wt0kx".parse::<Id<Page>>().is_err());
    assert!("T02wt0kx".parse::<Id<Page>>().is_err());
}

/// The alphabet omits the characters these are mistaken for, so a
/// mis-transcribed id from a call still resolves.
#[test]
fn folds_the_characters_the_alphabet_omits() {
    let expected = "T-1010101".parse::<Id<Ticket>>().unwrap();

    assert_eq!("T-lolOIoi".parse::<Id<Ticket>>(), Ok(expected));
}

#[test]
fn rejects_strings_that_are_not_ids() {
    for input in ["", "T-", "02wt0k", "02wt0kxx", "T-02wt0k!", "42", "T0042"] {
        assert!(
            input.parse::<Id<Ticket>>().is_err(),
            "{input} parsed as an id"
        );
    }
}

/// A body that happens to start with the prefix letter is not a prefixed id:
/// stripping it would leave six characters.
#[test]
fn does_not_strip_a_leading_prefix_letter_from_a_bare_body() {
    let ticket = "t02wt0k".parse::<Id<Ticket>>().unwrap();
    let page = "w02wt0k".parse::<Id<Page>>().unwrap();

    assert_eq!(ticket.to_string(), "T-t02wt0k");
    assert_eq!(page.to_string(), "W-w02wt0k");
}

#[test]
fn sorts_by_bucket_then_tail() {
    let first = Id::<Ticket>::new(BUCKET, 5).unwrap();
    let second = Id::<Ticket>::new(BUCKET, 6).unwrap();
    let later = Id::<Ticket>::new(BUCKET + 1, 0).unwrap();

    assert!(first < second);
    assert!(second < later);

    // Byte order is what every consumer sorts on, from `ls` to the board.
    let mut sorted = [later.as_str(), first.as_str(), second.as_str()];
    sorted.sort_unstable();
    assert_eq!(sorted, [first.as_str(), second.as_str(), later.as_str()]);
}

#[test]
fn normalizes_a_prefix_for_lookup() {
    assert_eq!(Id::<Ticket>::normalize("T-02wt"), Some("02wt".to_owned()));
    assert_eq!(Id::<Ticket>::normalize("02WT"), Some("02wt".to_owned()));
    assert_eq!(Id::<Ticket>::normalize("0l"), Some("01".to_owned()));
    assert_eq!(Id::<Ticket>::normalize(""), None);
    assert_eq!(Id::<Ticket>::normalize("02wt0kxx"), None);
    assert_eq!(Id::<Ticket>::normalize("no!"), None);
    assert_eq!(Id::<Page>::normalize("W-02wt"), Some("02wt".to_owned()));
}

#[test]
fn serializes_as_the_canonical_form() {
    let id = Id::<Ticket>::new(BUCKET, 637).unwrap();

    assert_eq!(serde_json::to_string(&id).unwrap(), "\"T-02wt0kx\"");
}

#[test]
fn an_invalid_id_names_its_kind() {
    let ticket = "42".parse::<Id<Ticket>>().unwrap_err();
    let page = "42".parse::<Id<Page>>().unwrap_err();

    assert_eq!(
        ticket.to_string(),
        "`42` is not a ticket id (try T-02wt0kx)."
    );
    assert_eq!(
        page.to_string(),
        "`42` is not a wiki page id (try W-02wt0kx)."
    );
}
