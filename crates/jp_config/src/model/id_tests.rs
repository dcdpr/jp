use serde_json::{Value, json};

use super::*;

#[test]
fn test_model_id_config_deserialize() {
    struct TestCase {
        data: Value,
        expected: PartialModelIdConfig,
    }

    let cases = vec![
        TestCase {
            data: json!({
                "provider": "ollama",
                "name": "bar",
            }),
            expected: PartialModelIdConfig {
                provider: Some(ProviderId::Ollama),
                name: "bar".parse().ok(),
            },
        },
        TestCase {
            data: json!("llamacpp/bar"),
            expected: PartialModelIdConfig {
                provider: Some(ProviderId::Llamacpp),
                name: "bar".parse().ok(),
            },
        },
    ];

    for TestCase { data, expected } in cases {
        let result = serde_json::from_value::<PartialModelIdConfig>(data);
        assert_eq!(result.unwrap(), expected);
    }
}

#[test]
fn resolve_partial_alias_from_map() {
    let aliases = IndexMap::from([("haiku".to_owned(), ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: "claude-haiku-4-5".parse().unwrap(),
    })]);

    let partial = PartialModelIdOrAliasConfig::Alias("haiku".into());
    let resolved = partial.resolve(&aliases).unwrap();
    assert_eq!(resolved.provider, ProviderId::Anthropic);
    assert_eq!(resolved.name.to_string(), "claude-haiku-4-5");
}

#[test]
fn resolve_partial_alias_falls_back_to_parse() {
    let aliases = IndexMap::new();

    let partial = PartialModelIdOrAliasConfig::Alias("anthropic/claude-haiku-4-5".into());
    let resolved = partial.resolve(&aliases).unwrap();
    assert_eq!(resolved.provider, ProviderId::Anthropic);
    assert_eq!(resolved.name.to_string(), "claude-haiku-4-5");
}

#[test]
fn resolve_partial_direct_id() {
    let aliases = IndexMap::new();

    let partial = PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
        provider: Some(ProviderId::Google),
        name: "gemini-pro".parse().ok(),
    });
    let resolved = partial.resolve(&aliases).unwrap();
    assert_eq!(resolved.provider, ProviderId::Google);
    assert_eq!(resolved.name.to_string(), "gemini-pro");
}

#[test]
fn resolve_partial_direct_id_missing_provider() {
    let aliases = IndexMap::new();

    let partial = PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
        provider: None,
        name: "some-model".parse().ok(),
    });
    assert!(partial.resolve(&aliases).is_err());
}

#[test]
fn resolve_partial_direct_id_missing_name() {
    let aliases = IndexMap::new();

    let partial = PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
        provider: Some(ProviderId::Anthropic),
        name: None,
    });
    assert!(partial.resolve(&aliases).is_err());
}

#[test]
fn resolved_returns_id() {
    let config = ModelIdOrAliasConfig::Id(ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: "claude-haiku-4-5".parse().unwrap(),
    });
    let id = config.resolved();
    assert_eq!(id.provider, ProviderId::Anthropic);
    assert_eq!(id.name.to_string(), "claude-haiku-4-5");
}

#[test]
#[should_panic(expected = "unresolved model alias 'haiku'")]
fn resolved_panics_on_alias() {
    let config = ModelIdOrAliasConfig::Alias("haiku".into());
    let _id = config.resolved();
}

#[test]
fn resolve_in_place_converts_alias() {
    let aliases = IndexMap::from([("haiku".to_owned(), ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: "claude-haiku-4-5".parse().unwrap(),
    })]);

    let mut config = ModelIdOrAliasConfig::Alias("haiku".into());
    config.resolve_in_place(&aliases).unwrap();

    let id = config.resolved();
    assert_eq!(id.provider, ProviderId::Anthropic);
    assert_eq!(id.name.to_string(), "claude-haiku-4-5");
}

#[test]
fn resolve_in_place_noop_on_id() {
    let aliases = IndexMap::new();
    let original = ModelIdConfig {
        provider: ProviderId::Google,
        name: "gemini-pro".parse().unwrap(),
    };

    let mut config = ModelIdOrAliasConfig::Id(original.clone());
    config.resolve_in_place(&aliases).unwrap();
    assert_eq!(config.resolved(), &original);
}

#[test]
fn resolve_in_place_error_on_unknown_alias() {
    let aliases = IndexMap::new();
    let mut config = ModelIdOrAliasConfig::Alias("nonexistent".into());
    assert!(config.resolve_in_place(&aliases).is_err());
}

#[test]
fn partial_resolve_in_place_converts_alias() {
    let aliases = IndexMap::from([("haiku".to_owned(), ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: "claude-haiku-4-5".parse().unwrap(),
    })]);

    let mut partial = PartialModelIdOrAliasConfig::Alias("haiku".into());
    partial.resolve_in_place(&aliases);

    match partial {
        PartialModelIdOrAliasConfig::Id(id) => {
            assert_eq!(id.provider, Some(ProviderId::Anthropic));
            assert_eq!(id.name.as_ref().unwrap().to_string(), "claude-haiku-4-5");
        }
        PartialModelIdOrAliasConfig::Alias(_) => panic!("expected Id variant"),
    }
}

#[test]
fn partial_resolve_in_place_noop_on_id() {
    let aliases = IndexMap::new();
    let mut partial = PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
        provider: Some(ProviderId::Google),
        name: "gemini-pro".parse().ok(),
    });

    partial.resolve_in_place(&aliases);

    match partial {
        PartialModelIdOrAliasConfig::Id(id) => {
            assert_eq!(id.provider, Some(ProviderId::Google));
        }
        PartialModelIdOrAliasConfig::Alias(_) => panic!("expected Id variant"),
    }
}

#[test]
fn partial_resolve_in_place_unknown_alias_left_as_is() {
    let aliases = IndexMap::new();
    let mut partial = PartialModelIdOrAliasConfig::Alias("nonexistent".into());
    partial.resolve_in_place(&aliases);

    // Unresolvable alias is left unchanged.
    assert!(matches!(partial, PartialModelIdOrAliasConfig::Alias(a) if a == "nonexistent"));
}
