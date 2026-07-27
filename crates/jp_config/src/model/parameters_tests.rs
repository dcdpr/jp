use serde_json::json;

use super::*;
use crate::types::json_value::JsonValue;

#[test]
fn assign_unknown_key_delegates_to_other() {
    let mut p = PartialParametersConfig::default();
    let kv = KvAssignment::try_from_cli("seed", "42").unwrap();
    p.assign(kv).unwrap();

    let other = p.other.as_ref().unwrap();
    assert_eq!(other["seed"], JsonValue(json!("42")));
}

#[test]
fn assign_unknown_nested_key_delegates_to_other() {
    let mut p = PartialParametersConfig::default();
    let kv = KvAssignment::try_from_cli("custom.depth", "3").unwrap();
    p.assign(kv).unwrap();

    let other = p.other.as_ref().unwrap();
    assert_eq!(other["custom"], JsonValue(json!({"depth": "3"})));
}

#[test]
fn known_keys_match_the_schema() {
    use schematic::{SchemaBuilder, SchemaType, Schematic as _};

    // `deserialize_collecting_other` splits the parameter block using this
    // list. A field added to the struct but missed here would be rerouted into
    // `other` and forwarded to the provider as a raw parameter instead.
    let schema = ParametersConfig::build_schema(SchemaBuilder::default());
    let SchemaType::Struct(struct_type) = &schema.ty else {
        panic!("expected a struct schema");
    };

    let mut fields: Vec<&str> = struct_type.fields.keys().map(String::as_str).collect();
    let mut known = KNOWN_KEYS.to_vec();
    fields.sort_unstable();
    known.sort_unstable();

    assert_eq!(fields, known);
}

/// Deserialize a `[parameters]` block through the production path: the
/// collector is wired up on `ModelConfig::parameters`, not on the parameter
/// config itself.
fn parameters_from_toml(block: &str) -> PartialParametersConfig {
    let toml = format!("[parameters]\n{block}");
    toml::from_str::<crate::model::PartialModelConfig>(&toml)
        .unwrap()
        .parameters
}

#[test]
fn deserialize_collects_unknown_keys_into_other() {
    // A provider parameter JP doesn't model, written directly in the parameter
    // block. Discarding it silently drops user intent.
    let p = parameters_from_toml(indoc::indoc!(
        r#"
            max_tokens = 100
            presence_penalty = 0.5
            logit_bias = { "50256" = -100 }
        "#
    ));

    assert_eq!(p.max_tokens, Some(100));

    let other = p.other.as_ref().unwrap();
    assert_eq!(other["presence_penalty"], JsonValue(json!(0.5)));
    assert_eq!(other["logit_bias"], JsonValue(json!({"50256": -100})));
    assert_eq!(other.len(), 2, "known keys must not leak into `other`");
}

#[test]
fn deserialize_accepts_an_explicit_other_table() {
    // The nested form is what every stored conversation config and existing
    // user file writes, so it has to keep working.
    let p = parameters_from_toml(indoc::indoc!(
        r"
            temperature = 0.7

            [parameters.other]
            presence_penalty = 0.5
        "
    ));

    assert_eq!(p.temperature, Some(0.7));
    assert_eq!(
        p.other.as_ref().unwrap()["presence_penalty"],
        JsonValue(json!(0.5))
    );
}

#[test]
fn deserialize_prefers_the_explicit_other_entry_on_collision() {
    let p = parameters_from_toml(indoc::indoc!(
        r"
            presence_penalty = 0.1

            [parameters.other]
            presence_penalty = 0.9
        "
    ));

    assert_eq!(
        p.other.as_ref().unwrap()["presence_penalty"],
        JsonValue(json!(0.9))
    );
}

#[test]
fn deserialize_leaves_other_unset_when_every_key_is_known() {
    let p = parameters_from_toml("top_k = 40");

    assert_eq!(p.top_k, Some(40));
    assert_eq!(p.other, None);
}

#[test]
fn deserialize_preserves_the_untagged_reasoning_field() {
    // `reasoning` deserializes from a bare string or a table, and the collector
    // routes every known field through a `serde_json::Value` intermediate, so
    // the untagged forms have to survive that trip.
    let p = parameters_from_toml(r#"reasoning = "off""#);
    assert_eq!(p.reasoning, Some(PartialReasoningConfig::Off));

    let p = parameters_from_toml(indoc::indoc!(
        r#"
            [parameters.reasoning]
            effort = "low"
        "#
    ));
    assert_eq!(
        p.reasoning,
        Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::Low),
                exclude: None,
            }
        ))
    );
}

#[test]
fn deserialize_keeps_an_explicit_empty_other() {
    // Serialization emits `other = {}` for a present-but-empty map, so dropping
    // it here would make a stored config lossy on round-trip.
    let p = parameters_from_toml("other = {}");

    assert_eq!(p.other, Some(IndexMap::new()));
}

#[test]
fn stop_words_append_across_layers() {
    use schematic::PartialConfig as _;

    let mut base = PartialParametersConfig {
        stop_words: Some(vec!["STOP".to_owned()]),
        ..Default::default()
    };
    let overlay = PartialParametersConfig {
        stop_words: Some(vec!["HALT".to_owned()]),
        ..Default::default()
    };

    base.merge(&(), overlay).unwrap();

    assert_eq!(
        base.stop_words,
        Some(vec!["STOP".to_owned(), "HALT".to_owned()])
    );
}

#[test]
fn assign_known_keys_not_routed_to_other() {
    let mut p = PartialParametersConfig::default();

    let kv = KvAssignment::try_from_cli("temperature", "0.7").unwrap();
    p.assign(kv).unwrap();
    assert!(p.other.is_none());

    let kv = KvAssignment::try_from_cli("max_tokens", "1024").unwrap();
    p.assign(kv).unwrap();
    assert!(p.other.is_none());
}
