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

    assert_eq!(labels.len(), 2);
    assert_eq!(
        labels.get("team").unwrap().value(),
        LabelValueRef::Static("platform")
    );

    let stage = labels.get("stage").unwrap();
    assert_eq!(stage.value(), LabelValueRef::Static("review"));
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
