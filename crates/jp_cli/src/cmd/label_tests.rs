use clap::Parser as _;

use super::*;

#[derive(Debug, clap::Parser)]
struct Mutating {
    #[command(flatten)]
    labels: LabelDirectives<true, false, false>,
}

#[derive(Debug, clap::Parser)]
struct SetOnly {
    #[command(flatten)]
    labels: LabelDirectives<false, false, false>,
}

/// The full surface, as `jp c label` has it.
#[derive(Debug, clap::Parser)]
struct WithAliases {
    #[command(flatten)]
    labels: LabelDirectives<true, true, true>,
}

#[derive(Debug, clap::Parser)]
struct Filtering {
    #[arg(long = "label")]
    labels: Vec<LabelSelector>,
}

fn parse(args: &[&str]) -> Vec<LabelDirective> {
    Mutating::try_parse_from(args).unwrap().labels.0
}

/// Directives from a command that rejects aliases, ready for `apply`.
fn parse_resolved(args: &[&str]) -> Resolved {
    Mutating::try_parse_from(args).unwrap().labels.resolved()
}

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

/// Only the first `=` separates key from value, so a value may contain one.
#[test]
fn only_the_first_equals_separates() {
    assert_eq!(parse(&["jp", "--label=branch=feat/a=b"]), [
        LabelDirective::Set {
            key: "branch".to_owned(),
            value: "feat/a=b".to_owned()
        },
    ]);
}

/// Splitting is the default, and the plural spelling is a plain alias, so a
/// mistype between the two is harmless.
#[test]
fn the_flag_splits_on_commas_under_either_spelling() {
    assert_eq!(
        parse(&["jp", "--labels=team=platform,branch=main,draft"]),
        parse(&["jp", "--label=team=platform,branch=main,draft"])
    );

    assert_eq!(parse(&["jp", "--label=team=platform,branch=main,draft"]), [
        LabelDirective::Set {
            key: "team".to_owned(),
            value: "platform".to_owned()
        },
        LabelDirective::Set {
            key: "branch".to_owned(),
            value: "main".to_owned()
        },
        LabelDirective::Set {
            key: "draft".to_owned(),
            value: String::new()
        },
    ]);
}

#[test]
fn removal_splits_on_commas_under_either_spelling() {
    assert_eq!(
        parse(&["jp", "--no-labels=team,branch"]),
        parse(&["jp", "--no-label=team,branch"])
    );

    assert_eq!(parse(&["jp", "--no-label=team,branch"]), [
        LabelDirective::Remove("team".to_owned()),
        LabelDirective::Remove("branch".to_owned()),
    ]);
}

/// A bare `--no-label` still clears everything: an empty value is never split,
/// so it can't expand to zero directives.
#[test]
fn a_bare_removal_still_clears_everything() {
    assert_eq!(parse(&["jp", "--no-label"]), [LabelDirective::RemoveAll]);
}

/// Removal names keys, and a key can never contain a comma, so `--no-label`
/// needs no literal form.
/// A `key=value` argument is rejected outright.
#[test]
fn removal_takes_keys_not_pairs() {
    let error = Mutating::try_parse_from(["jp", "--no-label=team=platform"]).unwrap_err();
    assert!(error.to_string().contains("invalid character '='"));
}

/// `--raw-label` is the only way to write a comma into a value.
#[test]
fn the_raw_flag_keeps_commas_in_the_value() {
    let parsed = WithAliases::try_parse_from(["jp", "--raw-label=branch=feat,exp"])
        .unwrap()
        .labels
        .0;

    assert_eq!(parsed, [LabelDirective::Set {
        key: "branch".to_owned(),
        value: "feat,exp".to_owned()
    }]);
}

/// The escape is only worth carrying where labelling is the command's job.
#[test]
fn the_raw_flag_is_absent_from_bulk_commands() {
    assert!(Mutating::try_parse_from(["jp", "--raw-label=a=1,2"]).is_err());
    assert!(SetOnly::try_parse_from(["jp", "--raw-label=a=1,2"]).is_err());
}

/// The flags interleave by command-line position, and entries expanded from one
/// `--label` keep their left-to-right order within it.
#[test]
fn directives_interleave_in_command_line_order() {
    let parsed =
        WithAliases::try_parse_from(["jp", "--label=a=1", "--raw-label=b=2,x", "--label=c=3,d=4"])
            .unwrap()
            .labels
            .0;

    assert_eq!(parsed, [
        LabelDirective::Set {
            key: "a".to_owned(),
            value: "1".to_owned()
        },
        LabelDirective::Set {
            key: "b".to_owned(),
            value: "2,x".to_owned()
        },
        LabelDirective::Set {
            key: "c".to_owned(),
            value: "3".to_owned()
        },
        LabelDirective::Set {
            key: "d".to_owned(),
            value: "4".to_owned()
        },
    ]);
}

