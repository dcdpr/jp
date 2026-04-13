use super::*;

#[test]
fn parse_policy_only() {
    assert_eq!("s".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: false,
        tools: None,
        summarize: true,
        range: None,
    });
    assert_eq!("r+t=strip".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: true,
        tools: Some(ToolCallsMode::Strip),
        summarize: false,
        range: None,
    });
    assert_eq!(
        "reasoning+tools=strip+summarize"
            .parse::<CompactSpec>()
            .unwrap(),
        CompactSpec {
            reasoning: true,
            tools: Some(ToolCallsMode::Strip),
            summarize: true,
            range: None,
        }
    );
}

#[test]
fn parse_tool_modes() {
    let mode = |s: &str| s.parse::<CompactSpec>().unwrap().tools;
    // Bare `t` / `tools` defaults to stripping both.
    assert_eq!(mode("t"), Some(ToolCallsMode::Strip));
    assert_eq!(mode("tools"), Some(ToolCallsMode::Strip));
    assert_eq!(mode("t=strip"), Some(ToolCallsMode::Strip));
    assert_eq!(mode("t=s"), Some(ToolCallsMode::Strip));
    assert_eq!(mode("t=sreq"), Some(ToolCallsMode::StripRequests));
    assert_eq!(
        mode("tools=strip-responses"),
        Some(ToolCallsMode::StripResponses)
    );
    assert_eq!(mode("t=o"), Some(ToolCallsMode::Omit));
}

#[test]
fn parse_tool_mode_with_range() {
    assert_eq!("t=sres:..-3".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: false,
        tools: Some(ToolCallsMode::StripResponses),
        summarize: false,
        range: Some(DslRange {
            keep_first: None,
            keep_last: Some(3),
        }),
    });
}

#[test]
fn parse_with_range() {
    assert_eq!("s:..-3".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: false,
        tools: None,
        summarize: true,
        range: Some(DslRange {
            keep_first: None,
            keep_last: Some(3),
        }),
    });
    assert_eq!(
        "r+t=strip:5..-3".parse::<CompactSpec>().unwrap(),
        CompactSpec {
            reasoning: true,
            tools: Some(ToolCallsMode::Strip),
            summarize: false,
            range: Some(DslRange {
                keep_first: Some(5),
                keep_last: Some(3),
            }),
        }
    );
    assert_eq!("s:..".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: false,
        tools: None,
        summarize: true,
        range: Some(DslRange {
            keep_first: None,
            keep_last: None,
        }),
    });
    assert_eq!("r:5..".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: true,
        tools: None,
        summarize: false,
        range: Some(DslRange {
            keep_first: Some(5),
            keep_last: None,
        }),
    });
}

#[test]
fn parse_single_number_shorthand() {
    // Negative: keep last N
    assert_eq!("s:-3".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: false,
        tools: None,
        summarize: true,
        range: Some(DslRange {
            keep_first: None,
            keep_last: Some(3),
        }),
    });
    // Positive: keep first N
    assert_eq!("r:5".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: true,
        tools: None,
        summarize: false,
        range: Some(DslRange {
            keep_first: Some(5),
            keep_last: None,
        }),
    });
}

#[test]
fn parse_errors() {
    assert!("".parse::<CompactSpec>().is_err());
    assert!("x".parse::<CompactSpec>().is_err());
    assert!("s:abc".parse::<CompactSpec>().is_err());
    // Positive right bound not supported
    assert!("s:5..10".parse::<CompactSpec>().is_err());
    // Unknown tool mode
    assert!("t=nope".parse::<CompactSpec>().is_err());
    // Boolean policies reject values
    assert!("r=strip".parse::<CompactSpec>().is_err());
    assert!("s=true".parse::<CompactSpec>().is_err());
}

#[test]
fn to_partial_rule_with_range() {
    let spec = "r+t=strip:..-3".parse::<CompactSpec>().unwrap();
    let rule = spec.to_partial_rule();
    assert_eq!(rule.reasoning, Some(ReasoningMode::Strip));
    assert_eq!(rule.tool_calls, Some(ToolCallsMode::Strip));
    assert!(rule.summary.is_none());
    assert_eq!(rule.keep_first, Some(RuleBound::Turns(0)));
    assert_eq!(rule.keep_last, Some(RuleBound::Turns(3)));
}

#[test]
fn to_partial_rule_no_range() {
    let spec = "s".parse::<CompactSpec>().unwrap();
    let rule = spec.to_partial_rule();
    assert!(rule.reasoning.is_none());
    assert!(rule.tool_calls.is_none());
    assert!(rule.summary.is_some());
    // No range → None → use config defaults
    assert!(rule.keep_first.is_none());
    assert!(rule.keep_last.is_none());
}

#[test]
fn apply_specs_only_replaces_rules() {
    let flag = CompactFlag {
        use_config_rules: false,
        specs: vec!["t=sreq:..-3".parse().unwrap()],
    };
    let mut partial = PartialAppConfig::default();
    flag.apply_to_config(&mut partial);

    let rules: &[_] = &partial.conversation.compaction.rules;
    assert_eq!(rules.len(), 1);
}

#[test]
fn apply_bare_compact_leaves_config_unchanged() {
    let flag = CompactFlag {
        use_config_rules: true,
        specs: vec![],
    };
    let mut partial = PartialAppConfig::default();
    let before = partial.conversation.compaction.rules.len();
    flag.apply_to_config(&mut partial);
    assert_eq!(partial.conversation.compaction.rules.len(), before);
}
