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

/// Setting one field in the inquiry request block leaves its siblings
/// inheriting from the top-level assistant, rather than resolving to `0`.
#[test]
fn inquiry_inherits_unset_request_fields_from_the_assistant() {
    use crate::assistant::request::{CachePolicy, MaxResponseBytes, PartialRequestConfig};

    let mut partial = PartialAppConfig::new_test();
    partial.assistant.request.max_response_bytes = Some(MaxResponseBytes::Bytes(500_000));
    partial.assistant.request.max_retries = Some(7);

    // Only a sibling field is set in the inquiry block.
    partial.conversation.inquiry.assistant.request = PartialRequestConfig {
        cache: Some(CachePolicy::Off),
        ..PartialRequestConfig::default()
    };

    let config = AppConfig::from_partial_with_defaults(partial).expect("valid config");
    let request = config.conversation.inquiry.assistant.request;

    assert_eq!(request.cache, CachePolicy::Off, "the set field survives");
    assert_eq!(
        request.max_response_bytes,
        MaxResponseBytes::Bytes(500_000),
        "an unset sibling inherits the assistant value"
    );
    assert_eq!(
        request.max_retries, 7,
        "inheritance covers every field in the block"
    );

    // The parent keeps its own value; the fill is one-directional.
    assert_eq!(
        config.assistant.request.max_response_bytes,
        MaxResponseBytes::Bytes(500_000)
    );
}

/// The mergeable collections inherit their entries, not just their metadata.
///
/// `MergeableVec::fill_from` keeps its own items by design, so filling the
/// inquiry block from the assistant with it would leave the inquiry's lists
/// empty and let the schematic defaults claim them.
/// `build_sections` reads the inquiry config, so that silently drops a user's
/// instructions and sections from every inquiry.
#[test]
fn inquiry_inherits_assistant_collections() {
    use crate::assistant::{
        instructions::PartialInstructionsConfig, sections::PartialSectionConfig,
    };

    let mut partial = PartialAppConfig::new_test();
    partial.assistant.instructions = vec![PartialInstructionsConfig {
        title: Some("House rules".to_owned()),
        items: Some(vec!["Be concise".to_owned()]),
        ..Default::default()
    }]
    .into();
    partial.assistant.system_prompt_sections = vec![PartialSectionConfig {
        content: Some("Context".to_owned()),
        ..Default::default()
    }]
    .into();

    let config = AppConfig::from_partial_with_defaults(partial).expect("valid config");
    let inquiry = &config.conversation.inquiry.assistant;

    assert_eq!(
        inquiry.instructions.len(),
        1,
        "the inquiry inherits the assistant's instructions rather than the type defaults"
    );
    assert_eq!(
        inquiry.instructions[0].title.as_deref(),
        Some("House rules"),
        "and inherits the user's entry, not a default one"
    );
    assert_eq!(
        inquiry.system_prompt_sections.len(),
        1,
        "the inquiry inherits the assistant's prompt sections"
    );
}

/// An inquiry that declares its own collections keeps them.
#[test]
fn an_explicit_inquiry_collection_wins_over_the_assistant() {
    use crate::assistant::instructions::PartialInstructionsConfig;

    let mut partial = PartialAppConfig::new_test();
    partial.assistant.instructions = vec![PartialInstructionsConfig {
        title: Some("Assistant rules".to_owned()),
        ..Default::default()
    }]
    .into();
    partial.conversation.inquiry.assistant.instructions = vec![PartialInstructionsConfig {
        title: Some("Inquiry rules".to_owned()),
        ..Default::default()
    }]
    .into();

    let config = AppConfig::from_partial_with_defaults(partial).expect("valid config");
    let inquiry = &config.conversation.inquiry.assistant;

    assert_eq!(inquiry.instructions.len(), 1);
    assert_eq!(
        inquiry.instructions[0].title.as_deref(),
        Some("Inquiry rules"),
        "a declared list is not merged with the parent's"
    );
    assert_eq!(
        config.assistant.instructions[0].title.as_deref(),
        Some("Assistant rules"),
        "and the parent keeps its own"
    );
}

