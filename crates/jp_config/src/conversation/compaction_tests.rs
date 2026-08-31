use super::*;
use crate::types::byte_size::ByteSize;

// ---------------------------------------------------------------------------
// Rule policies in TOML
// ---------------------------------------------------------------------------

/// Deserialize a single `[[conversation.compaction.rules]]` body from TOML.
fn rule_from_toml(body: &str) -> PartialCompactionRuleConfig {
    toml::from_str(body).unwrap()
}

#[test]
fn bare_string_policies_still_parse() {
    let rule = rule_from_toml(
        r#"
        reasoning = "strip"
        tool_calls = "strip-responses"
        "#,
    );

    assert_eq!(rule.reasoning, Some(ReasoningMode::Strip.into()));
    assert_eq!(rule.tool_calls, Some(ToolCallsMode::StripResponses.into()));
}

#[test]
fn table_policies_carry_a_size_threshold() {
    let rule = rule_from_toml(
        r#"
        reasoning = { policy = "strip", over = "16KB" }
        tool_calls = { policy = "strip-responses", over = "1MB" }
        "#,
    );

    assert_eq!(
        rule.reasoning,
        Some(PolicySpec::over(
            ReasoningMode::Strip,
            ByteSize::from_bytes(16 * 1024)
        ))
    );
    assert_eq!(
        rule.tool_calls,
        Some(PolicySpec::over(
            ToolCallsMode::StripResponses,
            ByteSize::from_bytes(1024 * 1024)
        ))
    );
}

