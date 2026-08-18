use super::*;

#[test]
fn parse_policy_only() {
    assert_eq!("s".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: None,
        tools: None,
        summarize: true,
        range: None,
    });
    assert_eq!("r+t=strip".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: Some(ReasoningMode::Strip.into()),
        tools: Some(ToolCallsMode::Strip.into()),
        summarize: false,
        range: None,
    });
    assert_eq!(
        "reasoning+tools=strip+summarize"
            .parse::<CompactSpec>()
            .unwrap(),
        CompactSpec {
            reasoning: Some(ReasoningMode::Strip.into()),
            tools: Some(ToolCallsMode::Strip.into()),
            summarize: true,
            range: None,
        }
    );
}

#[test]
fn parse_tool_modes() {
    let mode = |s: &str| s.parse::<CompactSpec>().unwrap().tools;
    // Bare `t` / `tools` defaults to stripping both.
    assert_eq!(mode("t"), Some(ToolCallsMode::Strip.into()));
    assert_eq!(mode("tools"), Some(ToolCallsMode::Strip.into()));
    assert_eq!(mode("t=strip"), Some(ToolCallsMode::Strip.into()));
    assert_eq!(mode("t=s"), Some(ToolCallsMode::Strip.into()));
    assert_eq!(mode("t=sreq"), Some(ToolCallsMode::StripRequests.into()));
    assert_eq!(
        mode("tools=strip-responses"),
        Some(ToolCallsMode::StripResponses.into())
    );
    assert_eq!(mode("t=o"), Some(ToolCallsMode::Omit.into()));
}

#[test]
fn parse_tool_mode_with_range() {
    assert_eq!("t=sres:..-3".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: None,
        tools: Some(ToolCallsMode::StripResponses.into()),
        summarize: false,
        range: Some(DslRange {
            from: None,
            to: Some(RuleBound::FromEnd(3)),
        }),
    });
}

#[test]
fn parse_with_range() {
    assert_eq!("s:..-3".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: None,
        tools: None,
        summarize: true,
        range: Some(DslRange {
            from: None,
            to: Some(RuleBound::FromEnd(3)),
        }),
    });
    assert_eq!(
        "r+t=strip:5..-3".parse::<CompactSpec>().unwrap(),
        CompactSpec {
            reasoning: Some(ReasoningMode::Strip.into()),
            tools: Some(ToolCallsMode::Strip.into()),
            summarize: false,
            range: Some(DslRange {
                from: Some(RuleBound::Absolute(5)),
                to: Some(RuleBound::FromEnd(3)),
            }),
        }
    );
    assert_eq!("s:..".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: None,
        tools: None,
        summarize: true,
        range: Some(DslRange {
            from: None,
            to: None,
        }),
    });
    assert_eq!("r:5..".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: Some(ReasoningMode::Strip.into()),
        tools: None,
        summarize: false,
        range: Some(DslRange {
            from: Some(RuleBound::Absolute(5)),
            to: None,
        }),
    });
}

#[test]
fn parse_absolute_range() {
    // Python-slice: positive bounds are absolute turn indices on both ends.
    assert_eq!(
        "s:5..10".parse::<CompactSpec>().unwrap().range,
        Some(DslRange {
            from: Some(RuleBound::Absolute(5)),
            to: Some(RuleBound::Absolute(10)),
        })
    );
    assert_eq!(
        "s:..10".parse::<CompactSpec>().unwrap().range,
        Some(DslRange {
            from: None,
            to: Some(RuleBound::Absolute(10)),
        })
    );
    assert_eq!(
        "s:-10..-3".parse::<CompactSpec>().unwrap().range,
        Some(DslRange {
            from: Some(RuleBound::FromEnd(10)),
            to: Some(RuleBound::FromEnd(3)),
        })
    );
}

#[test]
fn parse_single_number_shorthand() {
    // Negative shorthand `-3` = `..-3` (keep last 3).
    assert_eq!("s:-3".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: None,
        tools: None,
        summarize: true,
        range: Some(DslRange {
            from: None,
            to: Some(RuleBound::FromEnd(3)),
        }),
    });
    // Positive shorthand `5` = `5..` (keep first 5).
    assert_eq!("r:5".parse::<CompactSpec>().unwrap(), CompactSpec {
        reasoning: Some(ReasoningMode::Strip.into()),
        tools: None,
        summarize: false,
        range: Some(DslRange {
            from: Some(RuleBound::Absolute(5)),
            to: None,
        }),
    });
}

#[test]
fn parse_errors() {
    assert!("".parse::<CompactSpec>().is_err());
    assert!("x".parse::<CompactSpec>().is_err());
    assert!("s:abc".parse::<CompactSpec>().is_err());
    // Non-numeric bound
    assert!("s:5..x".parse::<CompactSpec>().is_err());
    // Absolute bounds are 1-based; `0` is invalid.
    assert!("s:0..".parse::<CompactSpec>().is_err());
    assert!("s:0..5".parse::<CompactSpec>().is_err());
    // Unknown tool mode
    assert!("t=nope".parse::<CompactSpec>().is_err());
    // Boolean policies reject values
    assert!("r=strip".parse::<CompactSpec>().is_err());
    assert!("s=true".parse::<CompactSpec>().is_err());
}

