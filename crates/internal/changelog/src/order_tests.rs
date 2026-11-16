use std::fs;

use camino_tempfile::Utf8TempDir;

use super::*;

#[test]
fn parse_reads_both_lists() {
    let order =
        Order::parse(r#"{ "highlights": ["story.md"], "entries": ["b.added.md", "a.fixed.md"] }"#)
            .unwrap();

    assert_eq!(order, Order {
        highlights: vec!["story.md".to_owned()],
        entries: vec!["b.added.md".to_owned(), "a.fixed.md".to_owned()],
    });
}

#[test]
fn parse_takes_either_list_alone() {
    let order = Order::parse(r#"{ "entries": ["a.added.md"] }"#).unwrap();

    assert_eq!(order.highlights, Vec::<String>::new());
    assert_eq!(order.entries, ["a.added.md"]);
}

#[test]
fn parse_skips_a_byte_order_mark() {
    let order = Order::parse("\u{feff}{}").unwrap();

    assert_eq!(order, Order::default());
}

#[test]
fn parse_refuses_a_misspelled_list() {
    let error = Order::parse(r#"{ "changelogs": ["a.added.md"] }"#).unwrap_err();

    assert!(
        error.to_string().starts_with(
            "not an order, which is a JSON object of `highlights` and `entries`, each a list of \
             file names: unknown field `changelogs`"
        ),
        "{error}"
    );
}

#[test]
fn parse_refuses_a_list_of_anything_but_names() {
    let error = Order::parse(r#"{ "entries": [1] }"#).unwrap_err();

    assert!(matches!(error, OrderError::Malformed(_)), "{error}");
}

#[test]
fn positions_follow_the_lists() {
    let order = Order {
        highlights: vec!["story.md".to_owned()],
        entries: vec!["b.added.md".to_owned(), "a.fixed.md".to_owned()],
    };

    assert_eq!(order.entry("a.fixed.md"), Some(1));
    assert_eq!(order.entry("b.added.md"), Some(0));
    assert_eq!(order.entry("c.added.md"), None);
    assert_eq!(order.highlight("story.md"), Some(0));
    assert_eq!(order.highlight("b.added.md"), None);
}

#[test]
fn read_defaults_without_a_file() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(Order::read(dir.path()).unwrap(), Order::default());
}

#[test]
fn read_parses_the_file() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join(ORDER), r#"{ "entries": ["a.added.md"] }"#).unwrap();

    assert_eq!(Order::read(dir.path()).unwrap().entries, ["a.added.md"]);
}
