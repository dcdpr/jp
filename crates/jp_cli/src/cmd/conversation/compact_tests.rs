use std::time::Duration;

use clap::Parser as _;
use jp_config::{
    AppConfig,
    conversation::compaction::{CompactionRuleConfig, ReasoningMode, RuleBound, ToolCallsMode},
};
use jp_conversation::{
    Compaction, ConversationStream, RangeBound, ReasoningPolicy, ToolCallPolicy,
    event::{ToolCallRequest, ToolCallResponse},
};
use jp_printer::Printer;
use serde_json::{Map, Value};

use super::{
    Bound, Compact, TimelineSegment, build_compaction_events, segments_for_compactions,
    timeline_lines,
};

/// Parse a `Compact` from `jp conversation compact <args>` for flag tests.
fn parse_compact(args: &[&str]) -> Compact {
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        compact: Compact,
    }

    let mut argv = vec!["compact"];
    argv.extend_from_slice(args);
    TestCli::try_parse_from(argv).unwrap().compact
}

#[test]
fn bare_compact_flag_parses_without_a_value() {
    // Bare `--compact` (no value) means "apply config rules".
    let compact = parse_compact(&["--compact"]);
    assert!(compact.compact_flag.use_config_rules);
    assert!(compact.compact_flag.specs.is_empty());
}

#[test]
fn keep_last_only_does_not_inject_a_policyless_rule() {
    // Range-only flags carry no policy, so `effective_rules` must fall through
    // to the configured rules unchanged rather than synthesize a policy-less
    // rule (which would project to a no-op). The range itself is applied as a
    // runtime override on those rules, not as a rule of its own.
    let compact = parse_compact(&["--keep-last", "5"]);
    let cfg = AppConfig::new_test();
    let rules = compact.effective_rules(&cfg).unwrap();
    assert_eq!(
        rules, cfg.conversation.compaction.rules,
        "range-only flags must leave the active rules untouched"
    );
}

#[test]
fn policy_flag_conflicts_with_dsl_spec() {
    // Dedicated policy flags and the `-k` DSL are mutually exclusive: combining
    // them is a parse error rather than silently dropping one side.
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        compact: Compact,
    }

    let result = TestCli::try_parse_from(["compact", "--reasoning", "-k", "s:..-3"]);
    assert!(
        result.is_err(),
        "--reasoning and -k DSL must conflict, got {:?}",
        result.map(|c| c.compact.compact_flag.specs)
    );
}

#[test]
fn reset_conflicts_with_selection_flags() {
    // `--reset` undoes compaction; combining it with policy/range/DSL flags is a
    // parse error rather than silently dropping them on the early-return reset
    // path.
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        compact: Compact,
    }

    for args in [
        &["compact", "--reset", "--reasoning"][..],
        &["compact", "--reset", "-k", "s:..-3"][..],
        &["compact", "--reset", "--keep-last", "5"][..],
    ] {
        assert!(
            TestCli::try_parse_from(args.iter().copied()).is_err(),
            "--reset must conflict with selection flags: {args:?}"
        );
    }

    // `--reset --dry-run` stays valid: it previews the removal.
    assert!(TestCli::try_parse_from(["compact", "--reset", "--dry-run"]).is_ok());
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().unwrap()
}

/// Each `ToolCallsMode` from the config maps to the right `ToolCallPolicy` on
/// the produced `Compaction` event (the `jp_config` -\> `jp_conversation`
/// bridge that lives in `build_mechanical_compaction`).
#[test]
fn tool_calls_mode_maps_to_policy() {
    // A few empty turns; `keep 0/0` makes the range cover all of them.
    let mut stream = ConversationStream::new_test();
    for t in 0..4 {
        stream.start_turn(format!("turn {t}"));
    }

    let cfg = AppConfig::new_test();
    let rt = runtime();

    let cases = [
        (ToolCallsMode::Strip, ToolCallPolicy::Strip {
            request: true,
            response: true,
        }),
        (ToolCallsMode::StripRequests, ToolCallPolicy::Strip {
            request: true,
            response: false,
        }),
        (ToolCallsMode::StripResponses, ToolCallPolicy::Strip {
            request: false,
            response: true,
        }),
        (ToolCallsMode::Omit, ToolCallPolicy::Omit),
    ];

    for (mode, expected) in cases {
        let rule = CompactionRuleConfig {
            keep_first: RuleBound::Turns(0),
            keep_last: RuleBound::Turns(0),
            reasoning: None,
            tool_calls: Some(mode),
            summary: None,
        };
        let compactions = rt
            .block_on(build_compaction_events(
                &stream,
                &cfg,
                std::slice::from_ref(&rule),
                Bound::Default,
                Bound::Default,
                Some(&Printer::sink()),
            ))
            .unwrap();
        assert_eq!(compactions.len(), 1, "non-empty range, mode {mode:?}");
        assert_eq!(compactions[0].tool_calls, Some(expected), "mode {mode:?}");
    }
}

