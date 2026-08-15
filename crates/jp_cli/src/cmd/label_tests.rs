use clap::Parser as _;

use super::*;

/// The `jp query` shape: `--label` with aliases, no reset.
#[derive(Debug, clap::Parser)]
struct Aliased {
    #[command(flatten)]
    labels: LabelDirectives<true, false>,
}

/// The `jp conversation fork` shape: no aliases, with `--reset-labels`.
#[derive(Debug, clap::Parser)]
struct Resettable {
    #[command(flatten)]
    labels: LabelDirectives<false, true>,
}

#[derive(Debug, clap::Parser)]
struct Filtering {
    #[arg(long = "label")]
    labels: Vec<LabelSelector>,
}

fn parse(args: &[&str]) -> Vec<LabelDirective> {
    Aliased::try_parse_from(args).unwrap().labels.0
}

// ── Flag parsing ─────────────────────────────────────────────────────────────

#[test]
fn key_value_and_bare_keys_parse() {
    assert_eq!(parse(&["jp", "--label=team=platform", "--label=draft"]), [
        LabelDirective::Set {
            key: "team".to_owned(),
            value: "platform".to_owned()
        },
        LabelDirective::Set {
            key: "draft".to_owned(),
            value: String::new()
        },
    ]);
}

/// One flag carries one label, so a value needs no escaping: commas, equals
/// signs and spaces are all just characters.
#[test]
fn values_are_taken_literally() {
    assert_eq!(parse(&["jp", "--label=branch=feat,exp"]), [
        LabelDirective::Set {
            key: "branch".to_owned(),
            value: "feat,exp".to_owned()
        },
    ]);

    assert_eq!(parse(&["jp", "--label=expr=a=b,c=d"]), [
        LabelDirective::Set {
            key: "expr".to_owned(),
            value: "a=b,c=d".to_owned()
        },
    ]);
}

/// The plural spelling is gone: one label per flag, repeated.
#[test]
fn the_plural_spelling_is_not_accepted() {
    assert!(Aliased::try_parse_from(["jp", "--labels=a=1"]).is_err());
}

#[test]
fn invalid_keys_are_rejected_at_parse_time() {
    let error = Aliased::try_parse_from(["jp", "--label=team.platform=x"]).unwrap_err();
    assert!(error.to_string().contains("invalid character '.'"));
}

/// A key that starts with `-` would be read as a flag where keys are written as
/// bare arguments, so the grammar requires a leading letter.
#[test]
fn keys_must_start_with_a_letter() {
    for raw in ["--label=-lead=x", "--label=1st=x", "--label=_x=y"] {
        let error = Aliased::try_parse_from(["jp", raw]).unwrap_err();
        let error = error.to_string();
        assert!(
            error.contains("starts with") || error.contains("unexpected argument"),
            "got: {error}"
        );
    }
}

#[test]
fn alias_syntax_parses_where_supported() {
    assert_eq!(parse(&["jp", "--label=:branch", "--label=a=1"]), [
        LabelDirective::Alias("branch".to_owned()),
        LabelDirective::Set {
            key: "a".to_owned(),
            value: "1".to_owned()
        },
    ]);
}

/// A command that may target several conversations can't resolve a rule, so it
/// points at the command that can.
#[test]
fn alias_syntax_is_rejected_without_alias_support() {
    let error = Resettable::try_parse_from(["jp", "--label=:branch"]).unwrap_err();
    let error = error.to_string();
    assert!(error.contains("not supported here"), "got: {error}");
    assert!(error.contains("conversation label add"), "got: {error}");
}

#[test]
fn an_alias_name_must_be_a_valid_key() {
    let error = Aliased::try_parse_from(["jp", "--label=:a.b"]).unwrap_err();
    assert!(error.to_string().contains("invalid character '.'"));
}

// ── `--reset-labels` ─────────────────────────────────────────────────────────

#[test]
fn reset_is_unavailable_where_it_is_not_registered() {
    assert!(Aliased::try_parse_from(["jp", "--reset-labels"]).is_err());
}

