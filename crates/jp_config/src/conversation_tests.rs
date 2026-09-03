use super::*;

#[test]
fn parse_ask() {
    assert_eq!(
        "ask".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::Ask
    );
}

#[test]
fn parse_last_aliases() {
    assert_eq!(
        "last".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::LastActivated
    );
    assert_eq!(
        "last-activated".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::LastActivated
    );
    assert_eq!(
        "last_activated".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::LastActivated
    );
}

#[test]
fn parse_last_created() {
    assert_eq!(
        "last-created".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::LastCreated
    );
    assert_eq!(
        "last_created".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::LastCreated
    );
}

#[test]
fn parse_previous_aliases() {
    assert_eq!(
        "previous".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::Previous
    );
    assert_eq!(
        "prev".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::Previous
    );
}

#[test]
fn parse_conversation_id_fallback() {
    let result = "jp-c17528832001".parse::<DefaultConversationId>().unwrap();
    assert_eq!(result, DefaultConversationId::Id("jp-c17528832001".into()));
}

#[test]
fn deserialize_from_toml() {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        id: DefaultConversationId,
    }

    let w: Wrapper = toml::from_str("id = \"last\"").unwrap();
    assert_eq!(w.id, DefaultConversationId::LastActivated);

    let w: Wrapper = toml::from_str("id = \"ask\"").unwrap();
    assert_eq!(w.id, DefaultConversationId::Ask);

    let w: Wrapper = toml::from_str("id = \"jp-c17528832001\"").unwrap();
    assert_eq!(w.id, DefaultConversationId::Id("jp-c17528832001".into()));
}

#[test]
fn default_is_ask() {
    assert!(DefaultConversationId::default().is_ask());
}

#[test]
fn deserialize_attachments_dedup_from_toml() {
    // [attachments] with dedup = true and no value key.
    let toml = r"
        [attachments]
        dedup = true
    ";

    let partial: PartialConversationConfig = toml::from_str(toml).unwrap();
    assert!(partial.attachments.dedup());
    assert_eq!(partial.attachments.len(), 0, "no attachment items");
}

#[test]
fn deserialize_attachments_dedup_via_app_config() {
    // Same thing but through PartialAppConfig (the real runtime path).
    let toml = r"
        [conversation.attachments]
        dedup = true
    ";

    let partial: crate::PartialAppConfig = toml::from_str(toml).unwrap();
    assert!(partial.conversation.attachments.dedup());
    assert_eq!(
        partial.conversation.attachments.len(),
        0,
        "no attachment items"
    );
}

#[test]
fn deserialize_attachments_dedup_via_schematic_loader() {
    // Through schematic's ConfigLoader — the actual runtime path.
    use camino_tempfile::tempdir;
    use schematic::ConfigLoader;

    // Explicit dedup in config file.
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    std::fs::write(&path, "[conversation.attachments]\ndedup = true\n").unwrap();

    let partial = ConfigLoader::<crate::AppConfig>::new()
        .file(&*path)
        .unwrap()
        .load_partial(&())
        .unwrap();

    assert!(partial.conversation.attachments.dedup());
    assert_eq!(partial.conversation.attachments.len(), 0);
}

