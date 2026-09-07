use indoc::indoc;

use super::*;

const VOCABULARY: &str = indoc! {r#"
    {
      "client": {
        "description": "The client surface the work lands in.",
        "values": ["cli", "macos", "web"]
      },
      "package": {
        "description": "The crate the work lands in.",
        "values": ["jp_cli", "jp_config"],
        "retired": ["jp_legacy"]
      },
      "draft": {
        "description": "Not ready to start."
      }
    }
"#};

fn vocabulary() -> Vocabulary {
    Vocabulary::parse(VOCABULARY).unwrap()
}

fn owned(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| (*token).to_owned()).collect()
}

#[test]
fn reads_keys_values_and_descriptions() {
    let vocabulary = vocabulary();

    assert_eq!(vocabulary.keys().collect::<Vec<_>>(), [
        "client", "draft", "package"
    ]);

    let package = vocabulary.facet("package").unwrap();
    assert_eq!(package.description(), "The crate the work lands in.");
    assert_eq!(package.values().collect::<Vec<_>>(), [
        "jp_cli",
        "jp_config"
    ]);
    assert_eq!(package.retired().collect::<Vec<_>>(), ["jp_legacy"]);
    assert!(!package.is_bare());
    assert!(vocabulary.facet("draft").unwrap().is_bare());
    assert!(vocabulary.facet("nope").is_none());
}

/// A board with no vocabulary file at all reads as an empty one, so reading an
/// item never depends on the file being there.
#[test]
fn an_empty_document_is_an_empty_vocabulary() {
    assert_eq!(Vocabulary::parse("").unwrap(), Vocabulary::default());
    assert_eq!(Vocabulary::parse("  \n").unwrap(), Vocabulary::default());
}