/// An inquiry value pinned to the same number as the assistant's does not
/// survive a round-trip, and follows a later assistant-only change.
///
/// This records a known limitation rather than desired behavior.
/// `to_partial` only has equality to work from, so it cannot tell a deliberate
/// same-as-parent value from an inherited one.
/// Dropping it is the lesser evil: recording every inherited value instead
/// would stop `assistant` changes from ever reaching the inquiry, which is the
/// far more common path (see
/// `inquiry_inheritance_survives_a_partial_round_trip`).
///
/// Fixing it needs per-field presence to survive resolution.
#[test]
fn a_same_valued_inquiry_pin_is_lost_on_a_round_trip() {
    use crate::assistant::request::MaxResponseBytes;

    let mut partial = PartialAppConfig::new_test();
    partial.assistant.request.max_response_bytes = Some(MaxResponseBytes::Bytes(4096));
    partial
        .conversation
        .inquiry
        .assistant
        .request
        .max_response_bytes = Some(MaxResponseBytes::Bytes(4096));

    let config = AppConfig::from_partial_with_defaults(partial).expect("valid config");

    // Round-trip, then raise only the assistant's ceiling.
    let mut round_tripped = config.to_partial();
    round_tripped.assistant.request.max_response_bytes = Some(MaxResponseBytes::Bytes(999_999));

    let config = AppConfig::from_partial_with_defaults(round_tripped).expect("valid config");

    assert_eq!(
        config
            .conversation
            .inquiry
            .assistant
            .request
            .max_response_bytes,
        MaxResponseBytes::Bytes(999_999),
        "the pin is indistinguishable from inheritance and follows the assistant"
    );
}

/// An explicit inquiry value wins over the inherited assistant value.
#[test]
fn inquiry_request_override_wins_over_the_assistant() {
    use crate::assistant::request::MaxResponseBytes;

    let mut partial = PartialAppConfig::new_test();
    partial.assistant.request.max_response_bytes = Some(MaxResponseBytes::Bytes(500_000));
    partial
        .conversation
        .inquiry
        .assistant
        .request
        .max_response_bytes = Some(MaxResponseBytes::Bytes(4096));

    let config = AppConfig::from_partial_with_defaults(partial).expect("valid config");

    assert_eq!(
        config
            .conversation
            .inquiry
            .assistant
            .request
            .max_response_bytes,
        MaxResponseBytes::Bytes(4096)
    );
    assert_eq!(
        config.assistant.request.max_response_bytes,
        MaxResponseBytes::Bytes(500_000)
    );
}

/// Disabling the ceiling for inquiries alone survives inheritance.
#[test]
fn inquiry_can_disable_a_ceiling_the_assistant_sets() {
    use crate::assistant::request::MaxResponseBytes;

    let mut partial = PartialAppConfig::new_test();
    partial.assistant.request.max_response_bytes = Some(MaxResponseBytes::Bytes(500_000));
    partial
        .conversation
        .inquiry
        .assistant
        .request
        .max_response_bytes = Some(MaxResponseBytes::Disabled);

    let config = AppConfig::from_partial_with_defaults(partial).expect("valid config");

    assert_eq!(
        config
            .conversation
            .inquiry
            .assistant
            .request
            .max_response_bytes,
        MaxResponseBytes::Disabled,
        "an explicit disable must not be overwritten by the inherited value"
    );
}

/// A round-trip through `to_partial` must not freeze an inherited inquiry
/// value.
///
/// `to_partial` is how a resolved config becomes a layer again (a stored
/// conversation config, a `--cfg` baseline).
/// If it recorded the inherited inquiry values verbatim, a later layer that
/// changes `assistant` would no longer reach the inquiry, which is the
/// inheritance silently stopping.
#[test]
fn inquiry_inheritance_survives_a_partial_round_trip() {
    use crate::model::id::{ModelIdConfig, PartialModelIdOrAliasConfig, ProviderId};

    let mut partial = PartialAppConfig::new_test();
    partial.assistant.model.id = ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: "first-model".parse().unwrap(),
    }
    .to_partial()
    .into();

    let config = AppConfig::from_partial_with_defaults(partial).expect("valid config");
    assert_eq!(
        config
            .conversation
            .inquiry
            .assistant
            .model
            .id
            .resolved()
            .name
            .as_ref(),
        "first-model",
        "the inquiry inherits the assistant model"
    );

    // Round-trip, then change only the assistant on a later layer.
    let mut round_tripped = config.to_partial();
    round_tripped.assistant.model.id = PartialModelIdOrAliasConfig::Id(
        ModelIdConfig {
            provider: ProviderId::Anthropic,
            name: "second-model".parse().unwrap(),
        }
        .to_partial(),
    );

    let config = AppConfig::from_partial_with_defaults(round_tripped).expect("valid config");

    assert_eq!(
        config
            .conversation
            .inquiry
            .assistant
            .model
            .id
            .resolved()
            .name
            .as_ref(),
        "second-model",
        "the inquiry must follow the new assistant model, not the round-tripped copy"
    );
}

