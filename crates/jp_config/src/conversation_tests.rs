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
