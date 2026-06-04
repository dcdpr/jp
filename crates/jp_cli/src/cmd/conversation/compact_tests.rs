use std::time::Duration;

use clap::Parser as _;
use jp_config::{
    AppConfig, PartialAppConfig,
    conversation::compaction::{CompactionRuleConfig, RuleBound, ToolCallsMode},
};
use jp_conversation::{
    ConversationStream, ToolCallPolicy,
    event::{ToolCallRequest, ToolCallResponse},
};
use jp_printer::Printer;
use serde_json::{Map, Value};

use super::{Bound, Compact, build_compaction_event, build_compaction_events_from_config};
use crate::ctx::IntoPartialAppConfig as _;

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
    // Range-only flags must narrow the configured rules at runtime, not replace
    // them with a policy-less rule (which would project to a no-op).
    let compact = parse_compact(&["--keep-last", "5"]);
    let partial = compact
        .apply_cli_config(None, PartialAppConfig::default(), None)
        .unwrap();
    assert!(
        partial.conversation.compaction.rules.is_empty(),
        "range-only flags must leave the rules array untouched"
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
        let compaction = rt
            .block_on(build_compaction_event(
                &stream,
                &cfg,
                &rule,
                Bound::Default,
                Bound::Default,
                &Printer::sink(),
            ))
            .unwrap()
            .expect("non-empty range");
        assert_eq!(compaction.tool_calls, Some(expected), "mode {mode:?}");
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
    let result = runtime()
        .block_on(build_compaction_event(
            &stream,
            &cfg,
            &rule,
            Bound::Default,
            Bound::Default,
            &Printer::sink(),
        ))
        .unwrap();
    assert!(
        result.is_none(),
        "keep_last covering the whole conversation must compact nothing"
    );
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
    let mut cfg = AppConfig::new_test();
    cfg.conversation.compaction.rules = vec![CompactionRuleConfig {
        keep_first: RuleBound::Turns(1),
        keep_last: RuleBound::Turns(1),
        reasoning: None,
        tool_calls: Some(ToolCallsMode::StripRequests),
        summary: None,
    }];

    let compactions = runtime()
        .block_on(build_compaction_events_from_config(
            &stream,
            &cfg,
            Bound::Default,
            Bound::Default,
            &Printer::sink(),
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