#[test]
fn dedup_inherits_from_default_via_build() {
    // Full production path: ConfigLoader + build().
    // ConfigLoader::load_partial doesn't apply default_values, but
    // build() -> from_partial_with_defaults -> fill_from applies dedup
    // via fill_attachments_defaults.
    use camino_tempfile::tempdir;
    use schematic::ConfigLoader;

    use crate::util;

    let tmp = tempdir().unwrap();
    let path = tmp.path().join("config.toml");

    std::fs::write(&path, indoc::indoc! {r#"
            [assistant.model]
            id = "anthropic/test"

            [conversation.tools.'*']
            run = "unattended"

            [[conversation.attachments]]
            type = "file"
            path = "/tmp/a"
        "#})
    .unwrap();

    let partial = ConfigLoader::<crate::AppConfig>::new()
        .file(&*path)
        .unwrap()
        .load_partial(&())
        .unwrap();

    let config = util::build(partial).unwrap();
    assert_eq!(config.conversation.attachments.len(), 1);
}

#[test]
fn merge_partials_preserves_dedup() {
    use schematic::PartialConfig as _;

    // Simulate what ConfigLoader does: default + file partial merge.
    let mut default = PartialConversationConfig::default();
    let file: PartialConversationConfig = toml::from_str(
        r"
        [attachments]
        dedup = true
    ",
    )
    .unwrap();

    default.merge(&(), file).unwrap();
    assert!(
        default.attachments.dedup(),
        "after merge: {:?}",
        default.attachments
    );
}

#[test]
fn deserialize_dedup_inherit_from_toml() {
    let toml = r#"
        [attachments]
        dedup = "inherit"
    "#;

    let partial: PartialConversationConfig = toml::from_str(toml).unwrap();
    // "inherit" maps to None — no opinion on dedup.
    assert!(!partial.attachments.dedup());
}

#[test]
fn deserialize_labels_from_toml() {
    use crate::conversation::label::PartialLabelConfig;

    let toml = r#"
        [labels]
        team = "platform"

        [labels.stage]
        value = "review"
        apply_on = { fork = true }
    "#;

    let partial: PartialConversationConfig = toml::from_str(toml).unwrap();

    assert_eq!(
        partial.labels.get("team"),
        Some(&PartialLabelConfig::Static("platform".to_owned()))
    );

    let PartialLabelConfig::Object(stage) = partial.labels.get("stage").unwrap() else {
        panic!("expected the object form");
    };
    assert_eq!(stage.apply_on.fork, Some(true));
    assert_eq!(stage.apply_on.new, None, "unset fields stay absent");
}

#[test]
fn merge_labels_deep_merges_entries() {
    use schematic::PartialConfig as _;

    use crate::conversation::label::PartialLabelConfig;

    let mut base: PartialConversationConfig =
        toml::from_str("[labels]\nteam = \"platform\"\n").unwrap();
    let next: PartialConversationConfig = toml::from_str("[labels]\nbranch = \"main\"\n").unwrap();

    base.merge(&(), next).unwrap();

    assert_eq!(
        base.labels.get("team"),
        Some(&PartialLabelConfig::Static("platform".to_owned()))
    );
    assert_eq!(
        base.labels.get("branch"),
        Some(&PartialLabelConfig::Static("main".to_owned()))
    );
}

/// A rule's values replace rather than extend across layers, so a narrower
/// layer can shorten an inherited list.
#[test]
fn merge_labels_replaces_a_rules_values() {
    use schematic::PartialConfig as _;

    use crate::conversation::label::PartialLabelConfig;

    let mut base: PartialConversationConfig =
        toml::from_str("[labels]\ncrate = [\"jp_config\", \"jp_llm\"]\n").unwrap();
    let next: PartialConversationConfig =
        toml::from_str("[labels]\ncrate = [\"jp_cli\"]\n").unwrap();

    base.merge(&(), next).unwrap();

    assert_eq!(
        base.labels.get("crate"),
        Some(&PartialLabelConfig::List(vec!["jp_cli".to_owned()]))
    );
}

#[test]
fn merge_labels_honors_replace_strategy() {
    use schematic::PartialConfig as _;

    let mut base: PartialConversationConfig =
        toml::from_str("[labels]\nteam = \"platform\"\n").unwrap();
    let next: PartialConversationConfig = toml::from_str(
        r#"
        [labels]
        strategy = "replace"
        value = { branch = "main" }
    "#,
    )
    .unwrap();

    base.merge(&(), next).unwrap();

    assert!(base.labels.get("team").is_none(), "replace drops the base");
    assert!(base.labels.get("branch").is_some());
}

#[test]
fn labels_survive_the_full_build() {
    use camino_tempfile::tempdir;
    use schematic::ConfigLoader;

    use crate::{conversation::label::LabelValueRef, util};

    let tmp = tempdir().unwrap();
    let path = tmp.path().join("config.toml");

    std::fs::write(&path, indoc::indoc! {r#"
            [assistant.model]
            id = "anthropic/test"

            [conversation.tools.'*']
            run = "unattended"

            [conversation.labels]
            team = "platform"
            crate = ["jp_config", "jp_llm"]

            [conversation.labels.stage]
            value = "review"
            apply_on = { new = false, fork = true }
        "#})
    .unwrap();

    let partial = ConfigLoader::<crate::AppConfig>::new()
        .file(&*path)
        .unwrap()
        .load_partial(&())
        .unwrap();

    let config = util::build(partial).unwrap();
    let labels = &config.conversation.labels;

    assert_eq!(labels.len(), 3);
    assert_eq!(
        labels.get("team").unwrap().value(),
        LabelValueRef::Values(&["platform".to_owned()])
    );
    assert_eq!(
        labels.get("crate").unwrap().value(),
        LabelValueRef::Values(&["jp_config".to_owned(), "jp_llm".to_owned()])
    );

    let stage = labels.get("stage").unwrap();
    assert_eq!(stage.value(), LabelValueRef::Values(&["review".to_owned()]));
    assert!(!stage.apply_on().new);
    assert!(stage.apply_on().fork);
}

/// A replacing layer drops a rule, and the recorded delta has to carry that
/// removal.
/// A minimal entry-wise delta cannot: a missing entry means "unchanged", so the
/// dropped rule would deep-merge back in on the next fold.
#[test]
fn label_delta_carries_a_removal_through_the_fold() {
    use schematic::PartialConfig as _;

    use crate::conversation::label::PartialLabelConfig;

    let prev: PartialConversationConfig =
        toml::from_str("[labels]\nteam = \"platform\"\nbranch = \"main\"\n").unwrap();

    // What a `--cfg` layer with `strategy = "replace"` resolves to: `team` is
    // gone, `branch` survives.
    let next: PartialConversationConfig = toml::from_str("[labels]\nbranch = \"main\"\n").unwrap();

    let delta = prev.delta(next);

    // Fold the delta back over the previous state, as the event stream does.
    let mut folded = prev;
    folded.merge(&(), delta).unwrap();

    assert!(
        folded.labels.get("team").is_none(),
        "the dropped rule must not come back: {:?}",
        folded.labels
    );
    assert_eq!(
        folded.labels.get("branch"),
        Some(&PartialLabelConfig::Static("main".to_owned()))
    );
}

/// The common case stays minimal: adding a rule records only that rule.
#[test]
fn label_delta_stays_minimal_when_nothing_is_dropped() {
    let prev: PartialConversationConfig =
        toml::from_str("[labels]\nteam = \"platform\"\n").unwrap();
    let next: PartialConversationConfig =
        toml::from_str("[labels]\nteam = \"platform\"\nbranch = \"main\"\n").unwrap();

    let delta = prev.delta(next);

    assert_eq!(delta.labels.len(), 1, "got: {:?}", delta.labels);
    assert!(delta.labels.contains_key("branch"));
}

/// A rule's values are assigned the way every other list setting is: a comma
/// separates, a JSON array is taken element-wise, `:+=` adds to what the key
/// already names, and `:=` replaces it.
#[test]
fn assign_labels_from_the_command_line() {
    use crate::{assignment::KvAssignment, conversation::label::PartialLabelConfig};

    let list = |values: &[&str]| {
        Some(PartialLabelConfig::List(
            values.iter().map(|v| (*v).to_owned()).collect(),
        ))
    };

    let mut partial = PartialConversationConfig::default();

    let kv = KvAssignment::try_from_cli("labels.crate", "jp_config,jp_llm").unwrap();
    partial.assign(kv).unwrap();
    assert_eq!(
        partial.labels.get("crate").cloned(),
        list(&["jp_config", "jp_llm"])
    );

    let kv = KvAssignment::try_from_cli("labels.crate:+", r#"["jp_cli"]"#).unwrap();
    partial.assign(kv).unwrap();
    assert_eq!(
        partial.labels.get("crate").cloned(),
        list(&["jp_config", "jp_llm", "jp_cli"])
    );

    let kv = KvAssignment::try_from_cli("labels.crate:", r#"["jp_cli"]"#).unwrap();
    partial.assign(kv).unwrap();
    assert_eq!(partial.labels.get("crate").cloned(), list(&["jp_cli"]));
}

/// The comma shorthand belongs to the bare form, so the JSON form names a value
/// containing one — as a string or as an array element.
#[test]
fn assign_a_label_value_containing_a_comma() {
    use crate::{assignment::KvAssignment, conversation::label::PartialLabelConfig};

    let mut partial = PartialConversationConfig::default();

    for value in [r#""feat,exp""#, r#"["feat,exp"]"#] {
        let kv = KvAssignment::try_from_cli("labels.branch:", value).unwrap();
        partial.assign(kv).unwrap();

        assert_eq!(
            partial.labels.get("branch"),
            Some(&PartialLabelConfig::List(vec!["feat,exp".to_owned()])),
            "got: {value}"
        );
    }
}

/// An empty string names no values, as it does for every other list setting, so
/// a bare label is written as a one-element array.
#[test]
fn assign_a_bare_label() {
    use crate::{assignment::KvAssignment, conversation::label::PartialLabelConfig};

    let mut partial = PartialConversationConfig::default();

    let kv = KvAssignment::try_from_cli("labels.draft", "").unwrap();
    partial.assign(kv).unwrap();
    assert_eq!(
        partial.labels.get("draft"),
        Some(&PartialLabelConfig::List(vec![])),
        "an empty string names nothing, so the rule produces no label"
    );

    for value in [r#""""#, r#"[""]"#] {
        let kv = KvAssignment::try_from_cli("labels.draft:", value).unwrap();
        partial.assign(kv).unwrap();

        assert_eq!(
            partial.labels.get("draft"),
            Some(&PartialLabelConfig::List(vec![String::new()])),
            "the empty value is a value, and the JSON form names it; got: {value}"
        );
    }
}

/// An empty item between two commas is a typo, not a value.
///
/// The whole value being empty names nothing, which is how a list is cleared;
/// `a,,b` asks for a value that isn't there, so it is refused rather than
/// quietly producing two.
#[test]
fn assign_rejects_an_empty_item_in_a_list() {
    use crate::assignment::KvAssignment;

    let mut partial = PartialConversationConfig::default();

    for value in ["jp_config,,jp_llm", "jp_config,", ",jp_llm"] {
        let kv = KvAssignment::try_from_cli("labels.crate", value).unwrap();
        let error = partial.assign(kv).unwrap_err();

        // The CLI prints the whole cause chain, and the reason lives in the
        // innermost cause.
        let chain: Vec<_> =
            std::iter::successors(Some(&*error as &dyn std::error::Error), |error| {
                error.source()
            })
            .map(ToString::to_string)
            .collect();

        assert!(
            chain
                .iter()
                .any(|cause| cause == &format!("empty value in '{value}'")),
            "unexpected causes for {value:?}: {chain:?}"
        );
    }
}

/// A rule declared in a config file is the base an assignment lands on, so a
/// merge adds to the value it already names rather than dropping it.
#[test]
fn assign_merges_onto_a_scalar_rule() {
    use crate::{assignment::KvAssignment, conversation::label::PartialLabelConfig};

    let mut partial: PartialConversationConfig =
        toml::from_str("[labels]\nteam = \"platform\"\n").unwrap();

    let kv = KvAssignment::try_from_cli("labels.team:+", r#"["urgent"]"#).unwrap();
    partial.assign(kv).unwrap();

    assert_eq!(
        partial.labels.get("team"),
        Some(&PartialLabelConfig::List(vec![
            "platform".to_owned(),
            "urgent".to_owned()
        ]))
    );
}

/// Naming values on a rule that carries `apply_on` and `run` lands on its
/// `value`, so the assignment cannot silently change which creation path
/// applies the rule.
#[test]
fn assign_values_onto_an_object_rule_keeps_its_policy() {
    use crate::{
        assignment::KvAssignment,
        conversation::label::{LabelRunMode, PartialLabelConfig, PartialLabelValue},
    };

    let mut partial: PartialConversationConfig = toml::from_str(
        "[labels.stage]\nvalue = \"review\"\napply_on = { new = false, fork = true }\nrun = \
         \"unattended\"\n",
    )
    .unwrap();

    for (key, value, expected) in [
        ("labels.stage", "ready", vec!["ready".to_owned()]),
        ("labels.stage:+", r#"["queued"]"#, vec![
            "ready".to_owned(),
            "queued".to_owned(),
        ]),
    ] {
        let kv = KvAssignment::try_from_cli(key, value).unwrap();
        partial.assign(kv).unwrap();

        let Some(PartialLabelConfig::Object(object)) = partial.labels.get("stage") else {
            panic!("expected the object form to survive; got: {key}={value}")
        };
        assert_eq!(object.value, PartialLabelValue::List(expected));
        assert_eq!(object.apply_on.new, Some(false), "got: {key}={value}");
        assert_eq!(object.apply_on.fork, Some(true), "got: {key}={value}");
        assert_eq!(
            object.run,
            Some(LabelRunMode::Unattended),
            "got: {key}={value}"
        );
    }
}

/// The full form is the object-shaped value, so naming `value` and `apply_on`
/// still works.
#[test]
fn assign_a_whole_label_rule_as_an_object() {
    use crate::{
        assignment::KvAssignment,
        conversation::label::{PartialLabelConfig, PartialLabelValue},
    };

    let mut partial = PartialConversationConfig::default();

    let kv = KvAssignment::try_from_cli(
        "labels.stage:",
        r#"{"value":"review","apply_on":{"fork":true}}"#,
    )
    .unwrap();
    partial.assign(kv).unwrap();

    let Some(PartialLabelConfig::Object(object)) = partial.labels.get("stage") else {
        panic!("expected the object form");
    };
    assert_eq!(object.value, PartialLabelValue::Static("review".to_owned()));
    assert_eq!(object.apply_on.fork, Some(true));
}

#[test]
fn assign_a_json_array_to_an_existing_rules_value() {
    use crate::{
        assignment::KvAssignment,
        conversation::label::{PartialLabelConfig, PartialLabelValue},
    };

    let mut partial: PartialConversationConfig =
        toml::from_str("[labels.crate]\nvalue = \"jp_config\"\napply_on = { fork = true }\n")
            .unwrap();

    let kv = KvAssignment::try_from_cli("labels.crate.value", "jp_llm,jp_cli").unwrap();
    partial.assign(kv).unwrap();

    let Some(PartialLabelConfig::Object(object)) = partial.labels.get("crate") else {
        panic!("expected the object form");
    };
    assert_eq!(
        object.value,
        PartialLabelValue::List(vec!["jp_llm".to_owned(), "jp_cli".to_owned()])
    );
    assert_eq!(
        object.apply_on.fork,
        Some(true),
        "the rest of the rule is untouched"
    );
}

#[test]
fn invalid_label_key_is_rejected_by_build() {
    use crate::{
        conversation::tool::RunMode,
        model::id::{PartialModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
        util,
    };

    let mut partial = crate::PartialAppConfig::default();
    partial.conversation.tools.defaults.run = Some(RunMode::Ask);
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
        provider: Some(ProviderId::Anthropic),
        name: "claude-opus-4".parse().ok(),
    });
    partial.conversation.labels.insert(
        "team.platform".to_owned(),
        crate::conversation::label::PartialLabelConfig::Static("x".to_owned()),
    );

    let error = util::build(partial).unwrap_err().to_string();
    assert!(error.contains("invalid character"), "got: {error}");
}

#[test]
fn deserialize_attachments_array_from_toml() {
    // [[attachments]] array-of-tables syntax.
    let toml = r#"
        [[attachments]]
        type = "file"
        path = "/tmp/test.txt"
    "#;

    let partial: PartialConversationConfig = toml::from_str(toml).unwrap();
    assert!(!partial.attachments.dedup());
    assert_eq!(partial.attachments.len(), 1);
}