/// A changed inquiry model id is recorded whole, so a later layer naming the
/// assistant model by alias cannot strand it.
///
/// A resolved config becomes a layer again through `to_partial` (a stored
/// conversation config, a `--cfg` baseline), which records the inquiry model as
/// a diff against the assistant's.
/// A diff keeping only the differing field would leave the inquiry holding a
/// name and no provider, and an assistant named by alias carries no provider
/// field to fill it back in.
#[test]
fn inquiry_model_survives_an_aliased_assistant_layer() {
    use crate::{
        model::id::{ModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
        util::build,
    };

    let mut partial = PartialAppConfig::new_test();
    partial.providers.llm.aliases.insert(
        "opus".to_owned(),
        PartialModelIdOrAliasConfig::from("anthropic/claude-opus-5"),
    );
    partial.assistant.model.id = ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: "claude-opus-5".parse().unwrap(),
    }
    .to_partial()
    .into();

    // Same provider as the assistant, different model.
    partial.conversation.inquiry.assistant.model.id = ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: "claude-haiku-4-5".parse().unwrap(),
    }
    .to_partial()
    .into();

    let mut round_tripped = build(partial).expect("valid config").to_partial();

    assert_eq!(
        round_tripped
            .conversation
            .inquiry
            .assistant
            .model
            .id
            .to_string(),
        "anthropic/claude-haiku-4-5",
        "the recorded diff carries the whole id, not just the changed name"
    );

    round_tripped.assistant.model.id = PartialModelIdOrAliasConfig::Alias("opus".to_owned());

    let config = build(round_tripped).expect("valid config");

    assert_eq!(
        config
            .conversation
            .inquiry
            .assistant
            .model
            .id
            .resolved()
            .to_string(),
        "anthropic/claude-haiku-4-5",
        "the inquiry keeps its own model, provider included"
    );
}

/// An inquiry model keeps its own provider when the assistant moves to another
/// one.
///
/// An id is one value across two fields: an inquiry left holding only
/// `claude-haiku-4-5` takes `openai` from the assistant below it and resolves
/// to a model that does not exist, which fails only once the request is sent.
#[test]
fn an_inquiry_model_keeps_its_provider_when_the_assistant_changes_provider() {
    use crate::{
        model::id::{ModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
        util::build,
    };

    let mut partial = PartialAppConfig::new_test();
    partial.providers.llm.aliases.insert(
        "sol".to_owned(),
        PartialModelIdOrAliasConfig::from("openai/gpt-5.6-sol"),
    );
    partial.assistant.model.id = ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: "claude-opus-5".parse().unwrap(),
    }
    .to_partial()
    .into();
    partial.conversation.inquiry.assistant.model.id = ModelIdConfig {
        provider: ProviderId::Anthropic,
        name: "claude-haiku-4-5".parse().unwrap(),
    }
    .to_partial()
    .into();

    // The assistant moves to another provider; the inquiry says nothing about
    // it.
    let mut round_tripped = build(partial).expect("valid config").to_partial();
    round_tripped.assistant.model.id = PartialModelIdOrAliasConfig::Alias("sol".to_owned());

    let config = build(round_tripped).expect("valid config");

    assert_eq!(
        config
            .conversation
            .inquiry
            .assistant
            .model
            .id
            .resolved()
            .to_string(),
        "anthropic/claude-haiku-4-5",
        "the inquiry keeps the provider its own model needs"
    );
    assert_eq!(
        config.assistant.model.id.resolved().to_string(),
        "openai/gpt-5.6-sol",
        "and the assistant moves to the model its alias names"
    );
}

