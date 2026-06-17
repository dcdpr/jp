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
    use crate::model::id::{
        ModelIdConfig, ModelIdOrAliasConfig, PartialModelIdOrAliasConfig, ProviderId,
    };

    let aliases = IndexMap::from([(
        "haiku".to_owned(),
        ModelIdOrAliasConfig::Id(ModelIdConfig {
            provider: ProviderId::Anthropic,
            name: "claude-haiku-4-5".parse().unwrap(),
        }),
    )]);

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
        .to_partial()
        .into(),
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

#[test]
fn build_resolves_chained_aliases() {
    use crate::{
        conversation::tool::RunMode,
        model::id::{ModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
        util::build,
    };

    let mut partial = PartialAppConfig::default();
    partial.conversation.tools.defaults.run = Some(RunMode::Ask);
    partial.providers.llm.aliases.insert(
        "opus".to_owned(),
        ModelIdConfig {
            provider: ProviderId::Anthropic,
            name: "claude-opus-4".parse().unwrap(),
        }
        .to_partial()
        .into(),
    );
    partial.providers.llm.aliases.insert(
        "coder".to_owned(),
        PartialModelIdOrAliasConfig::Alias("opus".into()),
    );
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Alias("coder".into());

    let config = build(partial).expect("valid config");

    let resolved = config.assistant.model.id.resolved();
    assert_eq!(resolved.provider, ProviderId::Anthropic);
    assert_eq!(resolved.name.to_string(), "claude-opus-4");
}

#[test]
fn compaction_rule_unset_bounds_resolve_to_field_defaults() {
    use crate::{
        conversation::{
            compaction::{PartialCompactionRuleConfig, RuleBound, ToolCallsMode},
            tool::RunMode,
        },
        model::id::{PartialModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
        types::vec::MergeableVec,
        util::build,
    };

    let mut partial = PartialAppConfig::default();
    partial.conversation.tools.defaults.run = Some(RunMode::Ask);
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
        provider: Some(ProviderId::Anthropic),
        name: "claude-opus-4".parse().ok(),
    });

    // A rule that sets only a tool-call policy, leaving keep_first/keep_last
    // unset — exactly what `jp c compact -t sreq` produces.
    partial.conversation.compaction.rules = MergeableVec::Vec(vec![PartialCompactionRuleConfig {
        tool_calls: Some(ToolCallsMode::StripRequests),
        ..Default::default()
    }]);

    let config = build(partial).expect("valid config");

    let rule = &config.conversation.compaction.rules[0];
    assert_eq!(rule.keep_first, RuleBound::Turns(1));
    assert_eq!(rule.keep_last, RuleBound::Turns(3));
}

#[test]
fn empty_config_preserves_default_compaction_rule() {
    use crate::{
        conversation::{
            compaction::{ReasoningMode, RuleBound, ToolCallsMode},
            tool::RunMode,
        },
        model::id::{PartialModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
        util::build,
    };

    // A config that sets only the required fields and leaves compaction
    // untouched must still resolve to the built-in default rule, so bare
    // `jp conversation compact` / `jp query --compact` are not no-ops.
    let mut partial = PartialAppConfig::default();
    partial.conversation.tools.defaults.run = Some(RunMode::Ask);
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
        provider: Some(ProviderId::Anthropic),
        name: "claude-opus-4".parse().ok(),
    });

    let config = build(partial).expect("valid config");

    let rules = &config.conversation.compaction.rules;
    assert_eq!(rules.len(), 1, "default rule must survive an empty config");
    assert_eq!(rules[0].reasoning, Some(ReasoningMode::Strip));
    assert_eq!(rules[0].tool_calls, Some(ToolCallsMode::Strip));
    assert_eq!(rules[0].keep_first, RuleBound::Turns(1));
    assert_eq!(rules[0].keep_last, RuleBound::Turns(3));
}

#[test]
fn build_rejects_alias_cycle() {
    use crate::{
        conversation::tool::RunMode,
        model::id::{PartialModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
        util::build,
    };

    let mut partial = PartialAppConfig::default();
    partial.conversation.tools.defaults.run = Some(RunMode::Ask);
    // A valid assistant model so `from_partial` succeeds; the cycle lives in
    // aliases that no field references, exercising the up-front validation.
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
        provider: Some(ProviderId::Anthropic),
        name: "claude-opus-4".parse().ok(),
    });
    partial.providers.llm.aliases.insert(
        "a".to_owned(),
        PartialModelIdOrAliasConfig::Alias("b".into()),
    );
    partial.providers.llm.aliases.insert(
        "b".to_owned(),
        PartialModelIdOrAliasConfig::Alias("a".into()),
    );

    let err = build(partial).unwrap_err();
    assert!(err.to_string().contains("cycle"), "got: {err}");
}
