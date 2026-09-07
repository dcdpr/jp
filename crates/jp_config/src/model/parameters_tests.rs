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

/// A provider parameter is cleared by its own name, with no wrapper in the
/// path.
#[test]
fn assign_clears_a_collected_parameter() {
    let mut p = PartialParametersConfig::default();
    p.assign(KvAssignment::try_from_cli("seed", "42").unwrap())
        .unwrap();
    p.assign(KvAssignment::unset("seed")).unwrap();

    assert!(
        p.other.as_ref().is_none_or(IndexMap::is_empty),
        "expected the parameter gone, got: {:?}",
        p.other
    );
}

/// The collector is flattened, so a provider parameter is written and read back
/// under its own name with no wrapper key in between.
#[test]
fn other_is_flattened_on_the_wire() {
    let mut p = PartialParametersConfig::default();
    p.assign(KvAssignment::try_from_cli("seed", "42").unwrap())
        .unwrap();

    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(
        json.get("seed"),
        Some(&json!("42")),
        "the parameter sits in the block: {json}"
    );
    assert!(
        json.get("other").is_none(),
        "no wrapper key reaches the wire: {json}"
    );

    let back: PartialParametersConfig = serde_json::from_value(json).unwrap();
    assert_eq!(back.other.as_ref().map(IndexMap::len), Some(1));
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

/// A stored config or user file written before `other` was flattened nested its
/// parameters under it, and those still land as parameters.
#[test]
fn deserialize_hoists_a_legacy_other_table() {
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
fn deserialize_collects_nothing_when_every_key_is_known() {
    let p = parameters_from_toml("top_k = 40");

    assert_eq!(p.top_k, Some(40));
    assert!(
        p.other.as_ref().is_none_or(IndexMap::is_empty),
        "expected no collected parameters, got: {:?}",
        p.other
    );
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
fn deserialize_hoists_an_empty_legacy_other_table() {
    let p = parameters_from_toml("other = {}");

    assert!(
        p.other.as_ref().is_none_or(IndexMap::is_empty),
        "an empty legacy table leaves no parameter behind, got: {:?}",
        p.other
    );
}

/// A provider parameter that is itself called `other` is written like any
/// other, now that the name is not a wrapper.
#[test]
fn a_parameter_named_other_is_not_a_wrapper() {
    let mut p = PartialParametersConfig::default();
    p.assign(KvAssignment::try_from_cli("other", "5").unwrap())
        .unwrap();

    assert_eq!(p.other.as_ref().unwrap()["other"], JsonValue(json!("5")));
}

#[test]
fn stop_words_append_across_layers() {
    use schematic::PartialConfig as _;

    let mut base = PartialParametersConfig {
        stop_words: Some(vec!["STOP".to_owned()].into()),
        ..Default::default()
    };
    let overlay = PartialParametersConfig {
        stop_words: Some(vec!["HALT".to_owned()].into()),
        ..Default::default()
    };

    base.merge(&(), overlay).unwrap();

    assert_eq!(
        base.stop_words,
        Some(vec!["STOP".to_owned(), "HALT".to_owned()].into())
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
