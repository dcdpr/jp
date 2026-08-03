use std::time::Duration;

use clap::Parser as _;
use jp_config::{
    AppConfig, PartialAppConfig,
    conversation::compaction::{
        CompactionConfig, CompactionRuleConfig, PartialCompactionRuleConfig, PartialSummaryConfig,
        ReasoningMode, RuleBound, SummaryConfig, ToolCallsMode,
    },
    model::{PartialModelConfig, id::PartialModelIdOrAliasConfig},
};
use jp_conversation::{
    Compaction, ConversationStream, RangeBound, ReasoningPolicy, SummaryPolicy, SummarySource,
    ToolCallPolicy,
    event::{ToolCallRequest, ToolCallResponse},
};
use jp_printer::Printer;
use serde_json::{Map, Value};

use super::{
    Bound, Compact, IntoPartialAppConfig as _, TimelineSegment, build_compaction_events,
    existing_segments, resolve_reset_index, segments_for_compactions, timeline_lines,
};
use crate::cmd::{conversation_id::ConversationIds as _, target::ConversationTarget};

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
fn model_flag_targets_the_assistant_model() {
    // `--model` rides the same `assistant.model.id` path as `jp query --model`,
    // so the pipeline resolves the alias. The summarizer picks it up through its
    // fallback: an unset `summary.model` means "use the assistant model".
    let compact = parse_compact(&["--summary", "--model", "gpt"]);
    let mut partial = PartialAppConfig::new_test();
    partial = compact.apply_cli_config(None, partial, None).unwrap();

    assert_eq!(
        partial.assistant.model.id,
        PartialModelIdOrAliasConfig::Alias("gpt".to_owned())
    );
}

#[test]
fn model_alias_reaches_a_configured_summary_model_through_the_pipeline() {
    // The whole path in one go: `--model gpt` -> `apply_cli_config` -> the
    // config pipeline (which resolves the alias) -> `effective_rules`. Asserting
    // the exact requested model catches a broken CLI-config hop, a missed alias
    // resolution, or a missed redirect, none of which the narrower tests below
    // can distinguish on their own.
    let mut partial = PartialAppConfig::new_test();
    partial.providers.llm.aliases.insert(
        "gpt".to_owned(),
        PartialModelIdOrAliasConfig::from("openai/gpt-5"),
    );
    partial.conversation.compaction.rules = vec![PartialCompactionRuleConfig {
        summary: Some(PartialSummaryConfig {
            model: Some(PartialModelConfig {
                id: "anthropic/claude-configured".into(),
                ..PartialModelConfig::default()
            }),
            ..PartialSummaryConfig::default()
        }),
        ..PartialCompactionRuleConfig::default()
    }]
    .into();

    // No `--summary`: a policy flag would replace the configured rule with an
    // ad-hoc one, and the configured `summary.model` is what this exercises.
    let compact = parse_compact(&["--model", "gpt"]);
    let partial = compact.apply_cli_config(None, partial, None).unwrap();
    let cfg = jp_config::util::build(partial).unwrap();

    let rules = compact.effective_rules(&cfg).unwrap();
    let summary = rules[0].summary.as_ref().expect("configured summary rule");

    assert_eq!(
        summary.model.as_ref().unwrap().id.resolved().to_string(),
        "openai/gpt-5"
    );
}

#[test]
fn model_flag_overrides_a_configured_summary_model() {
    // A rule with its own `summary.model` outranks the assistant model in the
    // summarizer, which would make `--model` a silent no-op.
    let mut cfg = AppConfig::new_test();
    cfg.conversation.compaction.rules =
        CompactionConfig::finalize_rules(vec![PartialCompactionRuleConfig {
            summary: Some(PartialSummaryConfig {
                model: Some(PartialModelConfig {
                    id: "anthropic/claude-configured".into(),
                    ..PartialModelConfig::default()
                }),
                instructions: Some("keep it short".to_owned()),
                ..PartialSummaryConfig::default()
            }),
            ..PartialCompactionRuleConfig::default()
        }])
        .unwrap();

    let compact = parse_compact(&["--model", "openai/gpt-5"]);
    let rules = compact.effective_rules(&cfg).unwrap();

    let summary = rules[0].summary.as_ref().expect("configured summary rule");
    assert_eq!(
        summary.model.as_ref().unwrap().id,
        cfg.assistant.model.id,
        "a configured summary model must be redirected to the assistant model"
    );
    // Only the model ID moves; the rest of the summary config survives.
    assert_eq!(summary.instructions.as_deref(), Some("keep it short"));
}