#[test]
fn a_malformed_document_is_an_error() {
    assert!(matches!(
        Vocabulary::parse(r#"["client"]"#),
        Err(Error::Malformed(_))
    ));
}

/// A flat map of key to description is the shape someone reaches for first.
/// It has to fail loudly: parsed leniently it would read as a vocabulary that
/// refuses everything, blaming the caller for the file's problem.
#[test]
fn a_flat_map_is_refused_rather_than_read_as_empty() {
    assert!(matches!(
        Vocabulary::parse(r#"{"client": "The client."}"#),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn a_key_outside_the_grammar_is_an_error() {
    assert!(matches!(
        Vocabulary::parse(r#"{"cli.ent": {"values": ["a"]}}"#),
        Err(Error::Name(_))
    ));
}

/// Two keys differing only by case would make the canonical spelling depend on
/// sort order rather than on intent.
#[test]
fn keys_differing_only_by_case_are_an_error() {
    assert!(matches!(
        Vocabulary::parse(r#"{"client": {"values": ["a"]}, "Client": {"values": ["b"]}}"#),
        Err(Error::DuplicateKey { .. })
    ));
}

/// The check that makes retirement mean something: a value in both lists has no
/// answer to "may this be added?".
#[test]
fn a_value_that_is_both_active_and_retired_is_an_error() {
    let source = r#"{"package": {"values": ["jp_cli"], "retired": ["JP_CLI"]}}"#;

    assert!(matches!(
        Vocabulary::parse(source),
        Err(Error::DuplicateValue { .. })
    ));
}

#[test]
fn values_differing_only_by_case_are_an_error() {
    let source = r#"{"package": {"values": ["jp_cli", "JP_CLI"]}}"#;

    assert!(matches!(
        Vocabulary::parse(source),
        Err(Error::DuplicateValue { .. })
    ));
}

/// A value that wouldn't survive the round trip is refused at the boundary
/// where it is declared.
#[test]
fn a_value_that_cannot_round_trip_is_an_error() {
    assert!(matches!(
        Vocabulary::parse("{\"package\": {\"values\": [\"two\\nlines\"]}}"),
        Err(Error::Name(_))
    ));
    assert!(matches!(
        Vocabulary::parse(r#"{"package": {"values": [""]}}"#),
        Err(Error::EmptyValue { .. })
    ));
}

#[test]
fn resolves_to_the_vocabularys_spelling() {
    let resolved = vocabulary()
        .resolve(&owned(&["  Client = CLI ", "package=JP_Config"]))
        .unwrap();

    assert_eq!(resolved.to_tokens(), ["client=cli", "package=jp_config"]);
}

#[test]
fn a_bare_key_resolves_when_the_facet_takes_no_values() {
    let resolved = vocabulary().resolve(&owned(&["draft"])).unwrap();

    assert_eq!(resolved.to_tokens(), ["draft"]);
}

/// A key that accepts values needs one, or the label says nothing.
#[test]
fn a_bare_key_is_refused_when_the_facet_takes_values() {
    let error = vocabulary().resolve(&owned(&["package"])).unwrap_err();

    assert_eq!(error.unknown_values, ["package"]);
}

#[test]
fn an_unknown_key_is_refused() {
    let error = vocabulary()
        .resolve(&owned(&["team=platform"]))
        .unwrap_err();

    assert_eq!(error.unknown_keys, ["team"]);
}

#[test]
fn an_unknown_value_is_refused_with_the_addable_set() {
    let error = vocabulary()
        .resolve(&owned(&["package=jp_nope"]))
        .unwrap_err();

    assert_eq!(error.unknown_values, ["package=jp_nope"]);
    assert_eq!(
        error.to_string(),
        "`package=jp_nope` is not a known label. Labels you can add: client=cli, client=macos, \
         client=web, draft, package=jp_cli, package=jp_config."
    );
}

/// The case the active/retired split exists for: an old item carries a value
/// the vocabulary has since retired, and adding a new one must not force the
/// retired one off first.
#[test]
fn a_retired_value_already_carried_can_be_kept() {
    let current = Labels::from_tokens(["package=jp_legacy"]).unwrap();

    let resolved = vocabulary()
        .resolve_against(&owned(&["package=jp_legacy", "client=cli"]), &current)
        .unwrap();

    assert_eq!(resolved.to_tokens(), ["client=cli", "package=jp_legacy"]);
}

#[test]
fn a_retired_value_not_already_carried_is_refused() {
    let error = vocabulary()
        .resolve(&owned(&["package=jp_legacy"]))
        .unwrap_err();

    assert_eq!(error.retired, ["package=jp_legacy"]);
    assert_eq!(
        error.to_string(),
        "`package=jp_legacy` is retired and can only stay on an item that already carries it. \
         Labels you can add: client=cli, client=macos, client=web, draft, package=jp_cli, \
         package=jp_config."
    );
}

/// Keeping a retired value is matched the same way as everything else.
#[test]
fn keeping_a_retired_value_ignores_case() {
    let current = Labels::from_tokens(["package=jp_legacy"]).unwrap();

    let resolved = vocabulary()
        .resolve_against(&owned(&["package=JP_LEGACY"]), &current)
        .unwrap();

    assert_eq!(resolved.to_tokens(), ["package=jp_legacy"]);
}

/// A retired value on a *different* key is not kept just because the name
/// matches somewhere else.
#[test]
fn keeping_a_retired_value_is_scoped_to_its_key() {
    let current = Labels::from_tokens(["client=jp_legacy"]).unwrap();

    let error = vocabulary()
        .resolve_against(&owned(&["package=jp_legacy"]), &current)
        .unwrap_err();

    assert_eq!(error.retired, ["package=jp_legacy"]);
}

/// One call reports every problem, so a caller fixing them doesn't have to
/// discover them one at a time.
#[test]
fn every_rejection_is_reported_at_once() {
    let error = vocabulary()
        .resolve(&owned(&[
            "1bad",
            "team=platform",
            "package=jp_nope",
            "package=jp_legacy",
            "client=cli",
        ]))
        .unwrap_err();

    assert_eq!(error.malformed, ["1bad"]);
    assert_eq!(error.unknown_keys, ["team"]);
    assert_eq!(error.unknown_values, ["package=jp_nope"]);
    assert_eq!(error.retired, ["package=jp_legacy"]);
}

/// The advertised set is what a write may add, so a caller offered only this
/// can still name everything it needs for a fresh item.
#[test]
fn tokens_advertise_the_addable_set() {
    assert_eq!(vocabulary().tokens(), [
        "client=cli",
        "client=macos",
        "client=web",
        "draft",
        "package=jp_cli",
        "package=jp_config"
    ]);
}

/// A caller replacing an item's whole set has to be able to name a retired
/// value to keep it, so its advertised set includes them.
#[test]
fn tokens_including_retired_add_the_keepable_ones() {
    assert_eq!(vocabulary().tokens_including_retired(), [
        "client=cli",
        "client=macos",
        "client=web",
        "draft",
        "package=jp_cli",
        "package=jp_config",
        "package=jp_legacy"
    ]);
}

/// A consumer that hasn't declared anything should say so, rather than listing
/// an empty set and leaving the caller to guess where labels come from.
#[test]
fn an_empty_vocabulary_refuses_everything() {
    let error = Vocabulary::default()
        .resolve(&owned(&["client=cli"]))
        .unwrap_err();

    assert_eq!(error.unknown_keys, ["client"]);
    assert_eq!(
        error.to_string(),
        "`client` is not a known label key. This vocabulary defines no labels."
    );
}