/// A half-written inquiry model id takes its missing half from an assistant
/// model named by alias.
///
/// `--cfg conversation.inquiry.assistant.model.id.name=<name>` sets one field
/// of the pair.
/// The other has to come from the assistant, which is reachable only once the
/// alias naming it has been resolved.
#[test]
fn a_half_written_inquiry_model_id_inherits_from_an_aliased_assistant() {
    use crate::{
        model::id::{PartialModelIdConfig, PartialModelIdOrAliasConfig},
        util::build,
    };

    let mut partial = PartialAppConfig::new_test();
    partial.providers.llm.aliases.insert(
        "opus".to_owned(),
        PartialModelIdOrAliasConfig::from("anthropic/claude-opus-5"),
    );
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Alias("opus".to_owned());
    partial.conversation.inquiry.assistant.model.id = PartialModelIdConfig {
        provider: None,
        name: Some("claude-haiku-4-5".parse().unwrap()),
    }
    .into();

    let config = build(partial).expect("valid config");

    assert_eq!(
        config
            .conversation
            .inquiry
            .assistant
            .model
            .id
            .resolved()
            .to_string(),
        "anthropic/claude-haiku-4-5",
        "the inquiry keeps its own name and inherits the assistant's provider"
    );
    assert_eq!(
        config.assistant.model.id.resolved().to_string(),
        "anthropic/claude-opus-5",
        "and the assistant keeps the model its alias names"
    );
}

/// An alias the map cannot resolve is still reported as an unresolved alias,
/// not as a missing field.
#[test]
fn an_unknown_assistant_alias_reports_itself() {
    use crate::{model::id::PartialModelIdOrAliasConfig, util::build};

    let mut partial = PartialAppConfig::new_test();
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Alias("nope".to_owned());

    let error = build(partial).unwrap_err().to_string();

    assert_eq!(
        error,
        "assistant.model.id: model ID does not match a known alias nor matches the full ID format \
         <provider>/<model>",
        "an unresolvable alias is reported as one, not as a missing field"
    );
}

/// An inquiry value the user genuinely set survives the same round-trip.
#[test]
fn an_explicit_inquiry_value_survives_a_partial_round_trip() {
    use crate::assistant::request::MaxResponseBytes;

    let mut partial = PartialAppConfig::new_test();
    partial.assistant.request.max_response_bytes = Some(MaxResponseBytes::Bytes(500_000));
    partial
        .conversation
        .inquiry
        .assistant
        .request
        .max_response_bytes = Some(MaxResponseBytes::Bytes(4096));

    let config = AppConfig::from_partial_with_defaults(partial).expect("valid config");
    let config = AppConfig::from_partial_with_defaults(config.to_partial()).expect("valid config");

    assert_eq!(
        config
            .conversation
            .inquiry
            .assistant
            .request
            .max_response_bytes,
        MaxResponseBytes::Bytes(4096),
        "an explicitly-set inquiry value is not mistaken for an inherited one"
    );
}

/// An MCP server whose only difference cannot be expressed as a delta does not
/// produce one.
///
/// `arguments` merges by appending, so a dropped argument has no delta to
/// record.
/// Keeping the server in the map anyway makes the whole partial look non-empty,
/// and every turn then writes a `config_delta` event holding nothing but the
/// server's transport tag.
#[test]
fn an_mcp_server_with_no_expressible_change_yields_no_delta() {
    use crate::providers::mcp::{McpProviderConfig, StdioConfig};

    let server = |arguments: &[&str]| {
        McpProviderConfig::Stdio(StdioConfig {
            command: "just".into(),
            arguments: arguments.iter().map(|a| (*a).to_owned()).collect(),
            variables: vec![],
            checksum: None,
            optional: false,
            startup_timeout_secs: 60,
        })
    };

    let mut prev = AppConfig::new_test();
    prev.providers
        .mcp
        .insert("bookworm".to_owned(), server(&["serve", "--verbose"]));

    let mut next = prev.clone();
    next.providers
        .mcp
        .insert("bookworm".to_owned(), server(&["serve"]));

    let delta = prev.to_partial().delta(next.to_partial());

    assert!(
        delta.providers.mcp.is_empty(),
        "expected no server entry, got: {:?}",
        delta.providers.mcp
    );
    assert!(delta.is_empty(), "expected an empty delta, got: {delta:?}");
}