#[test]
fn without_the_model_flag_a_configured_summary_model_is_kept() {
    let mut cfg = AppConfig::new_test();
    cfg.conversation.compaction.rules =
        CompactionConfig::finalize_rules(vec![PartialCompactionRuleConfig {
            summary: Some(PartialSummaryConfig {
                model: Some(PartialModelConfig {
                    id: "anthropic/claude-configured".into(),
                    ..PartialModelConfig::default()
                }),
                ..PartialSummaryConfig::default()
            }),
            ..PartialCompactionRuleConfig::default()
        }])
        .unwrap();

    let compact = parse_compact(&[]);
    let rules = compact.effective_rules(&cfg).unwrap();

    assert_eq!(
        rules[0]
            .summary
            .as_ref()
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .id
            .to_string(),
        "anthropic/claude-configured"
    );
}

#[test]
fn model_flag_leaves_summaryless_rules_untouched() {
    let compact = parse_compact(&["--reasoning", "--model", "openai/gpt-5"]);
    let cfg = AppConfig::new_test();
    let rules = compact.effective_rules(&cfg).unwrap();

    assert!(
        rules[0].summary.is_none(),
        "--model must not turn a mechanical rule into a summary rule"
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

#[test]
fn reset_takes_an_optional_compaction_index() {
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        compact: Compact,
    }

    assert_eq!(parse_compact(&["--reset"]).reset, Some(None));
    assert_eq!(parse_compact(&["--reset=2"]).reset, Some(Some(2)));
    assert_eq!(parse_compact(&[]).reset, None);

    // The index requires `=`, so a bare `--reset` followed by a conversation
    // target still targets the conversation instead of swallowing it as the
    // index.
    let compact = parse_compact(&["--reset", "latest"]);
    assert_eq!(compact.reset, Some(None));
    assert_eq!(compact.target.ids(), [ConversationTarget::Latest]);

    // Indices are 1-based, so `0` names nothing.
    assert!(TestCli::try_parse_from(["compact", "--reset=0"]).is_err());
    assert!(TestCli::try_parse_from(["compact", "--reset=x"]).is_err());
}

#[test]
fn reset_index_addresses_compactions_in_stream_order() {
    let mut stream = ConversationStream::new_test();
    for t in 0..6 {
        stream.start_turn(format!("turn {t}"));
    }
    stream.add_compaction(Compaction::new(0, 1));
    stream.add_compaction(Compaction::new(2, 4));

    // The label carries 1-based turn numbers, matching `jp conversation show`.
    assert_eq!(
        resolve_reset_index(&stream, 1),
        Ok((0, "turns 1..2".to_owned()))
    );
    assert_eq!(
        resolve_reset_index(&stream, 2),
        Ok((1, "turns 3..5".to_owned()))
    );
    assert_eq!(
        resolve_reset_index(&stream, 3),
        Err("compaction 3 out of range (conversation has 2 compaction event(s))".to_owned())
    );
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().unwrap()
}

/// A rule that summarizes the whole conversation with `text`, if given.
fn summary_rule(text: Option<&str>) -> CompactionRuleConfig {
    CompactionRuleConfig {
        keep_first: RuleBound::Turns(0),
        keep_last: RuleBound::Turns(0),
        reasoning: None,
        tool_calls: None,
        summary: Some(SummaryConfig {
            text: text.map(ToOwned::to_owned),
            model: None,
            instructions: None,
            context: None,
        }),
    }
}

fn stream_of(turns: usize) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    for t in 0..turns {
        stream.start_turn(format!("turn {t}"));
    }
    stream
}

#[test]
fn verbatim_summary_is_stored_as_authored_text() {
    let stream = stream_of(4);
    let cfg = AppConfig::new_test();

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &[summary_rule(Some("we settled on the layered loader"))],
            Bound::Default,
            Bound::Default,
            Some(&Printer::sink()),
        ))
        .unwrap();

    // `Authored` is reachable only through the branch that skips the
    // summarizer, so this pins the no-model path rather than just the text.
    assert_eq!(compactions.len(), 1);
    assert_eq!(
        compactions[0].summary,
        Some(SummaryPolicy::authored("we settled on the layered loader"))
    );
    assert_eq!(
        compactions[0].summary.as_ref().unwrap().source,
        SummarySource::Authored
    );
}

#[test]
fn blank_verbatim_summary_is_rejected() {
    let stream = stream_of(4);
    let cfg = AppConfig::new_test();

    let error = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &[summary_rule(Some("   "))],
            Bound::Default,
            Bound::Default,
            Some(&Printer::sink()),
        ))
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Compaction error: the summary text is empty; drop the value to generate a summary instead"
    );
}

