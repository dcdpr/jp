use chrono::{NaiveDate, Weekday};
use jp_plugin::message::{GenerateSummary, SummaryCompaction};
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
                attachments_dir: None,
            },
            decide: None,
            compact: vec![Compaction {
                from: NonZeroI64::new(2).unwrap(),
                to: NonZeroI64::new(-2).unwrap(),
                reasoning: Some(ReasoningCompaction { over_bytes: None }),
                tool_calls: Some(ToolCallsCompaction {
                    mode: ToolCallsMode::Strip,
                    over_bytes: None,
                }),
                summary: None,
            }],
            inject: None,
        })
    );
}

#[test]
fn compact_names_the_compactions_to_ask_for() {
    let mut options = minimal();
    options["compact"] = json!([
        { "from": 1, "to": -3, "summary": { "generate": { "model": "haiku" } } },
        { "from": -2, "to": -1, "tool_calls": { "mode": "omit" } },
    ]);

    assert_eq!(read(options).unwrap().compact, [
        Compaction {
            from: NonZeroI64::new(1).unwrap(),
            to: NonZeroI64::new(-3).unwrap(),
            reasoning: None,
            tool_calls: None,
            summary: Some(SummaryCompaction::Generate(GenerateSummary {
                model: Some("haiku".to_owned()),
                instructions: None,
                context: None,
            })),
        },
        Compaction {
            from: NonZeroI64::new(-2).unwrap(),
            to: NonZeroI64::new(-1).unwrap(),
            reasoning: None,
            tool_calls: Some(ToolCallsCompaction {
                mode: ToolCallsMode::Omit,
                over_bytes: None,
            }),
            summary: None,
        },
    ]);
}

#[test]
fn an_empty_compact_list_is_refused() {
    let mut options = minimal();
    options["compact"] = json!([]);

    assert_eq!(
        read(options),
        Err(
            "`plugins.command.serve-chat.options.compact` names no compactions; leave it out for \
             the default."
                .to_owned()
        )
    );
}

// Unknown fields are rejected at startup rather than when a user requests
// compaction.
#[test]
fn a_compaction_with_an_unknown_field_is_refused() {
    let mut options = minimal();
    options["compact"] = json!([{ "from": 2, "to": -2, "keep_last": 1 }]);

    let error = read(options).unwrap_err();

    assert!(
        error.starts_with(
            "`plugins.command.serve-chat.options` is invalid: unknown field `keep_last`"
        ),
        "{error}"
    );
}

#[test]
fn an_empty_decide_table_asks_jev_with_the_default_question() {
    let mut options = minimal();
    options["decide"] = json!({});

    assert_eq!(
        read(options).unwrap().decide,
        Some(DecideOptions {
            url: "https://api.typesafe.ai/v1/systemone".to_owned(),
            model: "jev-latest".to_owned(),
            api_key_env: Some("JEV_API_KEY".to_owned()),
            threshold: 0.5,
            instructions: DEFAULT_DECIDE_INSTRUCTIONS.to_owned(),
        })
    );
}

#[test]
fn an_empty_decide_api_key_env_sends_no_token() {
    let mut options = minimal();
    options["decide"] = json!({ "url": "http://127.0.0.1:8080/v1/systemone", "api_key_env": "" });

    let decide = read(options).unwrap().decide.unwrap();

    assert_eq!(decide.url, "http://127.0.0.1:8080/v1/systemone");
    assert_eq!(decide.api_key_env, None);
}

#[test]
fn a_decide_threshold_is_a_probability() {
    let mut options = minimal();
    options["decide"] = json!({ "threshold": 1.5 });

    assert_eq!(
        read(options),
        Err(
            "`plugins.command.serve-chat.options.decide.threshold` is `1.5`; it is a probability, \
             from 0 to 1."
                .to_owned()
        )
    );
}

/// [`minimal`], deciding with Jev and injecting `inject`.
fn injecting(inject: Value) -> Value {
    let mut options = minimal();
    options["decide"] = json!({});
    options["inject"] = inject;
    options
}

