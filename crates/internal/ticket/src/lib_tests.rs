use super::*;

#[test]
fn ids_render_in_canonical_form() {
    assert_eq!(TicketId::new(42).to_string(), "T0042");
    assert_eq!(TicketId::new(7).to_string(), "T0007");
    assert_eq!(TicketId::new(12345).to_string(), "T12345");
    assert_eq!(TicketId::new(42).file_prefix(), "0042-");
}

#[test]
fn ids_parse_from_every_accepted_form() {
    let expected = TicketId::new(42);

    assert_eq!("42".parse(), Ok(expected));
    assert_eq!("042".parse(), Ok(expected));
    assert_eq!("0042".parse(), Ok(expected));
    assert_eq!("T42".parse(), Ok(expected));
    assert_eq!("T0042".parse(), Ok(expected));
    assert_eq!("t0042".parse(), Ok(expected));
    assert_eq!(" T0042 ".parse(), Ok(expected));
}

#[test]
fn ids_reject_anything_else() {
    assert_eq!("".parse::<TicketId>(), Err(ParseError::Id(String::new())));
    assert_eq!("T".parse::<TicketId>(), Err(ParseError::Id("T".to_owned())));
    assert_eq!(
        "#42".parse::<TicketId>(),
        Err(ParseError::Id("#42".to_owned()))
    );
    // Zero is not a ticket: ids start at one.
    assert_eq!(
        "0000".parse::<TicketId>(),
        Err(ParseError::Id("0000".to_owned()))
    );
}

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

#[test]
fn kinds_round_trip() {
    for kind in [Kind::Bug, Kind::Feature, Kind::Chore] {
        assert_eq!(kind.to_string().parse(), Ok(kind));
    }
}

#[test]
fn kinds_parse_leniently() {
    assert_eq!("bug".parse(), Ok(Kind::Bug));
    assert_eq!("FEATURE".parse(), Ok(Kind::Feature));
    assert_eq!(
        "task".parse::<Kind>(),
        Err(ParseError::InvalidValue {
            field: "Kind",
            value: "task".to_owned(),
        })
    );
}
