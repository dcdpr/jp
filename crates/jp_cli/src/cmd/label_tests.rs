use clap::Parser as _;

use super::*;

#[derive(Debug, clap::Parser)]
struct Filtering {
    #[arg(long = "label")]
    labels: Vec<LabelSelector>,
}

/// Parse operands the way `add` and `set` do, with aliases accepted.
fn parse(raw: &[&str]) -> Vec<LabelOperand> {
    raw.iter()
        .map(|raw| LabelOperand::parse(raw, true).unwrap())
        .collect()
}

/// Resolve operands without a resolver, which only alias-free input allows.
fn resolved(raw: &[&str], aliases: bool) -> Resolved {
    Resolved(
        raw.iter()
            .map(|raw| match LabelOperand::parse(raw, aliases).unwrap() {
                LabelOperand::Pair { key, value } => (key, value),
                LabelOperand::Alias(name) => panic!("alias ':{name}' needs a resolver"),
            })
            .collect(),
    )
}

fn pair(key: &str, value: &str) -> LabelOperand {
    LabelOperand::Pair {
        key: key.to_owned(),
        value: Some(value.to_owned()),
    }
}

fn bare(key: &str) -> LabelOperand {
    LabelOperand::Pair {
        key: key.to_owned(),
        value: None,
    }
}

// ── Operand parsing ──────────────────────────────────────────────────────────

#[test]
fn key_value_and_bare_keys_parse() {
    assert_eq!(parse(&["team=platform", "draft"]), [
        pair("team", "platform"),
        bare("draft"),
    ]);
}

/// One argument carries one label, so a value needs no escaping: commas, equals
/// signs and spaces are all just characters.
#[test]
fn values_are_taken_literally() {
    assert_eq!(parse(&["branch=feat,exp"]), [pair("branch", "feat,exp")]);
    assert_eq!(parse(&["expr=a=b,c=d"]), [pair("expr", "a=b,c=d")]);
}

#[test]
fn invalid_keys_are_rejected() {
    let error = LabelOperand::parse("team.platform=x", true).unwrap_err();
    assert!(error.contains("invalid character '.'"), "got: {error}");
}

/// A key that starts with `-` would be read as a flag where keys are written as
/// bare arguments, so the grammar requires a leading letter.
///
/// `jp c label set` leans on the same rule: it marks its output with `-` and
/// `+`, which no label line can begin with on its own.
#[test]
fn keys_must_start_with_a_letter() {
    for raw in ["-lead=x", "+lead=x", "1st=x", "_x=y"] {
        let error = LabelOperand::parse(raw, true).unwrap_err();
        assert!(error.contains("starts with"), "got: {error}");
    }
}

#[test]
fn alias_syntax_parses_where_supported() {
    assert_eq!(parse(&[":branch", "a=1"]), [
        LabelOperand::Alias("branch".to_owned()),
        pair("a", "1"),
    ]);
}

/// `rm` names values already stored rather than a rule to resolve, so `:name`
/// is nothing but an invalid key there.
#[test]
fn an_alias_is_an_invalid_key_where_aliases_are_off() {
    let error = LabelOperand::parse(":branch", false).unwrap_err();
    assert!(error.contains("starts with ':'"), "got: {error}");
}

#[test]
fn an_alias_name_must_be_a_valid_key() {
    let error = LabelOperand::parse(":a.b", true).unwrap_err();
    assert!(error.contains("invalid character '.'"), "got: {error}");
}

/// Removal takes a bare key or a pair, so a `key=value` argument names one
/// value rather than being rejected.
#[test]
fn removal_takes_keys_and_pairs() {
    assert_eq!(
        LabelOperand::parse("team=platform", false).unwrap(),
        pair("team", "platform")
    );
    assert_eq!(LabelOperand::parse("team", false).unwrap(), bare("team"));
}

/// A conversation ID is a perfectly good label key, since keys and conversation
/// targets do not share an argument slot.
#[test]
fn a_conversation_id_is_an_ordinary_key() {
    assert_eq!(
        LabelOperand::parse("jp-c17866928997", false).unwrap(),
        bare("jp-c17866928997")
    );
}

// ── Grouping ─────────────────────────────────────────────────────────────────

/// A key named several times is applied once, with every value it was given.
/// Without that, `set k=a k=b` would replace the set twice and leave `{b}`.
#[test]
fn values_group_under_their_key_in_the_order_given() {
    let grouped = resolved(&["crate=jp_config", "team=platform", "crate=jp_llm"], true).grouped();

    assert_eq!(grouped.keys().collect::<Vec<_>>(), ["crate", "team"]);
    assert_eq!(grouped["crate"].iter().collect::<Vec<_>>(), [
        "jp_config",
        "jp_llm"
    ]);
}

#[test]
fn a_repeated_value_is_grouped_once() {
    let grouped = resolved(&["crate=jp_llm", "crate=jp_llm"], true).grouped();

    assert_eq!(grouped["crate"].iter().collect::<Vec<_>>(), ["jp_llm"]);
}

