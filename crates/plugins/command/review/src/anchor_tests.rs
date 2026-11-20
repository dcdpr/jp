use pretty_assertions::assert_eq;

use super::*;

fn tracked() -> Vec<String> {
    vec!["docs/a.md".to_owned(), "notes:old.md".to_owned()]
}

/// Finds a tracked path written exactly as recorded.
fn exact(typed: &str) -> Option<String> {
    tracked().into_iter().find(|path| path == typed)
}

#[test]
fn each_target_is_stored_with_only_what_it_names() {
    let range: Range = "42:5-43:18".parse().unwrap();
    let cases = [
        (Target::Review, r#"{"version":2}"#),
        (
            Target::File("docs/a.md".to_owned()),
            r#"{"version":2,"path":"docs/a.md"}"#,
        ),
        (
            Target::Range("docs/a.md".to_owned(), range),
            r#"{"version":2,"path":"docs/a.md","range":"42:5-43:18"}"#,
        ),
    ];

    for (target, json) in cases {
        let anchor = Anchor { version: 2, target };

        assert_eq!(serde_json::to_string(&anchor).unwrap(), json);
        assert_eq!(serde_json::from_str::<Anchor>(json).unwrap(), anchor);
    }
}

#[test]
fn a_stored_range_without_a_file_is_refused() {
    let error = serde_json::from_str::<Anchor>(r#"{"version":2,"range":"4"}"#).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("the anchor's range 4 names no file"),
        "{error}"
    );
}

#[test]
fn at_names_a_file_alone_or_with_a_range() {
    assert_eq!(
        place("docs/a.md", &tracked(), exact).unwrap(),
        ("docs/a.md".to_owned(), None)
    );
    assert_eq!(
        place("docs/a.md:42:5-43:18", &tracked(), exact).unwrap(),
        ("docs/a.md".to_owned(), Some("42:5-43:18".parse().unwrap()))
    );
    assert_eq!(
        place("docs/a.md:60-64", &tracked(), exact).unwrap(),
        ("docs/a.md".to_owned(), Some("60-64".parse().unwrap()))
    );
}

#[test]
fn a_path_holding_a_colon_is_read_as_a_path_first() {
    assert_eq!(
        place("notes:old.md", &tracked(), exact).unwrap(),
        ("notes:old.md".to_owned(), None)
    );
    assert_eq!(
        place("notes:old.md:3", &tracked(), exact).unwrap(),
        ("notes:old.md".to_owned(), Some("3".parse().unwrap()))
    );
}

#[test]
fn an_untracked_path_names_the_tracked_ones() {
    assert_eq!(
        place("docs/b.md:4", &tracked(), exact).unwrap_err(),
        "`docs/b.md:4` names no file in the review, which tracks docs/a.md, notes:old.md."
    );
}

#[test]
fn a_tracked_path_with_a_malformed_range_says_what_is_wrong_with_the_range() {
    assert_eq!(
        place("docs/a.md:9-4", &tracked(), exact).unwrap_err(),
        "The range `9-4` ends before it starts."
    );
}