/// A stray comma produces an empty entry rather than being silently forgiven,
/// so a typo is reported instead of changing what the flag means.
#[test]
fn a_stray_comma_is_rejected() {
    let error = Mutating::try_parse_from(["jp", "--label=team=platform,"]).unwrap_err();
    assert!(error.to_string().contains("must not be empty"));
}

#[test]
fn set_and_remove_keep_command_line_order() {
    assert_eq!(
        parse(&[
            "jp",
            "--label=a=1",
            "--no-label=b",
            "--label=c=3",
            "--no-label",
        ]),
        [
            LabelDirective::Set {
                key: "a".to_owned(),
                value: "1".to_owned()
            },
            LabelDirective::Remove("b".to_owned()),
            LabelDirective::Set {
                key: "c".to_owned(),
                value: "3".to_owned()
            },
            LabelDirective::RemoveAll,
        ]
    );
}

#[test]
fn invalid_keys_are_rejected_at_parse_time() {
    let error = Mutating::try_parse_from(["jp", "--label=team.platform=x"]).unwrap_err();
    assert!(error.to_string().contains("invalid character '.'"));
}

/// A command that may target several conversations can't resolve a rule, so it
/// points at the single-target command instead.
#[test]
fn alias_syntax_is_rejected_without_alias_support() {
    let error = Mutating::try_parse_from(["jp", "--label=:branch"]).unwrap_err();
    let error = error.to_string();
    assert!(error.contains("not supported here"), "got: {error}");
    assert!(error.contains("conversation label"), "got: {error}");
}

#[test]
fn alias_syntax_parses_where_supported() {
    let parsed = WithAliases::try_parse_from(["jp", "--label=:branch", "--label=a=1"])
        .unwrap()
        .labels
        .0;

    assert_eq!(parsed, [
        LabelDirective::Alias("branch".to_owned()),
        LabelDirective::Set {
            key: "a".to_owned(),
            value: "1".to_owned()
        },
    ]);
}

#[test]
fn an_alias_name_must_be_a_valid_key() {
    let error = WithAliases::try_parse_from(["jp", "--label=:a.b"]).unwrap_err();
    assert!(error.to_string().contains("invalid character '.'"));
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

#[test]
fn no_label_is_unavailable_on_set_only_commands() {
    assert!(SetOnly::try_parse_from(["jp", "--no-label=a"]).is_err());
    assert!(SetOnly::try_parse_from(["jp", "--label=a=1"]).is_ok());
}

#[test]
fn directives_apply_in_order() {
    let mut labels = BTreeMap::from([("keep".to_owned(), "yes".to_owned())]);

    apply(
        &mut labels,
        &parse_resolved(&[
            "jp",
            "--label=branch=main",
            "--label=branch=feat",
            "--label=draft",
        ]),
    );

    assert_eq!(
        labels,
        BTreeMap::from([
            ("keep".to_owned(), "yes".to_owned()),
            ("branch".to_owned(), "feat".to_owned()),
            ("draft".to_owned(), String::new()),
        ])
    );
}

#[test]
fn bare_no_label_clears_every_label() {
    let mut labels = BTreeMap::from([
        ("a".to_owned(), "1".to_owned()),
        ("b".to_owned(), "2".to_owned()),
    ]);

    apply(
        &mut labels,
        &parse_resolved(&["jp", "--no-label", "--label=c=3"]),
    );

    assert_eq!(labels, BTreeMap::from([("c".to_owned(), "3".to_owned())]));
}

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
    let resolver = resolve::Resolver::new(&rules, tmp.path(), false, &printer);

    let directives =
        WithAliases::try_parse_from(["jp", "--label=a=1", "--label=:stage", "--no-label=b"])
            .unwrap()
            .labels
            .0;

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
        LabelDirective::Remove("b".to_owned()),
    ]);
}

#[tokio::test]
async fn expand_aliases_propagates_an_unknown_name() {
    let rules = alias_resolver_rules(r#"{ "stage": "review" }"#);
    let tmp = camino_tempfile::tempdir().unwrap();
    let (printer, _out, _err) = jp_printer::Printer::memory(jp_printer::OutputFormat::Text);
    let resolver = resolve::Resolver::new(&rules, tmp.path(), false, &printer);

    let directives = WithAliases::try_parse_from(["jp", "--label=:missing"])
        .unwrap()
        .labels
        .0;

    let error = expand_aliases(&directives, &resolver)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("unknown label alias"), "got: {error}");
}