#[test]
fn a_bare_key_groups_as_the_empty_value() {
    let grouped = resolved(&["draft"], true).grouped();

    assert_eq!(grouped["draft"].iter().collect::<Vec<_>>(), [""]);
}

/// For removal a bare key names the whole key, which is an empty value set.
#[test]
fn a_bare_key_groups_as_the_whole_key_for_removal() {
    let grouped = resolved(&["draft", "crate=jp_llm"], false).grouped_for_removal();

    assert!(grouped["draft"].is_empty());
    assert_eq!(grouped["crate"].iter().collect::<Vec<_>>(), ["jp_llm"]);
}

/// Removing the key removes its values too, so naming both is the key.
#[test]
fn a_whole_key_removal_absorbs_the_values_named_alongside_it() {
    for raw in [["crate", "crate=jp_llm"], ["crate=jp_llm", "crate"]] {
        let grouped = resolved(&raw, false).grouped_for_removal();
        assert!(grouped["crate"].is_empty(), "got: {grouped:?}");
    }
}

// ── Application ──────────────────────────────────────────────────────────────

#[test]
fn add_accumulates_into_the_keys_set() {
    let mut labels = Labels::from_iter([("crate", ["jp_config"])]);

    let applied = apply(
        &mut labels,
        &LabelChange::Add(resolved(&["crate=jp_llm"], true).grouped()),
    );

    assert_eq!(
        labels,
        Labels::from_iter([("crate", ["jp_config", "jp_llm"])])
    );
    assert_eq!(applied.changes, [Change {
        key: "crate".to_owned(),
        before: IndexSet::from(["jp_config".to_owned()]),
        after: IndexSet::from(["jp_config".to_owned(), "jp_llm".to_owned()]),
    }]);
}

#[test]
fn set_replaces_the_keys_set_and_leaves_other_keys_alone() {
    let mut labels = Labels::from_iter([
        ("crate", vec!["jp_config", "jp_llm"]),
        ("team", vec!["platform"]),
    ]);

    let applied = apply(
        &mut labels,
        &LabelChange::Set(resolved(&["crate=jp_cli"], true).grouped()),
    );

    assert_eq!(
        labels,
        Labels::from_iter([("crate", ["jp_cli"]), ("team", ["platform"])])
    );
    assert_eq!(applied.changes, [Change {
        key: "crate".to_owned(),
        before: IndexSet::from(["jp_config".to_owned(), "jp_llm".to_owned()]),
        after: IndexSet::from(["jp_cli".to_owned()]),
    }]);
}

#[test]
fn removing_a_pair_takes_one_value_and_leaves_the_rest() {
    let mut labels = Labels::from_iter([("crate", ["jp_config", "jp_llm"])]);

    let applied = apply(
        &mut labels,
        &LabelChange::Remove(resolved(&["crate=jp_llm"], false).grouped_for_removal()),
    );

    assert_eq!(labels, Labels::from_iter([("crate", ["jp_config"])]));
    assert_eq!(applied.changes, [Change {
        key: "crate".to_owned(),
        before: IndexSet::from(["jp_config".to_owned(), "jp_llm".to_owned()]),
        after: IndexSet::from(["jp_config".to_owned()]),
    }]);
    assert!(applied.missing.is_empty());
}

#[test]
fn removing_a_bare_key_takes_every_value_it_held() {
    let mut labels = Labels::from_iter([
        ("crate", vec!["jp_config", "jp_llm"]),
        ("team", vec!["platform"]),
    ]);

    let applied = apply(
        &mut labels,
        &LabelChange::Remove(resolved(&["crate"], false).grouped_for_removal()),
    );

    assert_eq!(labels, Labels::from_iter([("team", ["platform"])]));
    assert_eq!(applied.changes, [Change {
        key: "crate".to_owned(),
        before: IndexSet::from(["jp_config".to_owned(), "jp_llm".to_owned()]),
        after: IndexSet::new(),
    }]);
}

#[test]
fn remove_all_takes_every_label() {
    let mut labels = Labels::from_iter([("crate", ["jp_llm"]), ("draft", [""])]);

    let applied = apply(&mut labels, &LabelChange::RemoveAll);

    assert!(labels.is_empty());
    assert_eq!(applied.changes, [
        Change {
            key: "crate".to_owned(),
            before: IndexSet::from(["jp_llm".to_owned()]),
            after: IndexSet::new(),
        },
        Change {
            key: "draft".to_owned(),
            before: IndexSet::from([String::new()]),
            after: IndexSet::new(),
        },
    ]);
    assert!(applied.missing.is_empty());
}

