use std::cmp::Ordering;

use super::*;
use crate::Vocabulary;

fn vocabulary() -> Vocabulary {
    Vocabulary::parse(
        r#"{
            "due": {"values": [], "type": "date", "max": 1},
            "priority": {
                "values": ["high", "medium", "low"],
                "retired": ["urgent"],
                "max": 1,
                "ordered": true
            },
            "client": {"values": ["cli", "macos"], "max": 1},
            "tag": {"values": [], "max": 1},
            "deadline": {"values": [], "type": "date"}
        }"#,
    )
    .unwrap()
}

fn labels(tokens: &[&str]) -> Labels {
    Labels::from_tokens(tokens).unwrap()
}

fn sorted<'a>(
    key: &SortKey<'_>,
    direction: Direction,
    items: &[(&'a str, Labels)],
) -> Vec<&'a str> {
    let mut items = items.to_vec();
    items.sort_by(|a, b| key.compare(&a.1, &b.1, direction));

    items.into_iter().map(|(name, _)| name).collect()
}

#[test]
fn only_date_keys_and_ordered_keys_with_max_one_sort() {
    let vocabulary = vocabulary();

    assert_eq!(vocabulary.sortable_keys().collect::<Vec<_>>(), [
        "due", "priority"
    ]);
    assert_eq!(vocabulary.sort_key("due").map(|key| key.key()), Some("due"));
    for key in ["client", "tag", "deadline", "unknown"] {
        assert!(vocabulary.sort_key(key).is_none(), "{key} sorts");
    }
}

#[test]
fn a_date_key_sorts_by_date_and_puts_undated_items_last_both_ways() {
    let vocabulary = vocabulary();
    let due = vocabulary.sort_key("due").unwrap();
    let items = [
        ("undated", labels(&[])),
        ("december", labels(&["due=2026-12-01"])),
        // Invalid date values on hand-edited items are treated as undated and sort last.
        ("friday", labels(&["due=friday"])),
        ("january", labels(&["due=2026-01-15"])),
    ];

    assert_eq!(sorted(&due, Direction::Ascending, &items), [
        "january", "december", "undated", "friday"
    ]);
    assert_eq!(sorted(&due, Direction::Descending, &items), [
        "december", "january", "undated", "friday"
    ]);
}

#[test]
fn an_ordered_key_sorts_by_the_declared_order() {
    let vocabulary = vocabulary();
    let priority = vocabulary.sort_key("priority").unwrap();
    let items = [
        ("low", labels(&["priority=low"])),
        ("urgent", labels(&["priority=urgent"])),
        ("none", labels(&[])),
        ("high", labels(&["priority=HIGH"])),
        ("medium", labels(&["priority=medium"])),
    ];

    assert_eq!(sorted(&priority, Direction::Ascending, &items), [
        "high", "medium", "low", "urgent", "none"
    ]);
    assert_eq!(sorted(&priority, Direction::Descending, &items), [
        "urgent", "low", "medium", "high", "none"
    ]);
}

#[test]
fn the_same_value_compares_equal() {
    let vocabulary = vocabulary();
    let due = vocabulary.sort_key("due").unwrap();
    let a = labels(&["due=2026-05-01"]);
    let b = labels(&["due=2026-05-01", "client=cli"]);

    assert_eq!(due.compare(&a, &b, Direction::Ascending), Ordering::Equal);
    assert_eq!(due.compare(&a, &b, Direction::Descending), Ordering::Equal);
    assert_eq!(
        due.compare(&labels(&[]), &labels(&[]), Direction::Ascending),
        Ordering::Equal
    );
}

#[test]
fn an_item_carrying_two_values_sorts_by_the_earliest() {
    let vocabulary = vocabulary();
    let due = vocabulary.sort_key("due").unwrap();
    let items = [
        ("february", labels(&["due=2026-02-01"])),
        (
            "january and march",
            labels(&["due=2026-03-01", "due=2026-01-01"]),
        ),
    ];

    assert_eq!(sorted(&due, Direction::Ascending, &items), [
        "january and march",
        "february"
    ]);
    assert_eq!(sorted(&due, Direction::Descending, &items), [
        "february",
        "january and march"
    ]);
}