/// `--reset-labels` is positioned like any other directive: it drops what came
/// before it and leaves what comes after.
#[test]
fn reset_takes_its_place_in_command_line_order() {
    let parsed = Resettable::try_parse_from(["jp", "--label=a=1", "--reset-labels", "--label=b=2"])
        .unwrap()
        .labels
        .0;

    assert_eq!(parsed, [
        LabelDirective::Set {
            key: "a".to_owned(),
            value: "1".to_owned()
        },
        LabelDirective::RemoveAll,
        LabelDirective::Set {
            key: "b".to_owned(),
            value: "2".to_owned()
        },
    ]);
}

/// `--reset-labels` takes no value, so it can never swallow the argument that
/// follows it.
#[test]
fn reset_consumes_no_value() {
    let parsed = Resettable::try_parse_from(["jp", "--reset-labels", "--label=a=1"])
        .unwrap()
        .labels
        .0;

    assert_eq!(parsed, [LabelDirective::RemoveAll, LabelDirective::Set {
        key: "a".to_owned(),
        value: "1".to_owned()
    },]);
}

// ── Bare-argument parsing, as `jp c label` uses it ───────────────────────────

#[test]
fn bare_arguments_parse_as_set_directives() {
    assert_eq!(
        LabelDirective::parse_set::<true>("team=platform").unwrap(),
        LabelDirective::Set {
            key: "team".to_owned(),
            value: "platform".to_owned()
        }
    );
    assert_eq!(
        LabelDirective::parse_set::<true>("draft").unwrap(),
        LabelDirective::Set {
            key: "draft".to_owned(),
            value: String::new()
        }
    );
    assert_eq!(
        LabelDirective::parse_set::<true>(":branch").unwrap(),
        LabelDirective::Alias("branch".to_owned())
    );
}

/// Removal names keys, so a `key=value` argument is rejected rather than
/// silently treated as a key.
#[test]
fn removal_takes_keys_not_pairs() {
    let error = LabelDirective::parse_remove("team=platform").unwrap_err();
    assert!(error.contains("invalid character '='"), "got: {error}");
}

/// A conversation ID is a perfectly good label key now that keys and
/// conversation targets no longer share an argument slot.
#[test]
fn a_conversation_id_is_an_ordinary_key() {
    assert_eq!(
        LabelDirective::parse_remove("jp-c17866928997").unwrap(),
        LabelDirective::Remove("jp-c17866928997".to_owned())
    );
}

// ── Application ──────────────────────────────────────────────────────────────

fn resolved(directives: Vec<LabelDirective>) -> Resolved {
    Resolved(directives)
}

#[test]
fn directives_apply_in_order() {
    let mut labels = BTreeMap::from([("keep".to_owned(), "yes".to_owned())]);

    apply(
        &mut labels,
        &resolved(vec![
            LabelDirective::Set {
                key: "branch".to_owned(),
                value: "main".to_owned(),
            },
            LabelDirective::Set {
                key: "branch".to_owned(),
                value: "feat".to_owned(),
            },
        ]),
    );

    assert_eq!(
        labels,
        BTreeMap::from([
            ("keep".to_owned(), "yes".to_owned()),
            ("branch".to_owned(), "feat".to_owned()),
        ])
    );
}

#[test]
fn remove_all_clears_then_later_sets_apply() {
    let mut labels = BTreeMap::from([
        ("a".to_owned(), "1".to_owned()),
        ("b".to_owned(), "2".to_owned()),
    ]);

    apply(
        &mut labels,
        &resolved(vec![LabelDirective::RemoveAll, LabelDirective::Set {
            key: "c".to_owned(),
            value: "3".to_owned(),
        }]),
    );

    assert_eq!(labels, BTreeMap::from([("c".to_owned(), "3".to_owned())]));
}

/// Removing a key the conversation doesn't carry is not an error — removal is
/// idempotent — but it is reported, so a directive that did nothing is
/// visible.
#[test]
fn removing_an_absent_key_is_reported() {
    let mut labels = BTreeMap::from([("kept".to_owned(), "yes".to_owned())]);

    let missing = apply(
        &mut labels,
        &resolved(vec![
            LabelDirective::Remove("absent".to_owned()),
            LabelDirective::Remove("kept".to_owned()),
            LabelDirective::Remove("alsoabsent".to_owned()),
        ]),
    );

    assert_eq!(missing, ["absent", "alsoabsent"], "in the order given");
    assert!(labels.is_empty(), "the present key was still removed");
}

