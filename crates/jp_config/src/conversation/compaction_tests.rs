use super::*;

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
fn rule_partial_roundtrip_json() {
    let rule = PartialCompactionRuleConfig {
        keep_first: None,
        keep_last: Some(RuleBound::Turns(3)),
        reasoning: Some(ReasoningMode::Strip),
        tool_calls: Some(ToolCallsMode::Strip),
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
        reasoning: Some(ReasoningMode::Strip),
        tool_calls: None,
        summary: None,
    };
    let json = serde_json::to_value(&rule).unwrap();
    let obj = json.as_object().unwrap();
    assert!(obj.contains_key("reasoning"));
    assert!(!obj.contains_key("tool_calls"));
}