/// A union that names an expanded form contributes both the shorthand path and
/// the expanded keys; a union of distinct values contributes only its path.
///
/// `enable` accepts `true` or `{ state, allow_toggle }`, and `editor.cmd`
/// accepts `"code --wait"` or `{ program, args }`.
/// Both spell one value two ways, so every spelling is writable.
/// `assistant.model.id` is either an id or an alias resolved through a lookup,
/// which are different values rather than spellings of one, so it contributes
/// only its own path.
#[test]
fn fields_follows_a_unions_expanded_form() {
    let fields = AppConfig::fields();
    let has = |key: &str| fields.contains(&key.to_owned());

    assert!(
        has("conversation.tools.*.enable"),
        "the shorthand `enable = true` stays writable"
    );
    assert!(
        has("conversation.tools.*.enable.state") && has("conversation.tools.*.enable.allow_toggle"),
        "the expanded form contributes its keys"
    );

    assert!(
        has("editor.cmd"),
        "the shorthand `cmd = \"code\"` stays writable"
    );
    assert!(
        has("editor.cmd.program") && has("editor.cmd.args") && has("editor.cmd.shell"),
        "the table form contributes its keys"
    );

    assert!(has("assistant.model.id"), "an id-or-alias is a leaf");
    assert!(
        !has("assistant.model.id.provider"),
        "an alias is not a spelling of an id, so its keys are not reported"
    );
}

/// Every assignable sub-field of an optional nested config appears in
/// `fields()`.
///
/// `Option<NestedConfig>` renders as a nullable union rather than a struct, so
/// a walk that only descends into structs stops at the block and reports it as
/// a leaf.
/// `assign` routes into those sub-keys regardless, and `envs()` is derived from
/// `fields()`, so the omission silently costs the env-var form of every key
/// inside such a block.
#[test]
fn fields_descends_into_optional_nested_configs() {
    let fields = AppConfig::fields();

    for key in [
        "conversation.title.generate.model.id",
        "style.reasoning.summary_model.id",
    ] {
        assert!(
            fields.contains(&key.to_owned()),
            "{key} is assignable but missing from fields()"
        );
    }

    // The block itself is no longer reported as a leaf: it has no value of its
    // own to set.
    assert!(
        !fields.contains(&"conversation.title.generate.model".to_owned()),
        "the containing block must not also appear as a leaf"
    );
}