#[test]
fn keep_last_duration_covering_whole_conversation_compacts_nothing() {
    // All turns are recent, so `keep_last = "30d"` covers the entire
    // conversation — it must preserve everything rather than fall back to the
    // default and compact through the end.
    let mut stream = ConversationStream::new_test();
    for t in 0..4 {
        stream.start_turn(format!("turn {t}"));
    }
    let cfg = AppConfig::new_test();
    let rule = CompactionRuleConfig {
        keep_first: RuleBound::Turns(0),
        keep_last: RuleBound::Duration(Duration::from_hours(720)),
        reasoning: None,
        tool_calls: Some(ToolCallsMode::Strip),
        summary: None,
    };
    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            std::slice::from_ref(&rule),
            Bound::Default,
            Bound::Default,
            Some(&Printer::sink()),
        ))
        .unwrap();
    assert!(
        compactions.is_empty(),
        "keep_last covering the whole conversation must compact nothing"
    );
}

#[test]
fn from_last_resolves_against_original_stream_for_every_rule() {
    // `--from last` (AfterLastCompaction) must resolve against the compactions
    // present at invocation start for *every* rule, not against a compaction
    // generated by an earlier rule in the same invocation. With two mechanical
    // rules and no pre-existing compaction, both resolve from turn 0.
    let mut stream = ConversationStream::new_test();
    for t in 0..6 {
        stream.start_turn(format!("turn {t}"));
    }
    let cfg = AppConfig::new_test();
    let rules = vec![
        CompactionRuleConfig {
            keep_first: RuleBound::Turns(0),
            keep_last: RuleBound::Turns(3),
            reasoning: Some(ReasoningMode::Strip),
            tool_calls: None,
            summary: None,
        },
        CompactionRuleConfig {
            keep_first: RuleBound::Turns(0),
            keep_last: RuleBound::Turns(3),
            reasoning: None,
            tool_calls: Some(ToolCallsMode::Strip),
            summary: None,
        },
    ];

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            Bound::At(RangeBound::AfterLastCompaction),
            Bound::Default,
            Some(&Printer::sink()),
        ))
        .unwrap();

    // Both rules apply; each resolves 0..=2 (keep_last = 3 over 6 turns). The
    // old single-`working` code let rule 1's generated compaction shift rule 2's
    // `last` baseline to turn 3, inverting its range and dropping it.
    assert_eq!(
        compactions.len(),
        2,
        "both rules must resolve against the original stream"
    );
    for c in &compactions {
        assert_eq!((c.from_turn, c.to_turn), (0, 2));
    }
}

/// End-to-end: a resolved config rule flows through `build_compaction_events`
/// into a `Compaction` event with the right range and policy, and projecting
/// the stream applies it — blanking request args in-range while keeping
/// responses and leaving out-of-range turns untouched.
#[test]
fn config_rule_strip_requests_blanks_args_through_projection() {
    // 6-turn stream, each turn carrying one tool call with arguments.
    let mut stream = ConversationStream::new_test();
    for t in 0..6 {
        stream.start_turn(format!("turn {t}"));
        stream
            .current_turn_mut()
            .add_tool_call_request(ToolCallRequest {
                id: format!("t{t}"),
                name: "tool".into(),
                arguments: Map::from_iter([("k".into(), Value::from("v"))]),
            })
            .add_tool_call_response(ToolCallResponse {
                id: format!("t{t}"),
                result: Ok("ok".into()),
            })
            .build()
            .unwrap();
    }

    // Resolved config rule: strip requests, keep first 1 and last 1.
    let cfg = AppConfig::new_test();
    let rules = vec![CompactionRuleConfig {
        keep_first: RuleBound::Turns(1),
        keep_last: RuleBound::Turns(1),
        reasoning: None,
        tool_calls: Some(ToolCallsMode::StripRequests),
        summary: None,
    }];

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            Bound::Default,
            Bound::Default,
            Some(&Printer::sink()),
        ))
        .unwrap();

    // One rule -> one compaction. keep_first=1/keep_last=1 over 6 turns -> 1..=4,
    // and `strip-requests` maps to `Strip { request: true, response: false }`.
    assert_eq!(compactions.len(), 1);
    assert_eq!((compactions[0].from_turn, compactions[0].to_turn), (1, 4));
    assert_eq!(
        compactions[0].tool_calls,
        Some(ToolCallPolicy::Strip {
            request: true,
            response: false,
        })
    );

    for compaction in compactions {
        stream.add_compaction(compaction);
    }
    stream.apply_projection();

    // Turns 1..=4: request args blanked, responses preserved. Turns 0 and 5
    // are out of range and untouched.
    for t in 0..6 {
        let req = stream
            .iter()
            .filter_map(|e| e.event.as_tool_call_request())
            .find(|r| r.id == format!("t{t}"))
            .expect("request present");

        if (1..=4).contains(&t) {
            assert!(req.arguments.is_empty(), "turn {t} args should be blanked");
            let resp = stream.find_tool_call_response(&format!("t{t}")).unwrap();
            assert_eq!(resp.content(), "ok", "turn {t} response preserved");
        } else {
            assert!(!req.arguments.is_empty(), "turn {t} args untouched");
        }
    }
}

