use assert_matches::assert_matches;
use schematic::PartialConfig as _;
use test_log::test;

use super::*;
use crate::assignment::{KvAssignmentError, KvAssignmentErrorKind};

#[test]
fn test_partial_app_config_empty_serialize() {
    insta::assert_debug_snapshot!(PartialAppConfig::empty());
}

#[test]
fn test_partial_app_config_default_values() {
    insta::assert_debug_snapshot!(PartialAppConfig::default_values(&()));
}

#[test]
fn test_partial_app_config_default() {
    insta::assert_debug_snapshot!(PartialAppConfig::default());
}

#[test]
fn test_app_config_fields() {
    insta::assert_debug_snapshot!(AppConfig::fields());
}

#[test]
fn test_ensure_no_missing_assignments() {
    // Some fields cannot be assigned via CLI.
    let skip_fields = ["extends"];

    for field in AppConfig::fields() {
        if skip_fields.contains(&field.as_str()) {
            continue;
        }

        let mut p = PartialAppConfig::default();
        let kv = KvAssignment::try_from_cli(&field, "foo").unwrap();
        if let Err(error) = p.assign(kv) {
            let Ok(error) = error.downcast::<KvAssignmentError>() else {
                continue;
            };

            match &error.error {
                KvAssignmentErrorKind::KvParse { .. }
                | KvAssignmentErrorKind::UnknownKey { .. }
                | KvAssignmentErrorKind::UnknownIndex { .. } => {}

                KvAssignmentErrorKind::Json(_)
                | KvAssignmentErrorKind::Parse { .. }
                | KvAssignmentErrorKind::Type { .. }
                | KvAssignmentErrorKind::ParseBool(_)
                | KvAssignmentErrorKind::ParseInt(_)
                | KvAssignmentErrorKind::ParseFloat(_) => continue,
            }

            panic!("unexpected error for field '{field}': {error:?}");
        }
    }
}

#[test]
fn test_partial_app_config_assign() {
    let mut p = PartialAppConfig::default();

    let kv = KvAssignment::try_from_cli("inherit", "true").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.inherit, Some(true));

    let kv = KvAssignment::try_from_cli("config_load_paths", "foo,bar").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.config_load_paths, Some(vec!["foo".into(), "bar".into()]));

    let kv = KvAssignment::try_from_cli("assistant.name", "foo").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.assistant.name.as_deref(), Some("foo"));

    let kv = KvAssignment::try_from_cli("assistant:", r#"{"name":"bar","system_prompt":"baz"}"#)
        .unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.assistant.name.as_deref(), Some("bar"));
    assert_eq!(p.assistant.system_prompt.as_deref(), Some("baz"));

    let kv = KvAssignment::try_from_cli("config_load_paths:", "[true]").unwrap();
    let error = p
        .assign(kv)
        .unwrap_err()
        .downcast::<KvAssignmentError>()
        .unwrap()
        .error;

    assert_matches!(
        error,
        KvAssignmentErrorKind::Type { need, .. } if need == ["string"]
    );
}

#[test]
fn resolve_model_aliases_resolves_assistant_model() {
    use crate::model::id::{ModelIdConfig, PartialModelIdOrAliasConfig, ProviderId};

    let aliases = IndexMap::from([("haiku".to_owned(), ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: "claude-haiku-4-5".parse().unwrap(),
    })]);

    let mut partial = PartialAppConfig::empty();
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Alias("haiku".into());

    partial.resolve_model_aliases(&aliases);

    match &partial.assistant.model.id {
        PartialModelIdOrAliasConfig::Id(id) => {
            assert_eq!(id.provider, Some(ProviderId::Anthropic));
            assert_eq!(id.name.as_ref().unwrap().to_string(), "claude-haiku-4-5");
        }
        PartialModelIdOrAliasConfig::Alias(a) => panic!("expected Id, got Alias({a})"),
    }
}

#[test]
fn resolve_model_aliases_leaves_direct_id_unchanged() {
    use crate::model::id::{PartialModelIdConfig, PartialModelIdOrAliasConfig, ProviderId};

    let aliases = IndexMap::new();
    let mut partial = PartialAppConfig::empty();
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
        provider: Some(ProviderId::Google),
        name: "gemini-pro".parse().ok(),
    });

    partial.resolve_model_aliases(&aliases);

    match &partial.assistant.model.id {
        PartialModelIdOrAliasConfig::Id(id) => {
            assert_eq!(id.provider, Some(ProviderId::Google));
        }
        PartialModelIdOrAliasConfig::Alias(a) => panic!("expected Id, got Alias({a})"),
    }
}

#[test]
fn build_resolves_aliases() {
    use crate::{
        conversation::tool::RunMode,
        model::id::{ModelIdConfig, ModelIdOrAliasConfig, PartialModelIdOrAliasConfig, ProviderId},
        util::build,
    };

    let mut partial = PartialAppConfig::default();
    partial.conversation.tools.defaults.run = Some(RunMode::Ask);
    partial.providers.llm.aliases.insert(
        "mymodel".to_owned(),
        ModelIdConfig {
            provider: ProviderId::Anthropic,
            name: "claude-haiku-4-5".parse().unwrap(),
        }
        .to_partial(),
    );
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Alias("mymodel".into());

    let config = build(partial).expect("valid config");

    assert!(
        matches!(&config.assistant.model.id, ModelIdOrAliasConfig::Id(_)),
        "expected Id variant after build, got: {:?}",
        config.assistant.model.id
    );

    let resolved = config.assistant.model.id.resolved();
    assert_eq!(resolved.provider, ProviderId::Anthropic);
    assert_eq!(resolved.name.to_string(), "claude-haiku-4-5");
}
