use chrono::{NaiveDate, Weekday};
use serde_json::json;

use super::*;

fn read(options: Value) -> Result<Options, String> {
    let Value::Object(options) = options else {
        panic!("options are an object");
    };

    Options::read(&options)
}

fn minimal() -> Value {
    json!({
        "signal": { "account": "+31600000000" },
        "people": { "client": { "name": "Ada Client", "signal": "+31611111111" } }
    })
}

#[test]
fn unset_options_take_their_defaults() {
    assert_eq!(
        read(minimal()),
        Ok(Options {
            persona: None,
            cfg: vec![],
            admin: None,
            rollover: Rollover::Daily,
            conversation_ttl: "7d".to_owned(),
            people: BTreeMap::from([("client".to_owned(), Person {
                name: "Ada Client".to_owned(),
                signal: Some("+31611111111".to_owned()),
            })]),
            checkout: CheckoutOptions {
                enabled: true,
                writable: vec![
                    "docs/ticket".to_owned(),
                    "docs/rfd".to_owned(),
                    "docs/wiki".to_owned()
                ],
                pull_interval: Duration::from_mins(5),
            },
            signal: SignalOptions {
                account: "+31600000000".to_owned(),
                account_id: None,
                command: "signal-cli".to_owned(),
            },
        })
    );
}

#[test]
fn a_weekday_rolls_over_weekly() {
    let mut options = minimal();
    options["rollover"] = json!("Sunday");

    let read = read(options).unwrap();

    assert_eq!(read.rollover, Rollover::Weekly(Weekday::Sun));
    assert_eq!(read.conversation_ttl, "14d");
}

#[test]
fn an_unknown_rollover_is_refused() {
    let mut options = minimal();
    options["rollover"] = json!("fortnightly");

    assert_eq!(
        read(options),
        Err(
            "`plugins.command.serve-chat.options.rollover` is `fortnightly`; use `daily`, or the \
             day of the week a new conversation starts on, such as `sunday`."
                .to_owned()
        )
    );
}

#[test]
fn a_ttl_shorter_than_the_rollover_is_refused() {
    let mut options = minimal();
    options["rollover"] = json!("sunday");
    options["conversation_ttl"] = json!("3d");

    assert_eq!(
        read(options),
        Err(
            "`plugins.command.serve-chat.options.conversation_ttl` is `3d`, shorter than a \
             conversation lasts: it would be removed while the chat still uses it. Make it at \
             least `7days`."
                .to_owned()
        )
    );
}

#[test]
fn a_weekly_conversation_starts_on_the_last_rollover_day() {
    let day = |day| NaiveDate::from_ymd_opt(2026, 11, day).unwrap();
    let sunday = Rollover::Weekly(Weekday::Sun);

    // 2026-11-08 is a Sunday.
    assert_eq!(sunday.start(day(7)), day(1));
    assert_eq!(sunday.start(day(8)), day(8));
    assert_eq!(sunday.start(day(14)), day(8));
    assert_eq!(Rollover::Daily.start(day(7)), day(7));
}

#[test]
fn nobody_to_answer_is_refused() {
    assert_eq!(
        read(json!({ "signal": { "account": "+31600000000" } })),
        Err(
            "`plugins.command.serve-chat.options.people` is empty, so the bridge would answer \
             nobody. List who it answers."
                .to_owned()
        )
    );
}

#[test]
fn the_account_is_required() {
    assert_eq!(
        read(json!({ "people": { "client": { "name": "Ada" } } })),
        Err(
            "`plugins.command.serve-chat.options.signal.account` is required: the account to run \
             as."
            .to_owned()
        )
    );
}

#[test]
fn the_admin_has_to_be_one_of_the_people() {
    let mut options = minimal();
    options["admin"] = json!("maintainer");

    assert_eq!(
        read(options),
        Err(
            "`plugins.command.serve-chat.options.admin` is `maintainer`, which isn't one of \
             `plugins.command.serve-chat.options.people`."
                .to_owned()
        )
    );
}

#[test]
fn a_writable_directory_has_to_stay_in_the_workspace() {
    for bad in ["/etc", "docs/../..", ""] {
        let mut options = minimal();
        options["checkout"] = json!({ "writable": [bad] });

        assert!(read(options).is_err(), "{bad:?} was accepted");
    }
}

#[test]
fn an_unknown_transport_is_refused() {
    let mut options = minimal();
    options["transport"] = json!("telegram");

    assert_eq!(
        read(options),
        Err(
            "`plugins.command.serve-chat.options.transport` is `telegram`; the only transport is \
             `signal`."
                .to_owned()
        )
    );
}

#[test]
fn a_person_is_found_by_any_of_their_signal_ids() {
    let options = read(minimal()).unwrap();

    let found = options.signal_person(&[
        "11111111-1111-4111-8111-111111111111".to_owned(),
        "+31611111111".to_owned(),
    ]);

    assert_eq!(found.map(|(key, _)| key), Some("client"));
    assert_eq!(options.signal_person(&["+31699999999".to_owned()]), None);
}
