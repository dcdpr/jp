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