/// A key set earlier in the same invocation counts as present: the check runs
/// against the live map, not the starting one.
#[test]
fn a_key_set_then_removed_is_not_reported_missing() {
    let mut labels = BTreeMap::new();

    let missing = apply(
        &mut labels,
        &resolved(vec![
            LabelDirective::Set {
                key: "tmp".to_owned(),
                value: "1".to_owned(),
            },
            LabelDirective::Remove("tmp".to_owned()),
        ]),
    );

    assert!(missing.is_empty(), "got: {missing:?}");
    assert!(labels.is_empty());
}

#[test]
fn a_reset_never_reports_missing() {
    let mut labels = BTreeMap::new();

    let missing = apply(&mut labels, &resolved(vec![LabelDirective::RemoveAll]));

    assert!(missing.is_empty());
}

// ── Filters ──────────────────────────────────────────────────────────────────

#[test]
fn selectors_and_together() {
    let filter = Filtering::try_parse_from(["jp", "--label=team=platform", "--label=draft"])
        .unwrap()
        .labels;

    let matching = BTreeMap::from([
        ("team".to_owned(), "platform".to_owned()),
        ("draft".to_owned(), String::new()),
    ]);
    assert!(matches(&matching, &filter));

    // Present-only selectors accept any value, including an empty one.
    let wrong_value = BTreeMap::from([
        ("team".to_owned(), "infra".to_owned()),
        ("draft".to_owned(), "yes".to_owned()),
    ]);
    assert!(!matches(&wrong_value, &filter));

    let missing_key = BTreeMap::from([("team".to_owned(), "platform".to_owned())]);
    assert!(!matches(&missing_key, &filter));
}

#[test]
fn an_empty_filter_matches_everything() {
    assert!(matches(&BTreeMap::new(), &[]));
}

/// Filters read persisted labels, so a rule name has nothing to resolve
/// against; the error names the resolved syntax rather than complaining about
/// the `:` character.
#[test]
fn alias_syntax_is_rejected_in_filters() {
    let error = Filtering::try_parse_from(["jp", "--label=:branch"]).unwrap_err();
    let error = error.to_string();
    assert!(error.contains("cannot be used as a filter"), "got: {error}");
    assert!(error.contains("--label=branch=VALUE"), "got: {error}");
}

// ── Alias expansion ──────────────────────────────────────────────────────────

fn alias_resolver_rules(
    json: &str,
) -> indexmap::IndexMap<String, jp_config::conversation::label::LabelConfig> {
    serde_json::from_str(json).unwrap()
}

#[tokio::test]
async fn expand_aliases_replaces_aliases_and_keeps_order() {
    let rules = alias_resolver_rules(r#"{ "stage": "review" }"#);
    let tmp = camino_tempfile::tempdir().unwrap();
    let (printer, _out, _err) = jp_printer::Printer::memory(jp_printer::OutputFormat::Text);
    let prompts = jp_inquire::prompt::MockPromptBackend::new();
    let resolver = resolve::Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let directives = parse(&["jp", "--label=a=1", "--label=:stage"]);
    let expanded = expand_aliases(&directives, &resolver).await.unwrap();

    assert_eq!(*expanded, [
        LabelDirective::Set {
            key: "a".to_owned(),
            value: "1".to_owned()
        },
        LabelDirective::Set {
            key: "stage".to_owned(),
            value: "review".to_owned()
        },
    ]);
}

#[tokio::test]
async fn expand_aliases_propagates_an_unknown_name() {
    let rules = alias_resolver_rules(r#"{ "stage": "review" }"#);
    let tmp = camino_tempfile::tempdir().unwrap();
    let (printer, _out, _err) = jp_printer::Printer::memory(jp_printer::OutputFormat::Text);
    let prompts = jp_inquire::prompt::MockPromptBackend::new();
    let resolver = resolve::Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let directives = parse(&["jp", "--label=:missing"]);
    let error = expand_aliases(&directives, &resolver)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("unknown label alias"), "got: {error}");
}