#[test]
fn verbatim_summary_refuses_to_widen_over_an_existing_summary() {
    let mut stream = stream_of(6);
    // Raw turns 3..5 are already summarized, so a verbatim summary of 0..3
    // would have to grow to 0..5 and stand in for turns it never described.
    stream.add_compaction(Compaction::new(3, 5).with_summary(SummaryPolicy::generated("earlier")));

    let mut rule = summary_rule(Some("hand-written"));
    rule.keep_last = RuleBound::FromEnd(2);

    let cfg = AppConfig::new_test();
    let error = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &[rule],
            Bound::Default,
            Bound::Default,
            Some(&Printer::sink()),
        ))
        .unwrap_err();

    // Turn numbers are reported 1-based, matching `--from`/`--to`.
    let crate::error::Error::SummaryOverlap {
        authored,
        from,
        to,
        required_from,
        required_to,
    } = error
    else {
        panic!("expected a summary overlap, got: {error}");
    };
    assert!(authored, "the new summary is the verbatim one");
    assert_eq!((from, to, required_from, required_to), (1, 4, 1, 6));
}

#[test]
fn generated_summary_refuses_to_widen_over_verbatim_text() {
    let mut stream = stream_of(6);
    stream.add_compaction(Compaction::new(3, 5).with_summary(SummaryPolicy::authored("mine")));

    let mut rule = summary_rule(None);
    rule.keep_last = RuleBound::FromEnd(2);

    let cfg = AppConfig::new_test();
    let error = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &[rule],
            Bound::Default,
            Bound::Default,
            Some(&Printer::sink()),
        ))
        .unwrap_err();

    // The refusal happens during range resolution, before any provider lookup,
    // so the hand-written text survives.
    let crate::error::Error::SummaryOverlap {
        authored,
        from,
        to,
        required_from,
        required_to,
    } = error
    else {
        panic!("expected a summary overlap, got: {error}");
    };
    assert!(!authored, "the blocking text is the existing summary");
    assert_eq!((from, to, required_from, required_to), (1, 4, 1, 6));
}

#[test]
fn verbatim_summary_covering_an_existing_summary_is_accepted() {
    let mut stream = stream_of(6);
    stream.add_compaction(Compaction::new(3, 5).with_summary(SummaryPolicy::generated("earlier")));

    // The rule covers every turn, so nothing has to grow.
    let cfg = AppConfig::new_test();
    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &[summary_rule(Some("covers everything"))],
            Bound::Default,
            Bound::Default,
            Some(&Printer::sink()),
        ))
        .unwrap();

    assert_eq!(compactions.len(), 1);
    assert_eq!(compactions[0].from_turn, 0);
    assert_eq!(compactions[0].to_turn, 5);
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
    // `--from last-compaction` (AfterLastCompaction) must resolve against the compactions
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
fn keep_first_composes_with_first() {
    // `--keep-first M --first N` compacts the first N turns minus the
    // preserved prefix: `--keep-first 1 --first 16` compacts turns 2..16
    // (indices 1..=15).
    let mut stream = ConversationStream::new_test();
    for t in 0..20 {
        stream.start_turn(format!("turn {t}"));
    }

    let compact = parse_compact(&["--keep-first", "1", "--first", "16", "-r"]);
    let cfg = AppConfig::new_test();
    let rules = compact.effective_rules(&cfg).unwrap();
    let from = compact.resolve_from(&stream);
    let to = compact.resolve_to(&stream);

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            from,
            to,
            Some(&Printer::sink()),
        ))
        .unwrap();

    assert_eq!(compactions.len(), 1);
    assert_eq!((compactions[0].from_turn, compactions[0].to_turn), (1, 15));
}

#[test]
fn keep_last_composes_with_last() {
    // `--keep-last M --last N` compacts the last N turns minus the preserved
    // suffix: over 20 turns, `--keep-last 2 --last 16` compacts turns 5..18
    // (indices 4..=17), leaving the final 2 untouched.
    let mut stream = ConversationStream::new_test();
    for t in 0..20 {
        stream.start_turn(format!("turn {t}"));
    }

    let compact = parse_compact(&["--keep-last", "2", "--last", "16", "-r"]);
    let cfg = AppConfig::new_test();
    let rules = compact.effective_rules(&cfg).unwrap();
    let from = compact.resolve_from(&stream);
    let to = compact.resolve_to(&stream);

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            from,
            to,
            Some(&Printer::sink()),
        ))
        .unwrap();

    assert_eq!(compactions.len(), 1);
    assert_eq!((compactions[0].from_turn, compactions[0].to_turn), (4, 17));
}

