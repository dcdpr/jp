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

/// A consumer that declares no keys has no closed set to hold a label to, so
/// every well-formed label goes through, spelled as given and in a fixed order.
#[test]
fn an_empty_vocabulary_accepts_any_label() {
    let labels = Vocabulary::default()
        .resolve(&owned(&["Client=CLI", "area=ui", "triaged", "area=docs"]))
        .unwrap();

    assert_eq!(labels.to_tokens(), [
        "Client=CLI",
        "area=docs",
        "area=ui",
        "triaged"
    ]);
}

/// Only the shape of a label is checked without a vocabulary.
#[test]
fn an_empty_vocabulary_refuses_a_malformed_label() {
    let error = Vocabulary::default()
        .resolve(&owned(&["area=ui", "=ui"]))
        .unwrap_err();

    assert_eq!(error.malformed, ["=ui"]);
    assert_eq!(error.to_string(), "`=ui` is not a `key=value` label.");
}

/// A board where every item carries exactly one `type`, at least one `area`,
/// and at most one `team`.
fn bounded() -> Vocabulary {
    Vocabulary::parse(
        r#"{
            "type": {"values": ["bug", "feature"], "retired": ["chore"], "min": 1, "max": 1},
            "area": {"values": ["cli", "docs", "llm"], "min": 1},
            "team": {"values": ["core", "web"], "max": 1},
            "draft": {}
        }"#,
    )
    .unwrap()
}

#[test]
fn reads_the_bounds_of_a_key() {
    let vocabulary = bounded();
    let type_ = vocabulary.facet("type").unwrap();

    assert_eq!(type_.min(), 1);
    assert_eq!(type_.max(), Some(1));
    assert_eq!(type_.bound().as_deref(), Some("exactly one label"));
    assert_eq!(
        vocabulary.facet("area").unwrap().bound().as_deref(),
        Some("at least one label")
    );
    assert_eq!(
        vocabulary.facet("team").unwrap().bound().as_deref(),
        Some("at most one label")
    );
    assert_eq!(vocabulary.facet("draft").unwrap().bound(), None);
    assert!(!vocabulary.facet("draft").unwrap().is_bounded());
}

#[test]
fn an_item_meeting_every_bound_resolves() {
    let labels = bounded()
        .resolve(&owned(&["type=bug", "area=cli", "area=docs", "team=core"]))
        .unwrap();

    assert_eq!(labels.to_tokens(), [
        "area=cli",
        "area=docs",
        "team=core",
        "type=bug"
    ]);
}

/// The refusal names the missing key and offers its values, so the caller can
/// fix the call without looking anything up.
#[test]
fn a_required_key_left_out_is_refused_with_its_values() {
    let error = bounded().resolve(&owned(&["area=cli"])).unwrap_err();

    assert_eq!(error.miscounted.len(), 1);
    assert_eq!(error.miscounted[0].key, "type");
    assert_eq!(
        error.to_string(),
        "`type` label missing: add one of type=bug, type=feature."
    );
}

#[test]
fn more_values_than_a_key_takes_are_refused() {
    let error = bounded()
        .resolve(&owned(&[
            "type=bug",
            "type=feature",
            "area=cli",
            "team=core",
            "team=web",
        ]))
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "`team` takes at most one label, and 2 are given: team=core, team=web. `type` takes \
         exactly one label, and 2 are given: type=bug, type=feature."
    );
}

