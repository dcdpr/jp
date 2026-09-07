use super::*;

#[test]
fn keys_follow_the_grammar() {
    assert_eq!(validate_key(""), Err(KeyError::EmptyKey));
    assert!(matches!(
        validate_key("1client"),
        Err(KeyError::KeyStart { .. })
    ));
    assert!(matches!(
        validate_key("-client"),
        Err(KeyError::KeyStart { .. })
    ));
    assert!(matches!(validate_key("cli.ent"), Err(KeyError::Key { .. })));
    assert!(matches!(validate_key("cli:ent"), Err(KeyError::Key { .. })));
    assert!(validate_key("client").is_ok());
    assert!(validate_key("apply-on_2").is_ok());
}

/// The messages are user-facing on `jp c label`, so they are pinned rather than
/// matched loosely.
#[test]
fn key_errors_name_the_character_and_the_grammar() {
    assert_eq!(
        validate_key("cli.ent").unwrap_err().to_string(),
        "invalid character '.' in label key 'cli.ent': a label key starts with an ASCII letter, \
         followed by any number of letters, digits, underscores, and hyphens"
    );
    assert_eq!(
        validate_key("1client").unwrap_err().to_string(),
        "label key '1client' starts with '1': a label key starts with an ASCII letter, followed \
         by any number of letters, digits, underscores, and hyphens"
    );
    assert_eq!(
        validate_key("").unwrap_err().to_string(),
        "label key must not be empty"
    );
}

/// [`Labels`] folds line breaks rather than refusing them, so this check exists
/// for a consumer *declaring* a value it will later have to match.
#[test]
fn declared_values_must_be_usable_as_written() {
    assert!(matches!(
        validate_value("two\nlines"),
        Err(KeyError::Value { .. })
    ));
    assert!(matches!(
        validate_value(" padded"),
        Err(KeyError::Value { .. })
    ));
    assert!(validate_value("jp_cli").is_ok());
    assert!(validate_value("feat,exp").is_ok());
}

#[test]
fn a_token_splits_on_the_first_equals() {
    assert_eq!(
        parse_token("crate=jp_cli").unwrap(),
        ("crate".to_owned(), Some("jp_cli".to_owned()))
    );
    assert_eq!(
        parse_token("expr=a=b").unwrap(),
        ("expr".to_owned(), Some("a=b".to_owned()))
    );
    assert_eq!(parse_token("draft").unwrap(), ("draft".to_owned(), None));
}

/// `key=` names a key with no value, which is the same thing a bare `key` says.
#[test]
fn an_empty_value_reads_as_a_bare_key() {
    assert_eq!(parse_token("draft=").unwrap(), ("draft".to_owned(), None));
}

#[test]
fn a_selector_renders_back_to_its_token() {
    for token in ["crate=jp_cli", "draft"] {
        assert_eq!(Selector::parse(token).unwrap().to_string(), token);
    }
}

#[test]
fn parsing_selectors_stops_at_the_first_bad_key() {
    assert!(Selector::parse_all(["crate=jp_cli", "1bad"]).is_err());
}
