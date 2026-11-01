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