#[test]
fn a_policy_table_without_over_matches_the_bare_string() {
    let table = rule_from_toml(r#"tool_calls = { policy = "omit" }"#);
    let bare = rule_from_toml(r#"tool_calls = "omit""#);

    assert_eq!(table.tool_calls, bare.tool_calls);
}

#[test]
fn a_policy_without_a_threshold_serializes_back_as_a_bare_string() {
    // Keeping the bare form when nothing narrows the policy means an existing
    // config or event stream is not rewritten into the table shape.
    let rule = rule_from_toml(r#"tool_calls = "strip""#);

    let json = serde_json::to_value(&rule).unwrap();

    assert_eq!(json["tool_calls"], serde_json::json!("strip"));
}

#[test]
fn a_misspelled_option_key_is_rejected() {
    // Silently dropping the leftover key would apply the policy to every item
    // in range instead of the large ones, which is the opposite of what the
    // line asks for. Every other type in this config tree rejects unknown
    // fields, so this does too.
    let err = toml::from_str::<PartialCompactionRuleConfig>(
        r#"tool_calls = { policy = "strip-responses", oer = "1MB" }"#,
    )
    .unwrap_err()
    .to_string();

    assert!(err.contains("unknown policy option `oer`"), "{err}");
}

#[test]
fn a_tagged_policy_keeps_its_own_sibling_fields() {
    // The rejection above must not catch a policy that legitimately serializes
    // as a map with fields of its own, which is how the stored `ToolCallPolicy`
    // is shaped.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(tag = "policy", rename_all = "snake_case")]
    enum Tagged {
        Strip { request: bool, response: bool },
    }

    let spec: PolicySpec<Tagged> = serde_json::from_value(serde_json::json!({
        "policy": "strip",
        "request": false,
        "response": true,
        "over": "1MB",
    }))
    .unwrap();

    assert_eq!(spec.policy, Tagged::Strip {
        request: false,
        response: true,
    });
    assert_eq!(spec.over, Some(ByteSize::from_bytes(1024 * 1024)));
}

#[test]
fn assigning_a_policy_string_accepts_an_over_option() {
    // `--cfg ...tool_calls=sres,over=1MB` goes through `FromStr`, which shares
    // its option syntax with the inline DSL.
    let spec: PolicySpec<ToolCallsMode> = "sres,over=1MB".parse().unwrap();

    assert_eq!(
        spec,
        PolicySpec::over(
            ToolCallsMode::StripResponses,
            ByteSize::from_bytes(1024 * 1024)
        )
    );
}

#[test]
fn builtin_rules_carry_no_threshold() {
    // The out-of-the-box behavior must stay "compact everything in range";
    // adding a default threshold would silently change every workspace.
    let rules = PartialCompactionConfig::builtin_rules();

    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].reasoning.and_then(|spec| spec.over), None);
    assert_eq!(rules[0].tool_calls.and_then(|spec| spec.over), None);
}

#[test]
fn tool_calls_mode_parse() {
    assert_eq!(
        "strip".parse::<ToolCallsMode>().unwrap(),
        ToolCallsMode::Strip
    );
    assert_eq!(
        "strip-responses".parse::<ToolCallsMode>().unwrap(),
        ToolCallsMode::StripResponses
    );
    assert_eq!(
        "strip_responses".parse::<ToolCallsMode>().unwrap(),
        ToolCallsMode::StripResponses
    );
    assert_eq!(
        "strip-requests".parse::<ToolCallsMode>().unwrap(),
        ToolCallsMode::StripRequests
    );
    assert_eq!(
        "omit".parse::<ToolCallsMode>().unwrap(),
        ToolCallsMode::Omit
    );
    assert!("invalid".parse::<ToolCallsMode>().is_err());
}

#[test]
fn tool_calls_mode_parse_short_aliases() {
    assert_eq!("s".parse::<ToolCallsMode>().unwrap(), ToolCallsMode::Strip);
    assert_eq!(
        "sreq".parse::<ToolCallsMode>().unwrap(),
        ToolCallsMode::StripRequests
    );
    assert_eq!(
        "sres".parse::<ToolCallsMode>().unwrap(),
        ToolCallsMode::StripResponses
    );
    assert_eq!("o".parse::<ToolCallsMode>().unwrap(), ToolCallsMode::Omit);
}

#[test]
fn tool_calls_mode_roundtrip() {
    for mode in [
        ToolCallsMode::Strip,
        ToolCallsMode::StripResponses,
        ToolCallsMode::StripRequests,
        ToolCallsMode::Omit,
    ] {
        let s = mode.to_string();
        assert_eq!(s.parse::<ToolCallsMode>().unwrap(), mode);
    }
}

#[test]
fn reasoning_mode_parse() {
    assert_eq!(
        "strip".parse::<ReasoningMode>().unwrap(),
        ReasoningMode::Strip
    );
}

#[test]
fn rule_bound_deserializes_from_integer_and_string() {
    // Config files write bare integers (`keep_first = 1`); those must map to
    // `Turns`, while string forms keep working.
    assert_eq!(
        serde_json::from_value::<RuleBound>(serde_json::json!(3)).unwrap(),
        RuleBound::Turns(3)
    );
    assert_eq!(
        serde_json::from_value::<RuleBound>(serde_json::json!("last-compaction")).unwrap(),
        RuleBound::AfterLastCompaction
    );
    assert!(matches!(
        serde_json::from_value::<RuleBound>(serde_json::json!("5h")).unwrap(),
        RuleBound::Duration(_)
    ));
}

#[test]
fn rule_bound_after_last_compaction_round_trips() {
    assert_eq!(
        "last-compaction".parse::<RuleBound>().unwrap(),
        RuleBound::AfterLastCompaction
    );
    assert_eq!(
        RuleBound::AfterLastCompaction.to_string(),
        "last-compaction"
    );

    // `last` alone is a duration-shaped input and no longer names the marker.
    assert!("last".parse::<RuleBound>().is_err());
}

#[test]
fn rule_bound_from_end_is_one_based_and_rejects_zero() {
    // `-1` is the last turn, matching `--from -1` / `--to -1` / `--turn -1`.
    assert_eq!("-1".parse::<RuleBound>().unwrap(), RuleBound::FromEnd(0));
    assert_eq!("-3".parse::<RuleBound>().unwrap(), RuleBound::FromEnd(2));
    assert!("-0".parse::<RuleBound>().is_err());

    // The written form round-trips through the stored 0-based offset.
    assert_eq!(RuleBound::FromEnd(0).to_string(), "-1");
    assert_eq!(RuleBound::FromEnd(2).to_string(), "-3");
}

#[test]
fn rule_bound_rejects_absolute_turns() {
    // A config rule applies to every conversation, so an absolute turn has no
    // spelling here — not as input, and not on the way back out.
    assert!("@1".parse::<RuleBound>().is_err());
    assert!("@5".parse::<RuleBound>().is_err());

    let err = serde_json::to_value(RuleBound::Absolute(5)).unwrap_err();
    assert_eq!(
        err.to_string(),
        "absolute turn `@5` cannot be stored in a config rule; use a turn count, a duration, or a \
         from-end position like `-3`"
    );
}

#[test]
fn rule_config_deserializes_integer_bounds() {
    // Two distinct explicit bounds, exercising integer deserialization.
    let rule: PartialCompactionRuleConfig =
        serde_json::from_value(serde_json::json!({ "keep_first": 1, "keep_last": 3 })).unwrap();
    assert_eq!(rule.keep_first, Some(RuleBound::Turns(1)));
    assert_eq!(rule.keep_last, Some(RuleBound::Turns(3)));
}

#[test]
fn rule_partial_roundtrip_json() {
    let rule = PartialCompactionRuleConfig {
        keep_first: None,
        keep_last: Some(RuleBound::Turns(3)),
        reasoning: Some(ReasoningMode::Strip.into()),
        tool_calls: Some(ToolCallsMode::Strip.into()),
        summary: None,
    };
    let json = serde_json::to_value(&rule).unwrap();
    let deserialized: PartialCompactionRuleConfig = serde_json::from_value(json).unwrap();
    assert_eq!(rule, deserialized);
}

#[test]
fn rule_partial_none_fields_omitted() {
    let rule = PartialCompactionRuleConfig {
        keep_first: None,
        keep_last: None,
        reasoning: Some(ReasoningMode::Strip.into()),
        tool_calls: None,
        summary: None,
    };
    let json = serde_json::to_value(&rule).unwrap();
    let obj = json.as_object().unwrap();
    assert!(obj.contains_key("reasoning"));
    assert!(!obj.contains_key("tool_calls"));
}
