use serde_json::json;

use super::*;

fn name(qualifier: &[&str], ident: &str) -> Name {
    Name {
        qualifier: qualifier.iter().map(|s| (*s).to_owned()).collect(),
        ident: ident.to_owned(),
    }
}

#[test]
fn parses_bare_and_qualified_names() {
    assert_eq!(Name::parse("iter"), Ok(name(&[], "iter")));
    assert_eq!(
        Name::parse(" Stream::iter() "),
        Ok(name(&["Stream"], "iter"))
    );
    assert_eq!(
        Name::parse("jp_conversation::ConversationStream"),
        Ok(name(&["jp_conversation"], "ConversationStream"))
    );
    assert_eq!(Name::parse("r#type"), Ok(name(&[], "r#type")));
}

#[test]
fn refuses_a_name_without_an_identifier() {
    assert_eq!(Name::parse("  "), Err("`name` is empty.".to_owned()));
    assert_eq!(
        Name::parse("stream.iter"),
        Err(
            "`stream.iter` does not end in an identifier. Give a name such as `iter` or \
             `Stream::iter`."
                .to_owned()
        )
    );
}

#[test]
fn a_raw_identifier_is_searched_without_its_prefix() {
    assert_eq!(name(&[], "r#type").bare_ident(), "type");
    assert_eq!(name(&[], "iter").bare_ident(), "iter");
}

#[test]
fn a_name_knows_its_query_and_parent() {
    let full = name(&["stream", "Stream"], "config");

    assert_eq!(full.query(), "stream::Stream::config");
    assert_eq!(full.parent(), Some(name(&["stream"], "Stream")));
    assert_eq!(name(&[], "config").parent(), None);
}

#[test]
fn a_qualifier_fits_by_type_or_by_module_path() {
    let fits = |qualifier: &[&str], path: &str, container: Option<&str>| {
        let qualifier = qualifier
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>();
        qualifier_fits(&qualifier, path, container)
    };
    let stream = "crates/jp_conversation/src/stream.rs";

    // The type a method is implemented on, generics and all.
    assert!(fits(
        &["ConversationStream"],
        stream,
        Some("ConversationStream")
    ));
    assert!(fits(
        &["Turn"],
        "crates/jp_conversation/src/stream/turn_iter.rs",
        Some("Turn<'a>")
    ));
    // Module path, then type, in order.
    assert!(fits(
        &["jp_conversation", "ConversationStream"],
        stream,
        Some("ConversationStream")
    ));
    assert!(fits(&["jp_conversation"], stream, None));
    assert!(fits(&[], stream, None));

    assert!(!fits(&["Other"], stream, Some("ConversationStream")));
    assert!(!fits(
        &["ConversationStream", "jp_conversation"],
        stream,
        Some("ConversationStream")
    ));
    // Whole segments only.
    assert!(!fits(&["Conversation"], stream, Some("ConversationStream")));
}

#[test]
fn finds_whole_word_occurrences() {
    let line = "let items = self.items.iter().map(|item| item_iter(iter));";

    assert_eq!(occurrences(line, "iter"), vec![23, 51]);
    assert_eq!(occurrences(line, "items"), vec![4, 17]);
    assert_eq!(occurrences(line, "tem"), Vec::<usize>::new());
}

#[test]
fn picks_an_occurrence_counting_from_one() {
    let line = "stream.iter().zip(other.iter())";

    assert_eq!(pick("self.iter()", "iter", None), Ok(5));
    assert_eq!(pick(line, "iter", Some(2)), Ok(24));
    assert_eq!(
        pick(line, "iter", None),
        Err(
            "`iter` appears 2 times on that line. Pass `occurrence` to pick one, counting from 1."
                .to_owned()
        )
    );
    assert_eq!(
        pick(line, "iter", Some(3)),
        Err("`iter` appears 2 times on that line, so `occurrence` must be 1 to 2.".to_owned())
    );
    assert_eq!(
        pick(line, "render", None),
        Err("`render` does not appear on that line.".to_owned())
    );
}

#[test]
fn keeps_symbols_named_exactly_once_per_location() {
    let symbol = |name: &str, line: u32| {
        serde_json::from_value::<SymbolInformation>(json!({
            "name": name,
            "kind": 6,
            "location": {
                "uri": "file:///work/src/lib.rs",
                "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": 4 } },
            },
        }))
        .unwrap()
    };

    let found = exact(
        vec![
            symbol("iter", 1),
            symbol("iter_turns", 2),
            symbol("Iter", 3),
            symbol("iter", 1),
            symbol("iter", 4),
        ],
        "iter",
    );

    let lines = found
        .iter()
        .map(|symbol| symbol.location.range.start.line)
        .collect::<Vec<_>>();
    assert_eq!(lines, vec![1, 4]);
}
