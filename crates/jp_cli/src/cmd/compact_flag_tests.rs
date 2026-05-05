use super::*;

#[test]
fn parse_policy_only() {
    assert_eq!("s".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: false,
        tools: false,
        summarize: true,
        range: None,
    });
    assert_eq!("r+t".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: true,
        tools: true,
        summarize: false,
        range: None,
    });
    assert_eq!(
        "reasoning+tools+summarize".parse::<CompactSpec>().unwrap(),
        CompactSpec {
            reasoning: true,
            tools: true,
            summarize: true,
            range: None,
        }
    );
}

#[test]
fn parse_with_range() {
    assert_eq!("s:..-3".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: false,
        tools: false,
        summarize: true,
        range: Some(DslRange {
            keep_first: None,
            keep_last: Some(3),
        }),
    });
    assert_eq!("r+t:5..-3".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: true,
        tools: true,
        summarize: false,
        range: Some(DslRange {
            keep_first: Some(5),
            keep_last: Some(3),
        }),
    });
    assert_eq!("s:..".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: false,
        tools: false,
        summarize: true,
        range: Some(DslRange {
            keep_first: None,
            keep_last: None,
        }),
    });
    assert_eq!("r:5..".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: true,
        tools: false,
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
        tools: false,
        summarize: true,
        range: Some(DslRange {
            keep_first: None,
            keep_last: Some(3),
        }),
    });
    // Positive: keep first N
    assert_eq!("r:5".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: true,
        tools: false,
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
}

#[test]
fn to_partial_rule_with_range() {
    let spec = "r+t:..-3".parse::<CompactSpec>().unwrap();
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
        specs: vec!["s:..-3".parse().unwrap()],
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
