use indoc::indoc;

use super::*;

const VOCABULARY: &str = indoc! {r#"
    {
      "active": {
        "app/macos": "The native macOS app.",
        "cli": "The command-line surface.",
        "config": "Configuration loading and merging."
      },
      "retired": {
        "legacy-ui": "The pre-rewrite terminal UI."
      }
    }
"#};

fn vocabulary() -> Vocabulary {
    Vocabulary::parse(VOCABULARY).unwrap()
}

fn owned(labels: &[&str]) -> Vec<String> {
    labels.iter().map(|label| (*label).to_owned()).collect()
}

#[test]
fn reads_active_and_retired_labels() {
    let vocabulary = vocabulary();

    assert_eq!(vocabulary.names().collect::<Vec<_>>(), [
        "app/macos",
        "cli",
        "config"
    ]);
    assert_eq!(vocabulary.retired_names().collect::<Vec<_>>(), [
        "legacy-ui"
    ]);
    assert_eq!(
        vocabulary.description("app/macos"),
        Some("The native macOS app.")
    );
    assert_eq!(
        vocabulary.description("legacy-ui"),
        Some("The pre-rewrite terminal UI.")
    );
    assert_eq!(vocabulary.description("nope"), None);
}

/// A board with no vocabulary file at all reads as an empty one, so listing a
/// ticket never depends on the file being there.
#[test]
fn an_empty_file_is_an_empty_vocabulary() {
    assert_eq!(Vocabulary::parse("").unwrap(), Vocabulary::default());
    assert_eq!(Vocabulary::parse("  \n").unwrap(), Vocabulary::default());
}

/// `retired` is for boards that have retired something; most have not.
#[test]
fn retired_is_optional() {
    let vocabulary = Vocabulary::parse(r#"{"active": {"cli": "The CLI."}}"#).unwrap();

    assert_eq!(vocabulary.names().collect::<Vec<_>>(), ["cli"]);
    assert_eq!(vocabulary.retired_names().count(), 0);
}

/// A bare map of label to description is the shape someone reaches for first.
/// It has to fail loudly: parsed leniently it would read as an empty vocabulary
/// and refuse every label, blaming the caller for the file's problem.
#[test]
fn a_flat_map_is_refused_rather_than_read_as_empty() {
    assert!(matches!(
        Vocabulary::parse(r#"{"cli": "The CLI."}"#),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn a_malformed_file_is_an_error() {
    assert!(matches!(
        Vocabulary::parse(r#"["cli"]"#),
        Err(Error::Malformed(_))
    ));
}

/// Nothing can say whether such a label may be added, so the file is wrong
/// rather than ambiguous.
#[test]
fn a_label_in_both_lists_is_an_error() {
    let source = r#"{"active": {"cli": "a"}, "retired": {"cli": "b"}}"#;

    assert_eq!(
        Vocabulary::parse(source),
        Err(Error::BothActiveAndRetired(vec!["cli".to_owned()]))
    );
}

#[test]
fn resolves_to_the_vocabularys_spelling() {
    let resolved = vocabulary().resolve(&owned(&["  CLI ", "config"])).unwrap();

    assert_eq!(join(&resolved), "cli, config");
}

/// The label line is a set: the order it was typed in says nothing, and asking
/// for one label twice is asking for it once.
#[test]
fn resolving_sorts_and_deduplicates() {
    let resolved = vocabulary()
        .resolve(&owned(&["config", "app/macos", "CONFIG"]))
        .unwrap();

    assert_eq!(join(&resolved), "app/macos, config");
}

#[test]
fn empty_entries_are_dropped() {
    let resolved = vocabulary().resolve(&owned(&["", "   ", "cli"])).unwrap();

    assert_eq!(join(&resolved), "cli");
}

/// The case this split exists for: an old ticket carries a retired label, and
/// adding a new one must not force the retired one off first.
#[test]
fn a_retired_label_already_on_the_ticket_can_be_kept() {
    let resolved = vocabulary()
        .resolve_against(&owned(&["legacy-ui", "cli"]), &owned(&["legacy-ui"]))
        .unwrap();

    assert_eq!(join(&resolved), "cli, legacy-ui");
}

#[test]
fn a_retired_label_not_on_the_ticket_is_refused() {
    let error = vocabulary()
        .resolve_against(&owned(&["legacy-ui", "cli"]), &owned(&["config"]))
        .unwrap_err();

    assert_eq!(error.retired, ["legacy-ui"]);
    assert!(error.unknown.is_empty());
    assert_eq!(
        error.to_string(),
        "`legacy-ui` is retired and can only stay on a ticket that already carries it. Labels you \
         can add: app/macos, cli, config."
    );
}

/// A new ticket carries nothing, so there is nothing for a retired label to
/// stay on.
#[test]
fn a_new_ticket_cannot_take_a_retired_label() {
    let error = vocabulary().resolve(&owned(&["legacy-ui"])).unwrap_err();

    assert_eq!(error.retired, ["legacy-ui"]);
}

/// Keeping a retired label is matched the same way as everything else.
#[test]
fn keeping_a_retired_label_ignores_case() {
    let resolved = vocabulary()
        .resolve_against(&owned(&["LEGACY-UI"]), &owned(&["legacy-ui"]))
        .unwrap();

    assert_eq!(join(&resolved), "legacy-ui");
}

/// One call reports every problem, so a caller fixing them doesn't have to
/// discover them one at a time.
#[test]
fn every_rejection_is_reported_at_once() {
    let error = vocabulary()
        .resolve(&owned(&["clii", "cli", "legacy-ui", "storage"]))
        .unwrap_err();

    assert_eq!(error.unknown, ["clii", "storage"]);
    assert_eq!(error.retired, ["legacy-ui"]);
    assert_eq!(
        error.to_string(),
        "`clii`, `storage` are not known labels. `legacy-ui` is retired and can only stay on a \
         ticket that already carries it. Labels you can add: app/macos, cli, config."
    );
}

#[test]
fn one_unknown_label_reads_as_one() {
    let error = vocabulary().resolve(&owned(&["storage"])).unwrap_err();

    assert_eq!(
        error.to_string(),
        "`storage` is not a known label. Labels you can add: app/macos, cli, config."
    );
}

/// A board that hasn't defined any labels should say so, rather than listing an
/// empty set and leaving the caller to guess where labels come from.
#[test]
fn an_empty_vocabulary_says_where_labels_come_from() {
    let error = Vocabulary::default().resolve(&owned(&["cli"])).unwrap_err();

    assert_eq!(
        error.to_string(),
        "`cli` is not a known label. This board defines no labels; add them to `.labels.json` in \
         the ticket directory."
    );
}

#[test]
fn splits_a_metadata_value() {
    assert_eq!(split("app/macos, config"), ["app/macos", "config"]);
    assert_eq!(split("app/macos,config"), ["app/macos", "config"]);
    assert_eq!(split("  app/macos ,, "), ["app/macos"]);
    assert!(split("").is_empty());
}