#[test]
fn fewer_values_than_a_key_needs_are_refused() {
    let vocabulary =
        Vocabulary::parse(r#"{"area": {"values": ["cli", "docs", "llm"], "min": 2}}"#).unwrap();

    let error = vocabulary.resolve(&owned(&["area=cli"])).unwrap_err();

    assert_eq!(
        error.to_string(),
        "`area` takes at least 2 labels, and 1 is given: add from area=cli, area=docs, area=llm."
    );
}

/// A refused value doesn't count toward its key, and the whole set is offered
/// for it ahead of what the bounds need.
#[test]
fn a_refused_value_counts_as_missing() {
    let error = bounded()
        .resolve(&owned(&["type=bugg", "area=cli"]))
        .unwrap_err();

    assert_eq!(error.unknown_values, ["type=bugg"]);
    assert_eq!(
        error.to_string(),
        "`type=bugg` is not a known label. Labels you can add: area=cli, area=docs, area=llm, \
         draft, team=core, team=web, type=bug, type=feature. `type` label missing: add one of \
         type=bug, type=feature."
    );
}

/// A retired value the item keeps is still one of its values, so it counts
/// toward the key's `max`.
#[test]
fn a_kept_retired_value_counts_toward_the_bound() {
    let current = Labels::from_tokens(["type=chore", "area=cli"]).unwrap();

    let kept = bounded()
        .resolve_against(&owned(&["type=chore", "area=cli"]), &current)
        .unwrap();
    assert_eq!(kept.to_tokens(), ["area=cli", "type=chore"]);

    let error = bounded()
        .resolve_against(&owned(&["type=chore", "type=bug", "area=cli"]), &current)
        .unwrap_err();
    assert_eq!(error.miscounted[0].given, ["bug", "chore"]);
}

#[test]
fn a_bound_on_a_key_without_values_is_an_error() {
    assert_eq!(
        Vocabulary::parse(r#"{"draft": {"min": 1}}"#),
        Err(Error::BoundOnBareKey {
            key: "draft".to_owned()
        })
    );
}

/// Bounds no item could meet are refused where they are declared, rather than
/// refusing every write later.
#[test]
fn bounds_no_item_can_meet_are_an_error() {
    assert_eq!(
        Vocabulary::parse(r#"{"type": {"values": ["bug"], "max": 0}}"#),
        Err(Error::ZeroMax {
            key: "type".to_owned()
        })
    );
    assert_eq!(
        Vocabulary::parse(r#"{"type": {"values": ["bug", "feature"], "min": 2, "max": 1}}"#),
        Err(Error::MinAboveMax {
            key: "type".to_owned(),
            min: 2,
            max: 1
        })
    );
    assert_eq!(
        Vocabulary::parse(r#"{"type": {"values": ["bug"], "retired": ["chore"], "min": 2}}"#),
        Err(Error::MinAboveValues {
            key: "type".to_owned(),
            min: 2,
            values: 1
        })
    );
}

/// A board whose `rfd` key takes any value, beside a bare key and a closed one.
fn open() -> Vocabulary {
    Vocabulary::parse(
        r#"{
            "rfd": {"description": "The RFD the work comes from.", "values": []},
            "draft": {},
            "client": {"values": ["cli"]}
        }"#,
    )
    .unwrap()
}

/// An empty list takes any value, and leaving `values` out takes none: the two
/// are told apart by whether the list is there.
#[test]
fn an_empty_list_of_values_takes_any_value() {
    let vocabulary = open();

    assert!(vocabulary.facet("rfd").unwrap().is_open());
    assert!(!vocabulary.facet("rfd").unwrap().is_bare());
    assert!(vocabulary.facet("draft").unwrap().is_bare());
    assert!(!vocabulary.facet("draft").unwrap().is_open());
    assert_eq!(vocabulary.open_keys().collect::<Vec<_>>(), ["rfd"]);

    let labels = vocabulary
        .resolve(&owned(&[
            "RFD=116",
            "rfd=Some Title",
            "draft",
            "client=CLI",
        ]))
        .unwrap();

    assert_eq!(labels.to_tokens(), [
        "client=cli",
        "draft",
        "rfd=116",
        "rfd=Some Title"
    ]);
}

/// A key that takes any value still takes one: alone, it says nothing.
#[test]
fn a_key_taking_any_value_needs_one() {
    let error = open().resolve(&owned(&["rfd"])).unwrap_err();

    assert_eq!(error.unknown_values, ["rfd"]);
    assert_eq!(
        error.to_string(),
        "`rfd` is not a known label. Labels you can add: client=cli, draft, and rfd=<value> with \
         any value."
    );
}

/// The listed tokens can only hold what is declared, so a key taking any value
/// has none there.
#[test]
fn a_key_taking_any_value_lists_no_tokens() {
    assert_eq!(open().tokens(), ["client=cli", "draft"]);
    assert_eq!(open().tokens_including_retired(), ["client=cli", "draft"]);
}

#[test]
fn a_required_key_taking_any_value_says_how_to_add_one() {
    let vocabulary = Vocabulary::parse(r#"{"rfd": {"values": [], "min": 1}}"#).unwrap();

    let error = vocabulary.resolve(&[]).unwrap_err();

    assert_eq!(
        error.to_string(),
        "`rfd` label missing: add one, as `rfd=<value>`, with any value."
    );
}

/// Moving every value of a key to `retired` and leaving `values = []` behind
/// would open the key to any value; it is refused, naming the shape that
/// retires them.
#[test]
fn retiring_values_of_a_key_taking_any_value_is_an_error() {
    let error = Vocabulary::parse(r#"{"client": {"values": [], "retired": ["cli"]}}"#).unwrap_err();

    assert_eq!(error, Error::RetiredOnOpenKey {
        key: "client".to_owned()
    });
    assert_eq!(
        error.to_string(),
        "`client` in the label vocabulary takes any value (`values = []`), so it has nothing to \
         retire. To retire every value of a key, list them under `retired` and leave `values` out."
    );

    let retired = Vocabulary::parse(r#"{"client": {"retired": ["cli"]}}"#).unwrap();
    assert!(!retired.facet("client").unwrap().is_open());
    assert!(retired.tokens().is_empty());
}

/// A board where each `rfd` value sits on one item, one item carries `focus`,
/// and one item carries `owner`, whatever its value.
fn unique() -> Vocabulary {
    Vocabulary::parse(
        r#"{
            "rfd": {"values": [], "unique": "key-value"},
            "focus": {"unique": "key-value"},
            "owner": {"values": ["jean", "jp"], "unique": "key"},
            "client": {"values": ["cli", "web"]}
        }"#,
    )
    .unwrap()
}

fn labels(tokens: &[&str]) -> Labels {
    Labels::from_tokens(tokens).unwrap()
}

#[test]
fn reads_how_much_of_a_key_one_item_holds() {
    let vocabulary = unique();

    assert_eq!(
        vocabulary.facet("rfd").unwrap().unique(),
        Some(Unique::KeyValue)
    );
    assert_eq!(
        vocabulary.facet("owner").unwrap().unique(),
        Some(Unique::Key)
    );
    assert_eq!(vocabulary.facet("client").unwrap().unique(), None);
    assert_eq!(Unique::KeyValue.to_string(), "key-value");
}

/// `key-value` holds each value to one item, so two items can carry the key
/// with different values.
#[test]
fn a_value_unique_key_clashes_on_a_shared_value_only() {
    let vocabulary = unique();

    assert_eq!(
        vocabulary.clashes(&labels(&["rfd=116", "rfd=117"]), &labels(&["rfd=117"])),
        [Clash {
            key: "rfd".to_owned(),
            value: Some("117".to_owned())
        }]
    );
    assert!(
        vocabulary
            .clashes(&labels(&["rfd=116"]), &labels(&["rfd=118"]))
            .is_empty()
    );
}

/// `key` holds the whole key to one item, whatever value either carries.
#[test]
fn a_key_unique_as_a_whole_clashes_on_any_value() {
    let clashes = unique().clashes(&labels(&["owner=jean"]), &labels(&["owner=jp"]));

    assert_eq!(clashes, [Clash {
        key: "owner".to_owned(),
        value: None
    }]);
    assert_eq!(clashes[0].to_string(), "owner");
}

/// A bare key has no values to tell apart, so `key-value` holds it as a whole.
#[test]
fn a_unique_bare_key_clashes_as_a_whole() {
    assert_eq!(
        unique().clashes(&labels(&["focus"]), &labels(&["focus", "client=cli"])),
        [Clash {
            key: "focus".to_owned(),
            value: None
        }]
    );
}

/// A label on another item written by hand matches without case, as it does
/// everywhere else.
#[test]
fn a_clash_matches_without_case() {
    assert_eq!(
        unique().clashes(&labels(&["rfd=abc"]), &labels(&["RFD=ABC"])),
        [Clash {
            key: "rfd".to_owned(),
            value: Some("abc".to_owned())
        }]
    );
}

#[test]
fn keys_that_are_not_unique_never_clash() {
    assert!(
        unique()
            .clashes(&labels(&["client=cli"]), &labels(&["client=cli"]))
            .is_empty()
    );
}

#[test]
fn an_unknown_uniqueness_is_an_error() {
    assert!(matches!(
        Vocabulary::parse(r#"{"rfd": {"values": [], "unique": "value"}}"#),
        Err(Error::Malformed(_))
    ));
}

/// Every item needing the key while one item may carry it fails as soon as the
/// collection holds two, so it is refused where it is declared.
#[test]
fn a_required_key_unique_as_a_whole_is_an_error() {
    assert_eq!(
        Vocabulary::parse(r#"{"owner": {"values": ["jp"], "min": 1, "unique": "key"}}"#),
        Err(Error::UniqueRequired {
            key: "owner".to_owned(),
            min: 1
        })
    );

    // Held per value, a required key can still be unique: each item carries
    // a value of its own.
    assert!(
        Vocabulary::parse(r#"{"slug": {"values": [], "min": 1, "unique": "key-value"}}"#).is_ok()
    );
}

/// A configuration file declares the vocabulary as a table, which reaches the
/// reader already parsed.
#[test]
fn reads_the_same_vocabulary_from_a_parsed_value() {
    let value: Value = serde_json::from_str(VOCABULARY).unwrap();

    assert_eq!(Vocabulary::from_value(value).unwrap(), vocabulary());
}

#[test]
fn a_parsed_value_is_checked_like_a_document() {
    let flat = serde_json::json!({ "client": "The client." });
    assert!(matches!(
        Vocabulary::from_value(flat),
        Err(Error::Malformed(_))
    ));

    let clashing = serde_json::json!({
        "client": { "values": ["cli"] },
        "Client": { "values": ["web"] },
    });
    assert!(matches!(
        Vocabulary::from_value(clashing),
        Err(Error::DuplicateKey { .. })
    ));
}