#[test]
fn keep_first_greater_than_first_is_rejected() {
    // A preserved prefix larger than the selection is nonsensical and must be
    // an error rather than a silent no-op.
    let err = parse_compact(&["--keep-first", "17", "--first", "16"])
        .validate()
        .unwrap_err();
    assert_eq!(
        err,
        "--keep-first 17 is greater than --first 16: nothing would remain to compact"
    );

    // Equal values select nothing, which is empty but not nonsensical.
    assert!(
        parse_compact(&["--keep-first", "16", "--first", "16"])
            .validate()
            .is_ok()
    );
}

#[test]
fn keep_last_greater_than_last_is_rejected() {
    // A preserved suffix larger than the selection is nonsensical and must be
    // an error rather than a silent no-op.
    let err = parse_compact(&["--keep-last", "17", "--last", "16"])
        .validate()
        .unwrap_err();
    assert_eq!(
        err,
        "--keep-last 17 is greater than --last 16: nothing would remain to compact"
    );

    // Equal values select nothing, which is empty but not nonsensical.
    assert!(
        parse_compact(&["--keep-last", "16", "--last", "16"])
            .validate()
            .is_ok()
    );
}

#[test]
fn summary_flag_distinguishes_absent_bare_and_valued() {
    // The three states the `Option<Option<String>>` encoding exists to separate:
    // no summary, generate one, and use this exact text.
    assert_eq!(parse_compact(&[]).summary, None);
    assert_eq!(parse_compact(&["--summary"]).summary, Some(None));
    assert_eq!(parse_compact(&["-s"]).summary, Some(None));
    assert_eq!(
        parse_compact(&["-s", "we settled on the layered loader"]).summary,
        Some(Some("we settled on the layered loader".to_owned())),
    );
}

#[test]
fn valued_summary_flag_becomes_verbatim_text_not_summarizer_context() {
    let compact = parse_compact(&["--summary", "the gist of it"]);
    let cfg = AppConfig::new_test();

    let rules = compact.effective_rules(&cfg).unwrap();
    let summary = rules[0].summary.as_ref().expect("summary rule");

    assert_eq!(summary.text.as_deref(), Some("the gist of it"));
    assert_eq!(summary.context, None);
}

#[test]
fn summary_context_flag_applies_to_configured_rules() {
    // `--summary-context` modifies whichever rules are active, the same way
    // `--model` does, instead of replacing them with an ad-hoc rule.
    let mut cfg = AppConfig::new_test();
    cfg.conversation.compaction.rules =
        CompactionConfig::finalize_rules(vec![PartialCompactionRuleConfig {
            summary: Some(PartialSummaryConfig {
                context: Some("configured context".to_owned()),
                ..PartialSummaryConfig::default()
            }),
            ..PartialCompactionRuleConfig::default()
        }])
        .unwrap();

    let compact = parse_compact(&["--summary-context", "focus on the architecture"]);
    let rules = compact.effective_rules(&cfg).unwrap();

    assert_eq!(rules.len(), 1, "the configured rule must survive");
    assert_eq!(
        rules[0].summary.as_ref().unwrap().context.as_deref(),
        Some("focus on the architecture")
    );
}

#[test]
fn summary_context_does_not_add_a_rule_of_its_own() {
    // Without a summary rule to modify there is nothing to summarize, so the
    // flag must not synthesize one.
    let compact = parse_compact(&["--summary-context", "focus on the architecture"]);
    let cfg = AppConfig::new_test();

    let rules = compact.effective_rules(&cfg).unwrap();

    assert_eq!(rules, cfg.conversation.compaction.rules);
}