#[test]
fn test_ensure_no_missing_assignments() {
    // Some fields cannot be assigned via CLI.
    //
    // `loader.reset` is load-time metadata steering how the declaring file is
    // loaded ([RFD 038]): it counts only in a file's own `[loader]` section
    // and never becomes part of the resolved application config.
    let skip_fields = ["extends", "loader.reset"];

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
fn config_load_paths_append_across_layers() {
    // Each layer adds its search directories to the accumulated list instead of
    // replacing it, and a directory named by two layers is kept once. Order
    // matters downstream: `--cfg <name>` resolution walks the list and takes
    // the first directory that holds a matching file.
    let mut base = PartialAppConfig::empty();
    base.config_load_paths = Some(vec![".jp/global".into(), ".jp/shared".into()]);

    let mut overlay = PartialAppConfig::empty();
    overlay.config_load_paths = Some(vec![".jp/shared".into(), ".jp/workspace".into()]);

    base.merge(&(), overlay).unwrap();

    let want: Vec<RelativePathBuf> = vec![
        ".jp/global".into(),
        ".jp/shared".into(),
        ".jp/workspace".into(),
    ];
    assert_eq!(base.config_load_paths, Some(want));
}

#[test]
fn assign_routes_nested_system_prompt_keys() {
    use crate::types::string::{MergedStringStrategy, PartialMergeableString, PartialMergedString};

    // `--cfg assistant.system_prompt.dedup=false` addresses the merge metadata,
    // so it has to reach `PartialMergedString` rather than stopping at
    // `PartialAssistantConfig` with an unknown key.
    let mut p = PartialAppConfig::default();

    let kv = KvAssignment::try_from_cli("assistant.system_prompt.dedup", "false").unwrap();
    p.assign(kv).unwrap();

    let kv = KvAssignment::try_from_cli("assistant.system_prompt.strategy", "prepend").unwrap();
    p.assign(kv).unwrap();

    assert_eq!(
        p.assistant.system_prompt,
        Some(PartialMergeableString::Merged(PartialMergedString {
            value: None,
            strategy: Some(MergedStringStrategy::Prepend),
            separator: None,
            discard_when_merged: None,
            dedup: Some(false),
        }))
    );

    // `inherit` states no opinion, leaving the opt-out set above in force.
    let kv = KvAssignment::try_from_cli("assistant.system_prompt.dedup", "inherit").unwrap();
    p.assign(kv).unwrap();

    assert_eq!(
        p.assistant.system_prompt,
        Some(PartialMergeableString::Merged(PartialMergedString {
            value: None,
            strategy: Some(MergedStringStrategy::Prepend),
            separator: None,
            discard_when_merged: None,
            dedup: Some(false),
        }))
    );

    // `system_prompt_sections` shares the `system_prompt` prefix but is a
    // different field, and must not be captured by the nested route.
    let kv = KvAssignment::try_from_cli("assistant.system_prompt_sections:", r#"[{"tag":"foo"}]"#)
        .unwrap();
    p.assign(kv).unwrap();

    assert_eq!(
        p.assistant.system_prompt_sections.first().unwrap().tag,
        Some("foo".to_owned())
    );
}

#[test]
fn metadata_only_system_prompt_keeps_the_default_prompt() {
    // A metadata-only override states no value, so the built-in default has to
    // survive gap-filling. Without it, `--cfg assistant.system_prompt.dedup=false`
    // resolves the prompt to an empty string.
    let mut p = PartialAppConfig::new_test();

    let kv = KvAssignment::try_from_cli("assistant.system_prompt.dedup", "false").unwrap();
    p.assign(kv).unwrap();

    let config = AppConfig::from_partial_with_defaults(p).unwrap();

    assert_eq!(
        config.assistant.system_prompt.as_deref(),
        Some("You are a helpful assistant.")
    );
}

#[test]
fn scalar_system_prompt_accepts_nested_metadata() {
    use crate::types::string::{MergedStringStrategy, PartialMergeableString, PartialMergedString};

    // A lower layer supplying the common scalar form must not block a dotted
    // override. The scalar is promoted to `Merged` with `replace` pinned, which
    // is what the plain form means.
    let mut p = PartialAppConfig::new_test();

    let kv = KvAssignment::try_from_cli("assistant.system_prompt", "base").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.assistant.system_prompt,
        Some(PartialMergeableString::String("base".to_owned()))
    );

    let kv = KvAssignment::try_from_cli("assistant.system_prompt.dedup", "false").unwrap();
    p.assign(kv).unwrap();

    assert_eq!(
        p.assistant.system_prompt,
        Some(PartialMergeableString::Merged(PartialMergedString {
            value: Some("base".to_owned()),
            strategy: Some(MergedStringStrategy::Replace),
            separator: None,
            discard_when_merged: None,
            dedup: Some(false),
        }))
    );

    // The promotion preserves the scalar's meaning end to end.
    let config = AppConfig::from_partial_with_defaults(p).unwrap();
    assert_eq!(config.assistant.system_prompt.as_deref(), Some("base"));
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
        tool_calls: Some(ToolCallsMode::StripRequests.into()),
        ..Default::default()
    }]);

    let config = build(partial).expect("valid config");

    let rule = &config.conversation.compaction.rules[0];
    assert_eq!(rule.keep_first, RuleBound::Turns(1));
    assert_eq!(rule.keep_last, RuleBound::Turns(1));
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
    assert_eq!(rules[0].reasoning, Some(ReasoningMode::Strip.into()));
    assert_eq!(rules[0].tool_calls, Some(ToolCallsMode::Strip.into()));
    assert_eq!(rules[0].keep_first, RuleBound::Turns(1));
    assert_eq!(rules[0].keep_last, RuleBound::Turns(1));
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