/// Removing something the conversation doesn't carry is not an error — removal
/// is idempotent — but it is reported, so an operand that did nothing is
/// visible.
#[test]
fn removing_what_is_not_there_is_reported() {
    let mut labels = Labels::from_iter([("crate", ["jp_config"])]);

    let applied = apply(
        &mut labels,
        &LabelChange::Remove(
            resolved(&["absent", "crate=jp_llm", "crate=jp_config"], false).grouped_for_removal(),
        ),
    );

    assert_eq!(
        applied.missing,
        [
            ("absent".to_owned(), None),
            ("crate".to_owned(), Some("jp_llm".to_owned())),
        ],
        "in the order given"
    );
    assert!(labels.is_empty(), "the value that was there is gone");
}

/// A key whose named values were all absent changed nothing, so it reports no
/// change to undo.
#[test]
fn a_removal_that_matched_nothing_reports_no_change() {
    let mut labels = Labels::from_iter([("crate", ["jp_config"])]);

    let applied = apply(
        &mut labels,
        &LabelChange::Remove(resolved(&["crate=jp_llm"], false).grouped_for_removal()),
    );

    assert!(applied.changes.is_empty());
    assert_eq!(applied.missing, [(
        "crate".to_owned(),
        Some("jp_llm".to_owned())
    )]);
}

/// Adding a value the key already holds is honest about having changed nothing,
/// rather than pretending the key was untouched.
#[test]
fn adding_a_value_the_key_already_holds_reports_both_sides_the_same() {
    let mut labels = Labels::from_iter([("crate", ["jp_config"])]);

    let applied = apply(
        &mut labels,
        &LabelChange::Add(resolved(&["crate=jp_config"], true).grouped()),
    );

    assert_eq!(applied.changes, [Change {
        key: "crate".to_owned(),
        before: IndexSet::from(["jp_config".to_owned()]),
        after: IndexSet::from(["jp_config".to_owned()]),
    }]);
}

// ── Filters ──────────────────────────────────────────────────────────────────

#[test]
fn selectors_and_together() {
    let filter = Filtering::try_parse_from(["jp", "--label=team=platform", "--label=draft"])
        .unwrap()
        .labels;

    let matching = Labels::from_iter([("team", ["platform"]), ("draft", [""])]);
    assert!(matches(&matching, &filter));

    // Present-only selectors accept any value, including an empty one.
    let wrong_value = Labels::from_iter([("team", ["infra"]), ("draft", ["yes"])]);
    assert!(!matches(&wrong_value, &filter));

    let missing_key = Labels::from_iter([("team", ["platform"])]);
    assert!(!matches(&missing_key, &filter));
}

/// A key holding several values matches on membership, so a filter naming one
/// of them finds the conversation, and naming two requires both.
#[test]
fn a_selector_matches_any_value_the_key_holds() {
    let one = Filtering::try_parse_from(["jp", "--label=crate=jp_llm"])
        .unwrap()
        .labels;
    let both = Filtering::try_parse_from(["jp", "--label=crate=jp_llm", "--label=crate=jp_cli"])
        .unwrap()
        .labels;

    let labels = Labels::from_iter([("crate", ["jp_config", "jp_llm"])]);
    assert!(matches(&labels, &one));
    assert!(!matches(&labels, &both));
}

#[test]
fn an_empty_filter_matches_everything() {
    assert!(matches(&Labels::default(), &[]));
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

    let operands = parse(&["a=1", ":stage"]);
    let expanded = expand_aliases(&operands, &resolver).await.unwrap();

    assert_eq!(
        expanded,
        Resolved(vec![
            ("a".to_owned(), Some("1".to_owned())),
            ("stage".to_owned(), Some("review".to_owned())),
        ])
    );
}

/// An alias contributes its values to its key's group, alongside anything else
/// named for that key.
#[tokio::test]
async fn an_alias_joins_its_keys_group() {
    let rules = alias_resolver_rules(r#"{ "stage": "review" }"#);
    let tmp = camino_tempfile::tempdir().unwrap();
    let (printer, _out, _err) = jp_printer::Printer::memory(jp_printer::OutputFormat::Text);
    let prompts = jp_inquire::prompt::MockPromptBackend::new();
    let resolver = resolve::Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let operands = parse(&["stage=draft", ":stage"]);
    let grouped = expand_aliases(&operands, &resolver)
        .await
        .unwrap()
        .grouped();

    assert_eq!(grouped["stage"].iter().collect::<Vec<_>>(), [
        "draft", "review"
    ]);
}

#[tokio::test]
async fn expand_aliases_propagates_an_unknown_name() {
    let rules = alias_resolver_rules(r#"{ "stage": "review" }"#);
    let tmp = camino_tempfile::tempdir().unwrap();
    let (printer, _out, _err) = jp_printer::Printer::memory(jp_printer::OutputFormat::Text);
    let prompts = jp_inquire::prompt::MockPromptBackend::new();
    let resolver = resolve::Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let operands = parse(&[":missing"]);
    let error = expand_aliases(&operands, &resolver)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("unknown label alias"), "got: {error}");
}