// ---------------------------------------------------------------------------
// Policy options (`,over=SIZE`)
// ---------------------------------------------------------------------------

#[test]
fn parse_over_option_binds_to_its_own_policy() {
    // The whole point of binding options to a policy rather than joining them
    // with `+`: two policies in one spec can carry different thresholds.
    let spec = "r,over=16kb+t=sres,over=1mb"
        .parse::<CompactSpec>()
        .unwrap();

    assert_eq!(
        spec.reasoning,
        Some(PolicySpec::over(
            ReasoningMode::Strip,
            ByteSize::from_bytes(16 * 1024)
        ))
    );
    assert_eq!(
        spec.tools,
        Some(PolicySpec::over(
            ToolCallsMode::StripResponses,
            ByteSize::from_bytes(1024 * 1024)
        ))
    );
}

#[test]
fn parse_over_option_composes_with_a_range() {
    let spec = "t=sres,over=1mb:..-3".parse::<CompactSpec>().unwrap();

    assert_eq!(
        spec.tools,
        Some(PolicySpec::over(
            ToolCallsMode::StripResponses,
            ByteSize::from_bytes(1024 * 1024)
        ))
    );
    assert_eq!(
        spec.range,
        Some(DslRange {
            from: None,
            to: Some(RuleBound::FromEnd(3)),
        })
    );
}

#[test]
fn a_policy_without_an_option_carries_no_threshold() {
    let spec = "t=sres".parse::<CompactSpec>().unwrap();

    assert_eq!(
        spec.tools,
        Some(PolicySpec::new(ToolCallsMode::StripResponses))
    );
}

#[test]
fn parse_over_option_errors() {
    // A summary replaces its whole range rather than acting per item, so a
    // threshold on it would be a silent no-op.
    assert_eq!(
        "s,over=1mb".parse::<CompactSpec>().unwrap_err(),
        "`summarize` does not take an `over` threshold"
    );
    assert_eq!(
        "t,under=1mb".parse::<CompactSpec>().unwrap_err(),
        "unknown policy option 'under'"
    );
    assert_eq!(
        "t,over".parse::<CompactSpec>().unwrap_err(),
        "invalid policy option 'over': expected `key=value`"
    );
    // An option cannot stand on its own: with nothing before the comma it is
    // read as a policy name, and there is no such policy.
    assert_eq!(
        "over=1mb".parse::<CompactSpec>().unwrap_err(),
        "unknown policy 'over'"
    );
    assert!("t,over=nonsense".parse::<CompactSpec>().is_err());
}

#[test]
fn over_option_reaches_the_partial_rule() {
    let rule = "t=sres,over=1mb"
        .parse::<CompactSpec>()
        .unwrap()
        .to_partial_rule();

    assert_eq!(
        rule.tool_calls,
        Some(PolicySpec::over(
            ToolCallsMode::StripResponses,
            ByteSize::from_bytes(1024 * 1024)
        ))
    );
}

#[test]
fn to_partial_rule_with_range() {
    let spec = "r+t=strip:..-3".parse::<CompactSpec>().unwrap();
    let rule = spec.to_partial_rule();
    assert_eq!(rule.reasoning, Some(ReasoningMode::Strip.into()));
    assert_eq!(rule.tool_calls, Some(ToolCallsMode::Strip.into()));
    assert!(rule.summary.is_none());
    // Open start maps to keep-first 0 (compact from the first turn); `-3` keeps
    // the last 3.
    assert_eq!(rule.keep_first, Some(RuleBound::Turns(0)));
    assert_eq!(rule.keep_last, Some(RuleBound::FromEnd(3)));
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

/// Resolved config rules to combine against (the built-in default: strip
/// reasoning + tools).
fn config_rules() -> Vec<CompactionRuleConfig> {
    CompactionConfig::finalize_rules(
        jp_config::conversation::compaction::PartialCompactionConfig::builtin_rules(),
    )
    .unwrap()
}

#[test]
fn specs_only_replace_config_rules() {
    // No bare `--compact`: the inline DSL rule replaces the config rules.
    let flag = CompactFlag {
        use_config_rules: false,
        specs: vec!["t=sreq:..-3".parse().unwrap()],
    };
    let rules = flag.effective_rules(&config_rules()).unwrap();

    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0].tool_calls,
        Some(ToolCallsMode::StripRequests.into())
    );
}

#[test]
fn bare_compact_plus_dsl_appends_to_config_rules() {
    // `--compact` plus an inline DSL spec runs the config rules *and* the DSL
    // rule, config rules first.
    let flag = CompactFlag {
        use_config_rules: true,
        specs: vec!["s:..-3".parse().unwrap()],
    };
    let rules = flag.effective_rules(&config_rules()).unwrap();

    assert_eq!(rules.len(), 2);
    // Config default first (strip reasoning + tools), then the summary spec.
    assert_eq!(rules[0].reasoning, Some(ReasoningMode::Strip.into()));
    assert_eq!(rules[0].tool_calls, Some(ToolCallsMode::Strip.into()));
    assert!(rules[1].summary.is_some());
}

#[test]
fn bare_compact_only_uses_config_rules_unchanged() {
    let flag = CompactFlag {
        use_config_rules: true,
        specs: vec![],
    };
    let config = config_rules();
    let rules = flag.effective_rules(&config).unwrap();
    assert_eq!(rules, config);
}
