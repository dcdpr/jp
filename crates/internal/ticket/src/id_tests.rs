use super::*;

/// The bucket for `2026-08-15T12:00:00Z`, five days and twelve hours past the
/// epoch: `(5 * 86_400 + 43_200) / 5`.
const BUCKET: u32 = 95_040;

#[test]
fn renders_the_canonical_form() {
    let id = TicketId::new(BUCKET, 637).unwrap();

    assert_eq!(id.to_string(), "T-02wt0kx");
    assert_eq!(id.as_str(), "02wt0kx");
    assert_eq!(id.file_prefix(), "02wt0kx-");
}

#[test]
fn round_trips_its_components() {
    let id = TicketId::new(BUCKET, 637).unwrap();

    assert_eq!(id.bucket(), BUCKET);
    assert_eq!(id.tail(), 637);
}

#[test]
fn pads_a_low_bucket_to_full_width() {
    let id = TicketId::new(0, 0).unwrap();

    assert_eq!(id.to_string(), "T-0000000");
}

#[test]
fn renders_the_last_expressible_id() {
    let id = TicketId::new(MAX_BUCKET - 1, TAIL_SPACE - 1).unwrap();

    assert_eq!(id.to_string(), "T-zzzzzzz");
}

#[test]
fn refuses_components_beyond_the_format() {
    assert_eq!(TicketId::new(MAX_BUCKET, 0), None);
    assert_eq!(TicketId::new(0, TAIL_SPACE), None);
}

#[test]
fn parses_every_accepted_spelling() {
    let expected = TicketId::new(BUCKET, 637).unwrap();

    for input in ["T-02wt0kx", "T02wt0kx", "02wt0kx", "t-02wt0kx", "T-02WT0KX"] {
        assert_eq!(input.parse::<TicketId>(), Ok(expected), "{input}");
    }
}

/// The alphabet omits the characters these are mistaken for, so a
/// mis-transcribed id from a call still resolves.
#[test]
fn folds_the_characters_the_alphabet_omits() {
    let expected = "T-1010101".parse::<TicketId>().unwrap();

    assert_eq!("T-lolOIoi".parse::<TicketId>(), Ok(expected));
}

#[test]
fn rejects_strings_that_are_not_ids() {
    for input in ["", "T-", "02wt0k", "02wt0kxx", "T-02wt0k!", "42", "T0042"] {
        assert!(
            input.parse::<TicketId>().is_err(),
            "{input} parsed as an id"
        );
    }
}

/// A body that happens to start with `t` is not a prefixed id: stripping it
/// would leave six characters.
#[test]
fn does_not_strip_a_leading_t_from_a_bare_body() {
    let id = "t02wt0k".parse::<TicketId>().unwrap();

    assert_eq!(id.to_string(), "T-t02wt0k");
}

#[test]
fn sorts_by_bucket_then_tail() {
    let first = TicketId::new(BUCKET, 5).unwrap();
    let second = TicketId::new(BUCKET, 6).unwrap();
    let later = TicketId::new(BUCKET + 1, 0).unwrap();

    assert!(first < second);
    assert!(second < later);

    // Byte order is what every consumer sorts on, from `ls` to the board.
    let mut sorted = [later.as_str(), first.as_str(), second.as_str()];
    sorted.sort_unstable();
    assert_eq!(sorted, [first.as_str(), second.as_str(), later.as_str()]);
}

#[test]
fn normalizes_a_prefix_for_lookup() {
    assert_eq!(TicketId::normalize("T-02wt"), Some("02wt".to_owned()));
    assert_eq!(TicketId::normalize("02WT"), Some("02wt".to_owned()));
    assert_eq!(TicketId::normalize("0l"), Some("01".to_owned()));
    assert_eq!(TicketId::normalize(""), None);
    assert_eq!(TicketId::normalize("02wt0kxx"), None);
    assert_eq!(TicketId::normalize("no!"), None);
}

#[test]
fn serializes_as_the_canonical_form() {
    let id = TicketId::new(BUCKET, 637).unwrap();

    assert_eq!(serde_json::to_string(&id).unwrap(), "\"T-02wt0kx\"");
}
