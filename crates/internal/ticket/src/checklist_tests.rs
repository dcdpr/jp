use indoc::indoc;
use serde::Deserialize;

use super::*;

/// One case of `checklist_cases.json`, which the site's reader in `jp-vite` is
/// tested against too.
#[derive(Deserialize)]
struct Case {
    case: String,
    description: String,
    items: Vec<(bool, String)>,
}

#[test]
fn the_shared_cases_read_as_listed() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("checklist_cases.json")).unwrap();

    for case in cases {
        let read: Vec<(bool, String)> = items(&case.description)
            .into_iter()
            .map(|item| (item.checked, item.text))
            .collect();

        assert_eq!(read, case.items, "{}", case.case);
    }
}

#[test]
fn an_item_knows_its_line() {
    let lines: Vec<usize> = items("Intro.\n\n- [ ] One\n- [x] Two")
        .iter()
        .map(|item| item.line)
        .collect();

    assert_eq!(lines, [2, 3]);
}

#[test]
fn progress_counts_the_checked_items() {
    let progress = progress("- [x] One\n- [ ] Two\n- [X] Three").unwrap();

    assert_eq!(progress, Progress {
        checked: 2,
        count: 3
    });
    assert_eq!(progress.open(), 1);
    assert_eq!(progress.to_string(), "2 of 3 checked");
}

#[test]
fn a_description_without_items_has_no_progress() {
    assert_eq!(progress("- A plain list\n- of two"), None);
}

const PLAN: &str = indoc! {"
    The plan:

    - [ ] Add the types
    - [x] Add the parser
    - [ ] Add the types to the docs

    ```markdown
    - [ ] Quoted, so not an item
    ```"};

fn named(queries: &[&str]) -> Vec<String> {
    queries.iter().map(|query| (*query).to_owned()).collect()
}

#[test]
fn checking_an_item_changes_its_checkbox_and_nothing_else() {
    let marked = mark(PLAN, &named(&["docs"]), &[]).unwrap();

    assert_eq!(marked.description, indoc! {"
        The plan:

        - [ ] Add the types
        - [x] Add the parser
        - [x] Add the types to the docs

        ```markdown
        - [ ] Quoted, so not an item
        ```"});
    assert_eq!(marked.changes, [Change {
        text: "Add the types to the docs".to_owned(),
        checked: true,
        changed: true,
    }]);
    assert_eq!(marked.progress, Progress {
        checked: 2,
        count: 3
    });
}

#[test]
fn unchecking_an_item_clears_its_checkbox() {
    let marked = mark("- [X] Done\n- [ ] Open", &[], &named(&["done"])).unwrap();

    assert_eq!(marked.description, "- [ ] Done\n- [ ] Open");
    assert_eq!(marked.changes, [Change {
        text: "Done".to_owned(),
        checked: false,
        changed: true,
    }]);
}

/// An item already in the state asked for is reported, not refused, so a
/// retried call lands the same way twice.
#[test]
fn an_item_already_in_place_is_reported_unchanged() {
    let marked = mark("- [X] Done", &named(&["Done"]), &[]).unwrap();

    assert_eq!(marked.description, "- [X] Done");
    assert_eq!(marked.changes, [Change {
        text: "Done".to_owned(),
        checked: true,
        changed: false,
    }]);
}

/// "Add the types" is also part of "Add the types to the docs"; naming one
/// item's whole text picks that item.
#[test]
fn a_whole_match_wins_over_a_partial_one() {
    let marked = mark(PLAN, &named(&["add the types"]), &[]).unwrap();

    assert_eq!(marked.changes[0].text, "Add the types");
}

#[test]
fn a_query_matching_several_items_is_refused_with_the_candidates() {
    let refused = mark(PLAN, &named(&["types"]), &[]).unwrap_err();

    assert_eq!(
        refused.to_string(),
        "`types` matches more than one checklist item: \"Add the types\", \"Add the types to the \
         docs\". Name more of the one you mean."
    );
}

#[test]
fn a_query_matching_nothing_is_refused_with_the_items() {
    let refused = mark(PLAN, &named(&["Quoted"]), &[]).unwrap_err();

    assert_eq!(refused.to_string(), indoc! {"
            No checklist item contains `Quoted`.
            The checklist items are:
            - [ ] Add the types
            - [x] Add the parser
            - [ ] Add the types to the docs"});
}

/// Every problem is named at once, so one retry can fix them all, and nothing
/// is changed by a call that names any.
#[test]
fn every_problem_is_reported_together() {
    let refused = mark(
        PLAN,
        &named(&["parser", "nothing like it", "  "]),
        &named(&["Add the parser"]),
    )
    .unwrap_err();

    assert_eq!(refused.to_string(), indoc! {"
            No checklist item contains `nothing like it`.
            A checklist item to find can't be empty.
            `Add the parser` is named to be both checked and unchecked.
            The checklist items are:
            - [ ] Add the types
            - [x] Add the parser
            - [ ] Add the types to the docs"});
}

#[test]
fn an_item_named_twice_is_changed_once() {
    let marked = mark(PLAN, &named(&["parser", "Add the parser"]), &[]).unwrap();

    assert_eq!(marked.changes.len(), 1);
}

#[test]
fn a_description_without_items_has_nothing_to_mark() {
    let refused = mark("Just prose.", &named(&["anything"]), &[]).unwrap_err();

    assert_eq!(
        refused.to_string(),
        "The description holds no checklist items (`- [ ] ...`) to check or uncheck."
    );
}

/// The description is split on `\n` alone, so a file written with CRLF line
/// endings keeps them.
#[test]
fn line_endings_are_kept() {
    let marked = mark("- [ ] One\r\n- [ ] Two", &named(&["One"]), &[]).unwrap();

    assert_eq!(marked.description, "- [x] One\r\n- [ ] Two");
    assert_eq!(marked.changes[0].text, "One");
}
