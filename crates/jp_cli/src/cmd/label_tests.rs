use clap::Parser as _;

use super::*;

#[derive(Debug, clap::Parser)]
struct Mutating {
    #[command(flatten)]
    labels: LabelDirectives<true, false>,
}

#[derive(Debug, clap::Parser)]
struct SetOnly {
    #[command(flatten)]
    labels: LabelDirectives<false, false>,
}

#[derive(Debug, clap::Parser)]
struct WithAliases {
    #[command(flatten)]
    labels: LabelDirectives<true, true>,
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

/// Label values are user-controlled text; only the first `=` separates the key.
#[test]
fn values_may_contain_separators() {
    assert_eq!(parse(&["jp", "--label=branch=feat/a=b,c"]), [
        LabelDirective::Set {
            key: "branch".to_owned(),
            value: "feat/a=b,c".to_owned()
        },
    ]);
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