#[test]
fn turn_out_of_range_is_rejected() {
    // `--turn` names specific turns, so an endpoint past the conversation is an
    // error rather than an empty (`--turn 100`) or clamped (`--turn ..100`)
    // range. With 5 turns, both forms flag turn 100; an in-range turn does not.
    assert_eq!(
        parse_compact(&["--turn", "100"]).range.turn_out_of_range(5),
        Some(100)
    );
    assert_eq!(
        parse_compact(&["--turn", "..100"])
            .range
            .turn_out_of_range(5),
        Some(100)
    );
    assert_eq!(
        parse_compact(&["--turn", "3"]).range.turn_out_of_range(5),
        None
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
        existing: false,
    }];
    let lines = timeline_lines(&segments, 8, false);
    assert_eq!(lines, vec![
        "Kept turn 1.".to_owned(),
        "Compacted turns 2..8 (7 total).".to_owned(),
        "Kept turn 9.".to_owned(),
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
            existing: false,
        },
        TimelineSegment {
            from: 6,
            to: 8,
            label: None,
            existing: false,
        },
    ];
    let lines = timeline_lines(&segments, 10, false);
    assert_eq!(lines, vec![
        "Kept turn 1.".to_owned(),
        "Compacted turns 2..4 (3 total).".to_owned(),
        "Kept turns 5..6.".to_owned(),
        "Compacted turns 7..9 (3 total).".to_owned(),
        "Kept turns 10..11.".to_owned(),
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
            existing: false,
        },
        TimelineSegment {
            from: 1,
            to: 3,
            label: None,
            existing: false,
        },
    ];
    let lines = timeline_lines(&segments, 8, false);
    assert_eq!(lines, vec![
        "Kept turn 1.".to_owned(),
        "Compacted turns 2..4 (3 total).".to_owned(),
        "Kept turns 5..6.".to_owned(),
        "Compacted turns 7..9 (3 total).".to_owned(),
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
            existing: false,
        },
        TimelineSegment {
            from: 3,
            to: 8,
            label: None,
            existing: false,
        },
    ];
    let lines = timeline_lines(&segments, 10, false);
    assert_eq!(lines, vec![
        "Kept turn 1.".to_owned(),
        "Compacted turns 2..6 (5 total).".to_owned(),
        "Compacted turns 4..9 (6 total).".to_owned(),
        "Kept turns 10..11.".to_owned(),
    ]);
}

#[test]
fn timeline_labels_describe_compaction_type() {
    let segments = vec![TimelineSegment {
        from: 1,
        to: 3,
        label: Some("reasoning + tools".to_owned()),
        existing: false,
    }];
    let lines = timeline_lines(&segments, 4, false);
    assert_eq!(lines, vec![
        "Kept turn 1.".to_owned(),
        "Compacted turns 2..4 (3 total, reasoning + tools).".to_owned(),
        "Kept turn 5.".to_owned(),
    ]);
}

#[test]
fn timeline_dry_run_uses_conditional_verbs() {
    let segments = vec![TimelineSegment {
        from: 1,
        to: 3,
        label: None,
        existing: false,
    }];
    let lines = timeline_lines(&segments, 4, true);
    assert_eq!(lines, vec![
        "Would have kept turn 1.".to_owned(),
        "Would have compacted turns 2..4 (3 total).".to_owned(),
        "Would have kept turn 5.".to_owned(),
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

#[test]
fn timeline_reports_pre_existing_compactions_not_as_kept() {
    // Regression: a prior run compacted turns 1..5; a new run (e.g. `--from
    // last`) compacts 6..8. The already-compacted range must read as compacted,
    // not kept, since the projected conversation still compacts it.
    let mut snapshot = ConversationStream::new_test();
    for i in 0..10 {
        snapshot.start_turn(format!("turn {i}"));
    }
    snapshot.add_compaction(Compaction::new(1, 5));

    let mut segments = existing_segments(&snapshot);
    segments.push(TimelineSegment {
        from: 6,
        to: 8,
        label: None,
        existing: false,
    });

    let lines = timeline_lines(&segments, 9, false);
    assert_eq!(lines, vec![
        "Kept turn 1.".to_owned(),
        "Compacted turns 2..6 (5 total, already compacted).".to_owned(),
        "Compacted turns 7..9 (3 total).".to_owned(),
        "Kept turn 10.".to_owned(),
    ]);
}

#[test]
fn timeline_dry_run_keeps_pre_existing_compactions_factual() {
    // Under `--dry-run`, the new range is hypothetical ("Would have compacted"),
    // but a pre-existing compaction is a fact that predates this run, so it stays
    // "Compacted".
    let mut snapshot = ConversationStream::new_test();
    for i in 0..10 {
        snapshot.start_turn(format!("turn {i}"));
    }
    snapshot.add_compaction(Compaction::new(1, 5));

    let mut segments = existing_segments(&snapshot);
    segments.push(TimelineSegment {
        from: 6,
        to: 8,
        label: None,
        existing: false,
    });

    let lines = timeline_lines(&segments, 9, true);
    assert_eq!(lines, vec![
        "Would have kept turn 1.".to_owned(),
        "Compacted turns 2..6 (5 total, already compacted).".to_owned(),
        "Would have compacted turns 7..9 (3 total).".to_owned(),
        "Would have kept turn 10.".to_owned(),
    ]);
}
