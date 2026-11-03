//! Tests for `TurnSummaryConfig` and `TurnSummaryGenerateConfig`.

use indexmap::IndexMap;
use indoc::indoc;
use schematic::PartialConfig as _;

use super::*;
use crate::{
    AppConfig, PartialAppConfig,
    model::id::{ModelIdOrAliasConfig, PartialModelIdOrAliasConfig},
    util,
};

fn resolve_with(toml: &str) -> AppConfig {
    let mut partial = PartialAppConfig::new_test();
    partial.merge(&(), toml::from_str(toml).unwrap()).unwrap();
    util::build(partial).unwrap()
}

#[test]
fn an_unconfigured_section_takes_its_defaults() {
    let mut partial = PartialAppConfig::new_test();
    partial
        .unset("conversation.turn_summary.generate.auto")
        .unwrap();

    let config = util::build(partial).unwrap();

    assert_eq!(
        config.conversation.turn_summary.generate,
        TurnSummaryGenerateConfig {
            auto: true,
            batch_size: 5,
            model: None,
        }
    );
}

#[test]
fn a_config_file_sets_every_key() {
    let config = resolve_with(indoc! {r#"
        [conversation.turn_summary.generate]
        auto = false
        batch_size = 2

        [conversation.turn_summary.generate.model]
        id = "ollama/llama3.2:3b"
    "#});

    let generate = config.conversation.turn_summary.generate;
    assert!(!generate.auto);
    assert_eq!(generate.batch_size, 2);
    assert_eq!(
        generate.model.map(|model| model.id),
        Some(
            "ollama/llama3.2:3b"
                .parse::<ModelIdOrAliasConfig>()
                .unwrap()
        )
    );
}

#[test]
fn every_key_is_assignable_by_path() {
    let mut partial = PartialAppConfig::empty();

    for (key, value) in [
        ("conversation.turn_summary.generate.auto", "false"),
        ("conversation.turn_summary.generate.batch_size", "10"),
        (
            "conversation.turn_summary.generate.model.id",
            "ollama/llama3.2:3b",
        ),
    ] {
        partial
            .assign(KvAssignment::try_from_cli(key, value).unwrap())
            .unwrap();
    }

    let generate = partial.conversation.turn_summary.generate;
    assert_eq!(generate.auto, Some(false));
    assert_eq!(generate.batch_size, Some(10));
    assert_eq!(
        generate.model.map(|model| model.id),
        Some(PartialModelIdOrAliasConfig::from("ollama/llama3.2:3b"))
    );
}

#[test]
fn the_section_is_assignable_as_one_json_object() {
    let mut partial = PartialAppConfig::empty();

    partial
        .assign(
            KvAssignment::try_from_cli(
                "conversation.turn_summary:",
                r#"{"generate": {"auto": false, "batch_size": 3}}"#,
            )
            .unwrap(),
        )
        .unwrap();

    let generate = partial.conversation.turn_summary.generate;
    assert_eq!(generate.auto, Some(false));
    assert_eq!(generate.batch_size, Some(3));
}

#[test]
fn a_batch_size_that_is_not_a_count_is_rejected() {
    for value in ["-1", "2.5", "many"] {
        let mut partial = PartialAppConfig::empty();
        let kv = KvAssignment::try_from_cli("conversation.turn_summary.generate.batch_size", value)
            .unwrap();

        assert!(partial.assign(kv).is_err(), "accepted {value:?}");
    }
}

#[test]
fn a_delta_records_only_the_key_that_changed() {
    let prev = resolve_with("").to_partial();
    let next = resolve_with(indoc! {"
        [conversation.turn_summary.generate]
        batch_size = 2
    "})
    .to_partial();

    let delta = prev.delta(next);

    assert_eq!(delta.conversation.turn_summary, PartialTurnSummaryConfig {
        generate: PartialTurnSummaryGenerateConfig {
            auto: None,
            batch_size: Some(2),
            model: None,
        },
    });
}

#[test]
fn clearing_the_model_is_recorded_as_an_unset() {
    let prev = resolve_with(indoc! {r#"
        [conversation.turn_summary.generate.model]
        id = "ollama/llama3.2:3b"
    "#})
    .to_partial();
    let next = resolve_with("");

    let mut unsets = Vec::new();
    let delta = prev.delta_with_unsets(next.to_partial(), "", &mut unsets);

    assert_eq!(unsets, ["conversation.turn_summary.generate.model"]);

    let mut folded = prev;
    folded
        .unset("conversation.turn_summary.generate.model")
        .unwrap();
    folded.merge(&(), delta).unwrap();

    assert_eq!(
        util::build(folded).unwrap().conversation.turn_summary,
        next.conversation.turn_summary
    );
}

#[test]
fn a_resolved_section_round_trips_through_its_partial() {
    let config = resolve_with(indoc! {r#"
        [conversation.turn_summary.generate]
        auto = false
        batch_size = 2

        [conversation.turn_summary.generate.model]
        id = "ollama/llama3.2:3b"
    "#});

    let rebuilt = util::build(config.to_partial()).unwrap();

    assert_eq!(
        rebuilt.conversation.turn_summary,
        config.conversation.turn_summary
    );
}

#[test]
fn a_model_alias_resolves_when_the_config_is_built() {
    let config = resolve_with(indoc! {r#"
        [providers.llm.aliases]
        fast = "ollama/llama3.2:3b"

        [conversation.turn_summary.generate.model]
        id = "fast"
    "#});

    assert_eq!(
        config
            .conversation
            .turn_summary
            .generate
            .model
            .map(|model| model.id),
        Some(
            "ollama/llama3.2:3b"
                .parse::<ModelIdOrAliasConfig>()
                .unwrap()
        )
    );
}

#[test]
fn a_model_alias_resolves_before_a_partial_is_stored() {
    let aliases = IndexMap::from([(
        "fast".to_owned(),
        "ollama/llama3.2:3b"
            .parse::<ModelIdOrAliasConfig>()
            .unwrap(),
    )]);
    let mut partial = PartialAppConfig::empty();
    partial
        .assign(
            KvAssignment::try_from_cli("conversation.turn_summary.generate.model.id", "fast")
                .unwrap(),
        )
        .unwrap();

    partial.resolve_model_aliases(&aliases);

    assert_eq!(
        partial
            .conversation
            .turn_summary
            .generate
            .model
            .map(|model| model.id),
        Some(PartialModelIdOrAliasConfig::from("ollama/llama3.2:3b"))
    );
}