#[test]
fn summarize_flag_distinguishes_absent_bare_and_valued() {
    // The three states the `Option<Option<String>>` encoding exists to separate.
    assert_eq!(parse_compact(&[]).summarize, None);
    assert_eq!(parse_compact(&["--summarize"]).summarize, Some(None));
    assert_eq!(parse_compact(&["-s"]).summarize, Some(None));
    assert_eq!(
        parse_compact(&["-s", "focus on the architectural design"]).summarize,
        Some(Some("focus on the architectural design".to_owned())),
    );
}

#[test]
fn timeline_keeps_genesis_and_trailing_turns() {
    // The default `-s` case from a 9-turn conversation (indices 0..=8):
    // keep_first=1 and keep_last=1 leave turn 0 and turn 8, compacting 1..=7.
    let segments = vec![TimelineSegment {
        from: 1,
        to: 7,
        label: None,
    }];
    let lines = timeline_lines(&segments, 8, false);
    assert_eq!(lines, vec![
        "Kept turn 0.".to_owned(),
        "Compacted turns 1..=7 (7 total).".to_owned(),
        "Kept turn 8.".to_owned(),
    ]);
}

#[test]
fn timeline_interleaves_gaps_between_compactions() {
    // Two non-contiguous compactions leave an interior gap and a trailing gap.
    let segments = vec![
        TimelineSegment {
            from: 1,
            to: 3,
            label: None,
        },
        TimelineSegment {
            from: 6,
            to: 8,
            label: None,
        },
    ];
    let lines = timeline_lines(&segments, 10, false);
    assert_eq!(lines, vec![
        "Kept turn 0.".to_owned(),
        "Compacted turns 1..=3 (3 total).".to_owned(),
        "Kept turns 4..=5.".to_owned(),
        "Compacted turns 6..=8 (3 total).".to_owned(),
        "Kept turns 9..=10.".to_owned(),
    ]);
}

#[test]
fn timeline_sorts_by_start_turn_regardless_of_generation_order() {
    // Rules can emit ranges out of turn order; the timeline still reads in
    // conversation order.
    let segments = vec![
        TimelineSegment {
            from: 6,
            to: 8,
            label: None,
        },
        TimelineSegment {
            from: 1,
            to: 3,
            label: None,
        },
    ];
    let lines = timeline_lines(&segments, 8, false);
    assert_eq!(lines, vec![
        "Kept turn 0.".to_owned(),
        "Compacted turns 1..=3 (3 total).".to_owned(),
        "Kept turns 4..=5.".to_owned(),
        "Compacted turns 6..=8 (3 total).".to_owned(),
    ]);
}

#[test]
fn timeline_collapses_overlapping_ranges() {
    // Overlapping ranges must not produce a spurious or negative gap between
    // them; the gap is only printed where no compaction covers a turn.
    let segments = vec![
        TimelineSegment {
            from: 1,
            to: 5,
            label: None,
        },
        TimelineSegment {
            from: 3,
            to: 8,
            label: None,
        },
    ];
    let lines = timeline_lines(&segments, 10, false);
    assert_eq!(lines, vec![
        "Kept turn 0.".to_owned(),
        "Compacted turns 1..=5 (5 total).".to_owned(),
        "Compacted turns 3..=8 (6 total).".to_owned(),
        "Kept turns 9..=10.".to_owned(),
    ]);
}

#[test]
fn timeline_labels_describe_compaction_type() {
    let segments = vec![TimelineSegment {
        from: 1,
        to: 3,
        label: Some("reasoning + tools".to_owned()),
    }];
    let lines = timeline_lines(&segments, 4, false);
    assert_eq!(lines, vec![
        "Kept turn 0.".to_owned(),
        "Compacted turns 1..=3 (3 total, reasoning + tools).".to_owned(),
        "Kept turn 4.".to_owned(),
    ]);
}

#[test]
fn timeline_dry_run_uses_conditional_verbs() {
    let segments = vec![TimelineSegment {
        from: 1,
        to: 3,
        label: None,
    }];
    let lines = timeline_lines(&segments, 4, true);
    assert_eq!(lines, vec![
        "Would have kept turn 0.".to_owned(),
        "Would have compacted turns 1..=3 (3 total).".to_owned(),
        "Would have kept turn 4.".to_owned(),
    ]);
}

#[test]
fn segment_label_reflects_mechanical_policies() {
    // The label distinguishes the kind of compaction; here, reasoning stripping
    // combined with full tool-call stripping.
    let compaction = Compaction::new(1, 3)
        .with_reasoning(ReasoningPolicy::Strip)
        .with_tool_calls(ToolCallPolicy::Strip {
            request: true,
            response: true,
        });
    let segments = segments_for_compactions(std::slice::from_ref(&compaction), "test-conv");
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].label.as_deref(), Some("reasoning + tools"));
}
