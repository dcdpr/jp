use super::{
    super::{protocol::Position, tests::fixture},
    *,
};

fn items() -> Vec<Item> {
    flatten(&fixture::stream_symbols())
}

fn name(items: &[Item], index: Option<usize>) -> Option<&str> {
    index.map(|index| items[index].name.as_str())
}

#[test]
fn names_items_through_their_enclosing_items() {
    let items = items();

    let names = items
        .iter()
        .map(|item| {
            (
                item.name.as_str(),
                item.depth,
                item.scope.join("::"),
                item.in_tests,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(names, vec![
        ("Stream", 0, String::new(), false),
        ("Stream::items", 1, "Stream".to_owned(), false),
        ("Stream::config", 1, "Stream".to_owned(), false),
        ("impl Stream", 0, String::new(), false),
        ("Stream::iter", 1, "Stream".to_owned(), false),
        ("impl Render for Stream", 0, String::new(), false),
        ("<Stream as Render>::render", 1, "Stream".to_owned(), false),
        ("total", 0, String::new(), false),
        ("tests", 0, String::new(), false),
        ("tests::counts", 1, "tests".to_owned(), true),
        ("more_tests", 0, String::new(), false),
    ]);
}

#[test]
fn the_innermost_item_encloses_a_line() {
    let items = items();

    // `let total: u32 = self.iter().sum();`
    assert_eq!(
        name(&items, enclosing(&items, 21)),
        Some("<Stream as Render>::render")
    );
    // The header of `impl Render for Stream`, outside the method.
    assert_eq!(
        name(&items, enclosing(&items, 19)),
        Some("impl Render for Stream")
    );
    // A field's type belongs to the struct, not the field.
    assert_eq!(name(&items, enclosing(&items, 9)), Some("Stream"));
    assert_eq!(name(&items, enclosing(&items, 37)), Some("tests::counts"));
    // `use crate::Render;`
    assert_eq!(name(&items, enclosing(&items, 0)), None);
}

#[test]
fn finds_the_item_named_at_a_position() {
    let items = items();

    let at_name = at(&items, Position {
        line: 9,
        character: 10,
    });
    let in_body = at(&items, Position {
        line: 15,
        character: 8,
    });

    assert_eq!(name(&items, at_name), Some("Stream::config"));
    assert_eq!(name(&items, in_body), Some("Stream::iter"));
}

#[test]
fn an_item_is_defined_where_its_name_starts() {
    let items = items();

    // `pub struct Stream`, at the name.
    assert!(defines(&items, Position {
        line: 6,
        character: 11
    }));
    // `use crate::Render;` is not a document symbol.
    assert!(!defines(&items, Position {
        line: 0,
        character: 11
    }));
    // Inside a body is not at a name.
    assert!(!defines(&items, Position {
        line: 15,
        character: 8
    }));
}

#[test]
fn a_qualifier_matches_the_enclosing_names() {
    let items = items();
    let names = |qualifier: &[&str], ident: &str| {
        let qualifier = qualifier
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>();
        matching(&items, &qualifier, ident)
            .into_iter()
            .map(|index| items[index].name.as_str())
            .collect::<Vec<_>>()
    };

    assert_eq!(names(&["Stream"], "iter"), vec!["Stream::iter"]);
    assert_eq!(names(&[], "render"), vec!["<Stream as Render>::render"]);
    assert_eq!(names(&["Stream"], "config"), vec!["Stream::config"]);
    assert_eq!(names(&["Other"], "iter"), Vec::<&str>::new());
}

#[test]
fn finds_children_by_name() {
    let items = items();

    let found = children_named(&items, 0, "config");

    assert_eq!(found, vec![2]);
}

#[test]
fn code_starts_after_doc_comments_and_attributes() {
    let items = items();
    let lines = fixture::stream_lines();

    assert_eq!(code_start(&items[0], &lines), 6);
    assert_eq!(code_start(&items[4], &lines), 14);
    assert_eq!(code_start(&items[7], &lines), 26);
}

#[test]
fn reads_the_first_doc_paragraph_from_the_source() {
    let items = items();
    let lines = fixture::stream_lines();
    let doc = |index: usize, max: usize| doc_paragraph(&items[index], &lines, max);

    // `Stream`'s doc has a second paragraph, behind the derive attribute.
    assert_eq!(doc(0, 6), vec!["A stream of numbers."]);
    assert_eq!(doc(1, 6), vec!["The items."]);
    assert_eq!(doc(4, 6), vec!["Iterate over the items."]);
    assert_eq!(doc(7, 6), Vec::<String>::new());
    assert_eq!(doc(0, 0), vec!["..."]);
}
