use camino::Utf8PathBuf;

use super::*;

/// A board whose `type` every ticket carries exactly one of, with one retired
/// value, and a link key each value of which one ticket may carry.
fn vocabulary() -> Vocabulary {
    Vocabulary::parse(
        r#"{
            "type": {
                "description": "The kind of work.",
                "values": ["bug", "task"],
                "retired": ["chore"],
                "min": 1,
                "max": 1
            },
            "rfd": {"description": "The RFD.", "values": [], "unique": "key-value"}
        }"#,
    )
    .unwrap()
}

/// A well-formed ticket carrying `labels`, as `id`.
fn ticket(id: &str, labels: &[&str]) -> TicketFile {
    let mut lines = String::new();
    for label in labels {
        lines.push_str("- **Label**: ");
        lines.push_str(label);
        lines.push('\n');
    }

    TicketFile {
        id: id.parse().unwrap(),
        path: Utf8PathBuf::from(format!("/t/{}-a.md", &id[2..])),
        source: format!(
            "# A ticket\n\n- **Status**: Todo\n- **Authors**: john\n- **Date**: \
             2026-08-05\n{lines}\nDescription.\n"
        ),
    }
}

fn messages(report: &Report) -> Vec<String> {
    report.problems.iter().map(ToString::to_string).collect()
}

#[test]
fn a_board_meeting_every_rule_has_no_problems() {
    let directory = Directory {
        legacy: vec![],
        tickets: vec![
            ticket("T-0aaaaaa", &["type=bug", "rfd=116"]),
            ticket("T-0bbbbbb", &["type=task", "rfd=117"]),
        ],
    };

    let report = report(&directory, &vocabulary());

    assert_eq!(report.tickets, 2);
    assert_eq!(messages(&report), Vec::<String>::new());
}

#[test]
fn a_ticket_missing_a_required_key_is_reported_with_the_values_to_add() {
    let directory = Directory {
        legacy: vec![],
        tickets: vec![ticket("T-0aaaaaa", &[])],
    };

    assert_eq!(messages(&report(&directory, &vocabulary())), vec![
        "T-0aaaaaa: `type` label missing: add one of type=bug, type=task.".to_owned()
    ]);
}

#[test]
fn a_ticket_carrying_too_many_of_a_bounded_key_is_reported() {
    let directory = Directory {
        legacy: vec![],
        tickets: vec![ticket("T-0aaaaaa", &["type=bug", "type=task"])],
    };

    assert_eq!(messages(&report(&directory, &vocabulary())), vec![
        "T-0aaaaaa: `type` takes exactly one label, and 2 are given: type=bug, type=task."
            .to_owned()
    ]);
}

#[test]
fn an_unknown_label_is_reported_without_the_whole_vocabulary() {
    let directory = Directory {
        legacy: vec![],
        tickets: vec![ticket("T-0aaaaaa", &["type=bug", "area=cli", "type=bogus"])],
    };

    // `type=bogus` is refused, so it doesn't count toward `type`'s bound.
    assert_eq!(messages(&report(&directory, &vocabulary())), vec![
        "T-0aaaaaa: `area` is not a known label key. `type=bogus` is not a known label.".to_owned()
    ]);
}

#[test]
fn a_retired_value_a_ticket_already_carries_is_not_a_problem() {
    let directory = Directory {
        legacy: vec![],
        tickets: vec![ticket("T-0aaaaaa", &["type=chore"])],
    };

    assert_eq!(
        messages(&report(&directory, &vocabulary())),
        Vec::<String>::new()
    );
}

#[test]
fn a_malformed_ticket_is_reported_and_its_labels_are_still_checked() {
    let mut broken = ticket("T-0aaaaaa", &["area=cli"]);
    broken.source = broken.source.replace("- **Status**: Todo\n", "");

    let directory = Directory {
        legacy: vec![],
        tickets: vec![broken],
    };

    assert_eq!(messages(&report(&directory, &vocabulary())), vec![
        "/t/0aaaaaa-a.md is not a well-formed ticket: Ticket is missing the `Status` field."
            .to_owned(),
        "T-0aaaaaa: `area` is not a known label key. `type` label missing: add one of type=bug, \
         type=task."
            .to_owned(),
    ]);
}

#[test]
fn a_label_line_that_is_not_a_label_is_reported() {
    let directory = Directory {
        legacy: vec![],
        tickets: vec![ticket("T-0aaaaaa", &["type=bug", "two words"])],
    };

    assert_eq!(messages(&report(&directory, &vocabulary())), vec![
        "T-0aaaaaa: `two words` is not a `key=value` label.".to_owned()
    ]);
}

#[test]
fn a_board_without_a_vocabulary_takes_any_well_formed_label() {
    let directory = Directory {
        legacy: vec![],
        tickets: vec![
            ticket("T-0aaaaaa", &["anything=goes", "blocked"]),
            ticket("T-0bbbbbb", &["two words"]),
        ],
    };

    assert_eq!(messages(&report(&directory, &Vocabulary::default())), vec![
        "T-0bbbbbb: `two words` is not a `key=value` label.".to_owned()
    ]);
}

#[test]
fn a_unique_label_on_several_tickets_names_every_ticket_once() {
    let directory = Directory {
        legacy: vec![],
        tickets: vec![
            ticket("T-0aaaaaa", &["type=bug", "rfd=116"]),
            ticket("T-0bbbbbb", &["type=bug", "rfd=117"]),
            // Values match without case, as they do on a write.
            ticket("T-0cccccc", &["type=bug", "RFD=116"]),
            ticket("T-0dddddd", &["type=task", "rfd=116"]),
        ],
    };

    assert_eq!(messages(&report(&directory, &vocabulary())), vec![
        "`rfd=116` is on T-0aaaaaa, T-0cccccc, T-0dddddd, and the board lets one ticket carry \
         each `rfd` value."
            .to_owned()
    ]);
}

#[test]
fn files_claiming_one_id_are_reported_together() {
    let mut second = ticket("T-0aaaaaa", &["type=bug"]);
    second.path = Utf8PathBuf::from("/t/0aaaaaa-b.md");

    let directory = Directory {
        legacy: vec![],
        tickets: vec![ticket("T-0aaaaaa", &["type=bug"]), second],
    };

    assert_eq!(messages(&report(&directory, &vocabulary())), vec![
        "T-0aaaaaa is claimed by more than one file: /t/0aaaaaa-a.md, /t/0aaaaaa-b.md. Give all \
         but one a fresh id with `jp ticket refresh PATH`."
            .to_owned()
    ]);
}

#[test]
fn problems_come_file_wide_first_then_by_ticket_then_shared() {
    let directory = Directory {
        legacy: vec![Utf8PathBuf::from("/t/0001-old.md")],
        tickets: vec![
            ticket("T-0aaaaaa", &["rfd=116"]),
            ticket("T-0bbbbbb", &["type=bug", "rfd=116"]),
        ],
    };

    let report = report(&directory, &vocabulary());

    assert_eq!(report.tickets, 2);
    assert_eq!(messages(&report), vec![
        "/t/0001-old.md is named in the four-digit `NNNN-slug.md` form, which no command reads. \
         Rename it to the `<id>-slug.md` form."
            .to_owned(),
        "T-0aaaaaa: `type` label missing: add one of type=bug, type=task.".to_owned(),
        "`rfd=116` is on T-0aaaaaa, T-0bbbbbb, and the board lets one ticket carry each `rfd` \
         value."
            .to_owned(),
    ]);
}