#[test]
fn inject_reads_packs_and_references() {
    let inject = read(injecting(json!({
        "packs": {
            "oldest": {
                "when": " The message asks for the oldest tickets. ",
                "command": ["jp", "ticket", "list", "--order", "filed"]
            },
            "in_progress": {
                "when": "The message asks about work in progress.",
                "title": "Work in progress",
                "command": ["jp", "ticket", "list", "--status", "in progress"]
            }
        },
        "references": [
            { "pattern": "\\bT-[0-9a-z]{7}\\b", "command": ["jp", "ticket", "show", "{0}"] }
        ]
    })))
    .unwrap()
    .inject
    .unwrap();

    assert!(
        inject.threshold.total_cmp(&0.5).is_eq(),
        "{}",
        inject.threshold
    );
    assert_eq!(inject.max_chars, 8000);
    assert_eq!(inject.instructions, DEFAULT_INJECT_INSTRUCTIONS);
    assert_eq!(inject.packs["oldest"], Pack {
        when: "The message asks for the oldest tickets.".to_owned(),
        title: "oldest".to_owned(),
        command: vec![
            "jp".to_owned(),
            "ticket".to_owned(),
            "list".to_owned(),
            "--order".to_owned(),
            "filed".to_owned()
        ],
    });
    assert_eq!(inject.packs["in_progress"].title, "Work in progress");
    assert_eq!(inject.references[0].pattern.as_str(), r"\bT-[0-9a-z]{7}\b");
}

/// The decide model picks the pack, and the lookups run while a message is
/// decided, so `inject` without `decide` would never run.
#[test]
fn inject_needs_decide() {
    let mut options = minimal();
    options["inject"] = json!({});

    assert_eq!(
        read(options),
        Err("`plugins.command.serve-chat.options.inject` needs \
             `plugins.command.serve-chat.options.decide`: the decide model picks the pack, and \
             the lookups run while a message is being decided."
            .to_owned())
    );
}

#[test]
fn an_unusable_pack_or_reference_is_refused() {
    let key = "plugins.command.serve-chat.options.inject";
    let cases = [
        (
            json!({ "packs": { "none": { "when": "x", "command": ["jp"] } } }),
            format!(
                "`{key}.packs.none` needs another name: lowercase letters, digits, and `_`, other \
                 than `none`."
            ),
        ),
        (
            json!({ "packs": { "Oldest": { "when": "x", "command": ["jp"] } } }),
            format!(
                "`{key}.packs.Oldest` needs another name: lowercase letters, digits, and `_`, \
                 other than `none`."
            ),
        ),
        (
            json!({ "packs": { "oldest": { "command": ["jp"] } } }),
            format!(
                "`{key}.packs.oldest.when` is required: it tells the decide model when to pick \
                 the pack."
            ),
        ),
        (
            json!({ "packs": { "oldest": { "when": "x", "command": [] } } }),
            format!(
                "`{key}.packs.oldest.command` is required: the program to run and its arguments."
            ),
        ),
        (
            json!({ "references": [{ "pattern": "T-", "command": [" "] }] }),
            format!(
                "`{key}.references[0].command` is required: the program to run and its arguments."
            ),
        ),
        (
            json!({ "threshold": 2 }),
            format!("`{key}.threshold` is `2`; it is a probability, from 0 to 1."),
        ),
        (
            json!({ "max_chars": 0 }),
            format!("`{key}.max_chars` is 0, which keeps nothing; leave it out for the default."),
        ),
    ];

    for (inject, error) in cases {
        assert_eq!(read(injecting(inject.clone())), Err(error), "{inject}");
    }

    let error = read(injecting(
        json!({ "references": [{ "pattern": "(", "command": ["jp"] }] }),
    ))
    .unwrap_err();
    assert!(
        error.starts_with(
            "`plugins.command.serve-chat.options.inject.references[0].pattern` is not a regular \
             expression: "
        ),
        "{error}"
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
