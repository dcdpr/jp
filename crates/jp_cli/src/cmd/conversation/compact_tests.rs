use std::time::Duration;

use camino_tempfile::{Utf8TempDir, tempdir};
use clap::Parser as _;
use jp_config::{
    AppConfig, PartialAppConfig,
    conversation::compaction::{
        CompactionConfig, CompactionRuleConfig, PartialCompactionRuleConfig, PartialSummaryConfig,
        ReasoningMode, RuleBound, ToolCallsMode,
    },
    model::{PartialModelConfig, id::PartialModelIdOrAliasConfig},
};
use jp_conversation::{
    ByteSize, Compaction, ConversationStream, PolicySpec, ReasoningPolicy, SummaryPolicy,
    SummarySource, ToolCallPolicy,
    event::{ToolCallRequest, ToolCallResponse},
};
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_workspace::Workspace;
use serde_json::{Map, Value};
use tokio::runtime::Runtime;

use super::{
    Compact, IntoPartialAppConfig as _, TimelineSegment, TurnSelection, build_compaction_events,
    existing_segments, plan_rule_ranges, resolve_reset_index, segments_for_compactions,
    timeline_lines,
};
use crate::{
    Globals,
    cmd::{conversation_id::ConversationIds as _, target::ConversationTarget},
    ctx::Ctx,
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

/// A rule that summarizes the whole selection, with both keep bounds open so
/// the rule's own bounds never narrow the window under test.
///
/// `text` supplies a verbatim summary; `None` leaves the rule to generate one.
fn summary_rule(text: Option<&str>) -> CompactionRuleConfig {
    CompactionConfig::finalize_rules(vec![PartialCompactionRuleConfig {
        keep_first: Some(RuleBound::Turns(0)),
        keep_last: Some(RuleBound::Turns(0)),
        summary: Some(PartialSummaryConfig {
            text: text.map(ToOwned::to_owned),
            ..PartialSummaryConfig::default()
        }),
        ..PartialCompactionRuleConfig::default()
    }])
    .unwrap()
    .remove(0)
}

#[test]
fn bare_compact_flag_parses_without_a_value() {
    // Bare `--compact` (no value) means "apply config rules".
    let compact = parse_compact(&["--compact"]);
    assert!(compact.compact_flag.use_config_rules);
    assert!(compact.compact_flag.specs.is_empty());
}

#[test]
fn over_flag_reaches_the_stored_policy() {
    // The threshold has to survive the whole path (flag -> ad-hoc rule ->
    // stored `Compaction`), because projection reads it from the event, not
    // from the invocation.
    let compact = parse_compact(&["--tools=sres", "--over", "1mb"]);
    let rules = compact.effective_rules(&AppConfig::new_test()).unwrap();

    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0].tool_calls,
        Some(PolicySpec::over(
            ToolCallsMode::StripResponses,
            ByteSize::from_bytes(1024 * 1024)
        ))
    );

    let compaction = super::build_mechanical_compaction(0, 0, &rules[0]);

    assert_eq!(
        compaction.tool_calls,
        Some(PolicySpec::over(
            ToolCallPolicy::Strip {
                request: false,
                response: true,
            },
            ByteSize::from_bytes(1024 * 1024)
        ))
    );
}

#[test]
fn over_flag_applies_to_every_mechanical_policy_it_sets() {
    let compact = parse_compact(&["--reasoning", "--tools=strip", "--over", "512kb"]);
    let rules = compact.effective_rules(&AppConfig::new_test()).unwrap();

    let over = Some(ByteSize::from_bytes(512 * 1024));
    assert_eq!(rules[0].reasoning.map(|spec| spec.over), Some(over));
    assert_eq!(rules[0].tool_calls.map(|spec| spec.over), Some(over));
}

#[test]
fn over_without_a_policy_is_rejected() {
    // Without a policy to narrow, the flag would silently do nothing.
    let compact = parse_compact(&["--over", "1mb"]);

    assert_eq!(
        compact.validate().unwrap_err(),
        "--over needs a policy to narrow: pass --reasoning and/or --tools"
    );
}

#[test]
fn over_conflicts_with_summarize() {
    // A summary replaces its whole range rather than acting per item, so it
    // ignores the mechanical policies a threshold would narrow.
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        compact: Compact,
    }

    assert!(TestCli::try_parse_from(["compact", "--summarize", "--over", "1mb"]).is_err());
}

#[test]
fn no_over_flag_leaves_the_policy_unnarrowed() {
    let compact = parse_compact(&["--tools=sres"]);
    let rules = compact.effective_rules(&AppConfig::new_test()).unwrap();

    assert_eq!(
        rules[0].tool_calls,
        Some(PolicySpec::new(ToolCallsMode::StripResponses))
    );
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
    let compact = parse_compact(&["--reset", "recent"]);
    assert_eq!(compact.reset, Some(None));
    assert_eq!(compact.target.ids(), [ConversationTarget::Recent]);

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
            &parse_compact(&[]).range,
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
            &parse_compact(&[]).range,
            Some(&Printer::sink()),
        ))
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Compaction error: the summary text is empty; drop the value to generate a summary instead"
    );
}

/// A `Ctx` backed by an in-memory printer, for exercising the dry-run preview.
///
/// The tempdir is returned so it outlives the ctx, whose workspace points into
/// it.
fn preview_ctx() -> (Ctx, SharedBuffer, Utf8TempDir) {
    let tmp = tempdir().unwrap();
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);

    let workspace = Workspace::in_memory(tmp.path());

    let ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    );

    (ctx, out, tmp)
}

#[test]
fn preview_rejects_a_blank_verbatim_summary() {
    // Regression: `--summary '' --dry-run` used to print a successful preview
    // while the real run rejected the same rule, so the preview promised a
    // compaction that could not be performed.
    let (ctx, _out, _tmp) = preview_ctx();
    let stream = stream_of(4);

    let error = Compact::preview_compaction(
        &ctx,
        &stream,
        &[summary_rule(Some("   "))],
        &parse_compact(&[]).range,
    )
    .unwrap_err();

    // `preview_compaction` yields the rendered command error, so this pins what
    // the user actually reads.
    assert_eq!(
        error.to_string(),
        "error 1: Compaction error (error:\"the summary text is empty; drop the value to generate \
         a summary instead\")"
    );
}

#[test]
fn preview_refuses_an_overlap_the_real_run_would_refuse() {
    // The preview shares `resolve_rule_range` with the real run, so the same
    // widening over an existing summary is refused before anything is printed.
    let (ctx, out, _tmp) = preview_ctx();
    let mut stream = stream_of(6);
    stream.add_compaction(Compaction::new(3, 5).with_summary(SummaryPolicy::generated("earlier")));

    let mut rule = summary_rule(Some("hand-written"));
    rule.keep_last = RuleBound::FromEnd(2);

    let error =
        Compact::preview_compaction(&ctx, &stream, &[rule], &parse_compact(&[]).range).unwrap_err();

    ctx.printer.flush();
    // The full refusal as the user reads it: what went wrong, and the exact
    // range that resolves it.
    assert_eq!(
        error.to_string(),
        "error 1: Summary overlap (reason:\"A summary cannot be nested inside or split across \
         another one, so your text for turns 1..4 would have to stand in for turns 1..6 as \
         well.\", suggestion:\"Re-run with `--from 1 --to 6` to cover the whole range, or `jp \
         conversation compact --reset` to drop the existing compactions first.\")"
    );
    assert_eq!(
        out.lock().clone(),
        "",
        "a refused preview must print no timeline"
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
            &parse_compact(&[]).range,
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
            &parse_compact(&[]).range,
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
fn a_later_overlap_is_refused_before_any_summarizer_request() {
    // Regression: the first rule's summary used to be generated before the
    // second rule's range was resolved, so an overlap in the second rule threw
    // away a paid request nothing recorded.
    //
    // `AppConfig::new_test()` points the assistant at `anthropic/test`, so any
    // summarizer call fails on provider lookup. Getting `SummaryOverlap` back is
    // therefore proof that rule 1 never reached a provider.
    let mut stream = stream_of(10);
    stream.add_compaction(Compaction::new(6, 8).with_summary(SummaryPolicy::authored("mine")));

    // Rule 1 generates a summary for turns 0..2, disjoint from the authored one.
    let mut generated = summary_rule(None);
    generated.keep_last = RuleBound::FromEnd(7);

    // Rule 2 covers 4..7, so it has to grow over the authored summary.
    let mut conflicting = summary_rule(None);
    conflicting.keep_first = RuleBound::Absolute(5);
    conflicting.keep_last = RuleBound::FromEnd(2);

    let cfg = AppConfig::new_test();
    let error = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &[generated, conflicting],
            &parse_compact(&[]).range,
            Some(&Printer::sink()),
        ))
        .unwrap_err();

    let crate::error::Error::SummaryOverlap {
        from,
        to,
        required_from,
        required_to,
        ..
    } = error
    else {
        panic!("expected the overlap to be reported before summarizing, got: {error}");
    };
    assert_eq!((from, to, required_from, required_to), (5, 8, 5, 9));
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
            &parse_compact(&[]).range,
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
            tool_calls: Some(mode.into()),
            summary: None,
        };
        let compactions = rt
            .block_on(build_compaction_events(
                &stream,
                &cfg,
                std::slice::from_ref(&rule),
                &TurnSelection::default(),
                Some(&Printer::sink()),
            ))
            .unwrap();
        assert_eq!(compactions.len(), 1, "non-empty range, mode {mode:?}");
        assert_eq!(
            compactions[0].tool_calls,
            Some(expected.into()),
            "mode {mode:?}"
        );
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
        tool_calls: Some(ToolCallsMode::Strip.into()),
        summary: None,
    };
    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            std::slice::from_ref(&rule),
            &TurnSelection::default(),
            Some(&Printer::sink()),
        ))
        .unwrap();
    assert!(
        compactions.is_empty(),
        "keep_last covering the whole conversation must compact nothing"
    );
}

#[test]
fn from_last_compaction_resolves_against_original_stream_for_every_rule() {
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
            reasoning: Some(ReasoningMode::Strip.into()),
            tool_calls: None,
            summary: None,
        },
        CompactionRuleConfig {
            keep_first: RuleBound::Turns(0),
            keep_last: RuleBound::Turns(3),
            reasoning: None,
            tool_calls: Some(ToolCallsMode::Strip.into()),
            summary: None,
        },
    ];

    let compact = parse_compact(&["--from", "last-compaction"]);
    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            &compact.range,
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
        tool_calls: Some(ToolCallsMode::StripRequests.into()),
        summary: None,
    }];

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            &TurnSelection::default(),
            Some(&Printer::sink()),
        ))
        .unwrap();

    // One rule -> one compaction. keep_first=1/keep_last=1 over 6 turns -> 1..=4,
    // and `strip-requests` maps to `Strip { request: true, response: false }`.
    assert_eq!(compactions.len(), 1);
    assert_eq!((compactions[0].from_turn, compactions[0].to_turn), (1, 4));
    assert_eq!(
        compactions[0].tool_calls,
        Some(
            ToolCallPolicy::Strip {
                request: true,
                response: false,
            }
            .into()
        )
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

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            &compact.range,
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

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            &compact.range,
            Some(&Printer::sink()),
        ))
        .unwrap();

    assert_eq!(compactions.len(), 1);
    assert_eq!((compactions[0].from_turn, compactions[0].to_turn), (4, 17));
}

#[test]
fn from_end_bounds_agree_across_flags_and_dsl() {
    // `-3` is the third turn from the end wherever it is written. Over 10 turns
    // that is index 7, so every spelling compacts through index 7 inclusive and
    // leaves the final two alone.
    //
    // The start differs by design: `--to` leaves `keep_first` to the config
    // default (1, preserving the genesis turn), while the DSL's explicit open
    // start (`..-3`) asks for the whole front of the conversation.
    //
    // `--keep-last 2` is the count that names the same end: preserving two
    // trailing turns stops one turn earlier than the position `-3` suggests to
    // a reader who expects the numbers to match.
    let mut stream = ConversationStream::new_test();
    for t in 0..10 {
        stream.start_turn(format!("turn {t}"));
    }

    let cfg = AppConfig::new_test();
    for (args, expected) in [
        (vec!["--to=-3", "-r"], (1, 7)),
        (vec!["-k", "r:..-3"], (0, 7)),
        // The count that preserves the same two trailing turns.
        (vec!["--keep-last", "2", "-r"], (1, 7)),
    ] {
        let compact = parse_compact(&args);
        let rules = compact.effective_rules(&cfg).unwrap();

        let compactions = runtime()
            .block_on(build_compaction_events(
                &stream,
                &cfg,
                &rules,
                &compact.range,
                Some(&Printer::sink()),
            ))
            .unwrap();

        assert_eq!(compactions.len(), 1, "{args:?}");
        assert_eq!(
            (compactions[0].from_turn, compactions[0].to_turn),
            expected,
            "{args:?}"
        );
    }
}

#[test]
fn first_and_last_compact_two_windows_and_skip_the_middle() {
    // `--first 2 --last 2` over 8 turns compacts turns 1-2 and 7-8, leaving the
    // four turns between them raw. Each window becomes its own compaction event.
    let mut stream = ConversationStream::new_test();
    for t in 0..8 {
        stream.start_turn(format!("turn {t}"));
    }

    let compact = parse_compact(&["--first", "2", "--last", "2", "-r"]);
    let cfg = AppConfig::new_test();
    let rules = compact.effective_rules(&cfg).unwrap();

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            &compact.range,
            Some(&Printer::sink()),
        ))
        .unwrap();

    let ranges: Vec<_> = compactions
        .iter()
        .map(|c| (c.from_turn, c.to_turn))
        .collect();
    assert_eq!(ranges, vec![(0, 1), (6, 7)]);
}

#[test]
fn an_existing_summary_spanning_both_windows_plans_one_range() {
    // An existing summary over turns 1-8 sits under both `--first 2` and
    // `--last 2`. `extend_summary_range` grows each window onto it, so both
    // become 1-8: without coalescing that is two LLM calls and two identical
    // compactions, the second immediately superseding the first.
    //
    // Asserted at the planning seam, which is where the duplicate would be
    // introduced; going through `build_compaction_events` would need a live
    // summarizer.
    let mut stream = ConversationStream::new_test();
    for t in 0..8 {
        stream.start_turn(format!("turn {t}"));
    }
    stream.add_compaction(Compaction::new(0, 7).with_summary(SummaryPolicy::generated("existing")));

    let rule = summary_rule(None);
    let compact = parse_compact(&["--first", "2", "--last", "2"]);
    let windows = compact.range.windows(&stream);
    assert_eq!(windows.len(), 2, "the two windows start out disjoint");

    let ranges = plan_rule_ranges(&stream, &stream, &rule, &windows, &compact.range).unwrap();

    assert_eq!(
        ranges
            .iter()
            .map(|r| (r.from_turn, r.to_turn))
            .collect::<Vec<_>>(),
        vec![(0, 7)],
        "both windows extend onto the same existing summary, so they are one range"
    );
}

#[test]
fn disjoint_windows_without_an_overlapping_summary_stay_separate() {
    // The coalescing must not collapse windows that genuinely name distinct
    // regions: with no summary to extend onto, `--first 2 --last 2` over 8 turns
    // plans two ranges.
    let mut stream = ConversationStream::new_test();
    for t in 0..8 {
        stream.start_turn(format!("turn {t}"));
    }

    let rule = summary_rule(None);
    let compact = parse_compact(&["--first", "2", "--last", "2"]);
    let windows = compact.range.windows(&stream);

    let ranges = plan_rule_ranges(&stream, &stream, &rule, &windows, &compact.range).unwrap();

    assert_eq!(
        ranges
            .iter()
            .map(|r| (r.from_turn, r.to_turn))
            .collect::<Vec<_>>(),
        vec![(0, 1), (6, 7)]
    );
}

#[test]
fn overlapping_first_and_last_windows_compact_once() {
    // On a 3-turn conversation `--first 2 --last 2` covers every turn twice.
    // Compaction acts per window, so two windows here would compact turn 2
    // twice — and for a summary rule, generate two overlapping summaries.
    let mut stream = ConversationStream::new_test();
    for t in 0..3 {
        stream.start_turn(format!("turn {t}"));
    }

    let compact = parse_compact(&["--first", "2", "--last", "2", "-r"]);
    let cfg = AppConfig::new_test();
    let rules = compact.effective_rules(&cfg).unwrap();

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            &compact.range,
            Some(&Printer::sink()),
        ))
        .unwrap();

    let ranges: Vec<_> = compactions
        .iter()
        .map(|c| (c.from_turn, c.to_turn))
        .collect();
    assert_eq!(ranges, vec![(0, 2)]);
}

#[test]
fn abutting_first_and_last_windows_compact_once() {
    // `--first 2 --last 2` over exactly 4 turns leaves no gap between the two
    // windows, so they are one region rather than two touching ones.
    let mut stream = ConversationStream::new_test();
    for t in 0..4 {
        stream.start_turn(format!("turn {t}"));
    }

    let compact = parse_compact(&["--first", "2", "--last", "2", "-r"]);
    let cfg = AppConfig::new_test();
    let rules = compact.effective_rules(&cfg).unwrap();

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            &compact.range,
            Some(&Printer::sink()),
        ))
        .unwrap();

    let ranges: Vec<_> = compactions
        .iter()
        .map(|c| (c.from_turn, c.to_turn))
        .collect();
    assert_eq!(ranges, vec![(0, 3)]);
}

#[test]
fn cli_keep_first_replaces_the_configured_keep_first() {
    // An explicit `--keep-first 1` overrides the rule's `keep_first = 4` rather
    // than stacking with it: the compacted range starts at turn 2 (index 1).
    let mut stream = ConversationStream::new_test();
    for t in 0..8 {
        stream.start_turn(format!("turn {t}"));
    }

    let cfg = AppConfig::new_test();
    let rules = vec![CompactionRuleConfig {
        keep_first: RuleBound::Turns(4),
        keep_last: RuleBound::Turns(0),
        reasoning: Some(ReasoningMode::Strip.into()),
        tool_calls: None,
        summary: None,
    }];

    let compact = parse_compact(&["--keep-first", "1"]);
    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            &compact.range,
            Some(&Printer::sink()),
        ))
        .unwrap();

    assert_eq!(compactions.len(), 1);
    assert_eq!((compactions[0].from_turn, compactions[0].to_turn), (1, 7));
}

#[test]
fn keep_first_clamps_rather_than_shifts_an_explicit_from() {
    // Turn 1 is already outside `--from 3`, so `--keep-first 1` has nothing to
    // protect and the range is unchanged.
    // `--to -1` pins the end via the CLI; left open, the ad-hoc `-r` rule's
    // default `keep_last = 1` would supply it and obscure the start behaviour
    // under test.
    let mut stream = ConversationStream::new_test();
    for t in 0..8 {
        stream.start_turn(format!("turn {t}"));
    }

    let compact = parse_compact(&["--from", "3", "--to", "-1", "--keep-first", "1", "-r"]);
    let cfg = AppConfig::new_test();
    let rules = compact.effective_rules(&cfg).unwrap();

    let compactions = runtime()
        .block_on(build_compaction_events(
            &stream,
            &cfg,
            &rules,
            &compact.range,
            Some(&Printer::sink()),
        ))
        .unwrap();

    assert_eq!(compactions.len(), 1);
    assert_eq!((compactions[0].from_turn, compactions[0].to_turn), (2, 7));
}

#[test]
fn keep_first_greater_than_first_is_rejected() {
    // A preserved prefix larger than the selection is nonsensical and must be
    // an error rather than a silent no-op.
    let err = parse_compact(&["--keep-first", "17", "--first", "16"])
        .range
        .validate()
        .unwrap_err();
    assert_eq!(
        err,
        "--keep-first 17 is greater than --first 16: nothing would remain to select"
    );

    // Equal values select nothing, which is empty but not nonsensical.
    assert!(
        parse_compact(&["--keep-first", "16", "--first", "16"])
            .range
            .validate()
            .is_ok()
    );
}

#[test]
fn keep_last_greater_than_last_is_rejected() {
    // A preserved suffix larger than the selection is nonsensical and must be
    // an error rather than a silent no-op.
    let err = parse_compact(&["--keep-last", "17", "--last", "16"])
        .range
        .validate()
        .unwrap_err();
    assert_eq!(
        err,
        "--keep-last 17 is greater than --last 16: nothing would remain to select"
    );

    // Equal values select nothing, which is empty but not nonsensical.
    assert!(
        parse_compact(&["--keep-last", "16", "--last", "16"])
            .range
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
        parse_compact(&["--turn", "100"])
            .range
            .check_turn_range(5)
            .unwrap_err(),
        "turn 100 out of range (conversation has 5 turns)"
    );
    assert!(
        parse_compact(&["--turn", "..100"])
            .range
            .check_turn_range(5)
            .is_err()
    );
    assert!(
        parse_compact(&["--turn", "3"])
            .range
            .check_turn_range(5)
            .is_ok()
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
        items: None,
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
            items: None,
        },
        TimelineSegment {
            from: 6,
            to: 8,
            label: None,
            existing: false,
            items: None,
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
            items: None,
        },
        TimelineSegment {
            from: 1,
            to: 3,
            label: None,
            existing: false,
            items: None,
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
            items: None,
        },
        TimelineSegment {
            from: 3,
            to: 8,
            label: None,
            existing: false,
            items: None,
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
        items: None,
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
        items: None,
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
    let segments = segments_for_compactions(
        std::slice::from_ref(&compaction),
        &ConversationStream::new_test(),
        "test-conv",
    );
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].label.as_deref(), Some("reasoning + tools"));
}

/// A one-turn stream with two tool calls: a 4 KB response and a 2-byte one.
fn stream_with_a_large_and_a_small_call() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("read them");
    stream
        .current_turn_mut()
        .add_tool_call_request(ToolCallRequest {
            id: "big".into(),
            name: "fs_read_file".into(),
            arguments: Map::from_iter([("path".into(), Value::from("huge.log"))]),
        })
        .add_tool_call_response(ToolCallResponse {
            id: "big".into(),
            result: Ok("x".repeat(4096)),
        })
        .add_tool_call_request(ToolCallRequest {
            id: "small".into(),
            name: "fs_read_file".into(),
            arguments: Map::from_iter([("path".into(), Value::from("tiny.log"))]),
        })
        .add_tool_call_response(ToolCallResponse {
            id: "small".into(),
            result: Ok("ok".into()),
        })
        .build()
        .unwrap();
    stream
}

#[test]
fn timeline_lists_what_a_threshold_caught() {
    // A threshold reaches an unpredictable subset of its range, so the range
    // line alone would leave the user guessing whether it hit 1 call or 12.
    let stream = stream_with_a_large_and_a_small_call();
    let compaction = Compaction::new(0, 0).with_tool_calls(PolicySpec::over(
        ToolCallPolicy::Strip {
            request: false,
            response: true,
        },
        ByteSize::from_bytes(1024),
    ));

    let segments = segments_for_compactions(std::slice::from_ref(&compaction), &stream, "conv");
    let lines = timeline_lines(&segments, 0, true);

    assert_eq!(lines, vec![
        "Would have compacted turns 1..1 (1 total, tool responses over 1KB).".to_owned(),
        "  turn 1  fs_read_file (response)  4.0 KB".to_owned(),
    ]);
}

#[test]
fn timeline_says_so_when_a_threshold_caught_nothing() {
    // Silence here would read as "the whole range was compacted".
    let stream = stream_with_a_large_and_a_small_call();
    let compaction = Compaction::new(0, 0).with_tool_calls(PolicySpec::over(
        ToolCallPolicy::Strip {
            request: false,
            response: true,
        },
        ByteSize::from_bytes(1024 * 1024),
    ));

    let segments = segments_for_compactions(std::slice::from_ref(&compaction), &stream, "conv");
    let lines = timeline_lines(&segments, 0, true);

    assert_eq!(lines, vec![
        "Would have compacted turns 1..1 (1 total, tool responses over 1MB).".to_owned(),
        "  nothing over the threshold.".to_owned(),
    ]);
}

#[test]
fn timeline_does_not_itemize_a_summary_rule() {
    // `-k 's+r,over=1kb'` parses: the threshold binds to `r`, and the summary
    // rule carries it. A summary replaces its whole range, so projection never
    // consults the threshold and itemizing it would name items nothing selected.
    let stream = stream_with_a_large_and_a_small_call();
    let compaction = Compaction::new(0, 0)
        .with_tool_calls(PolicySpec::over(
            ToolCallPolicy::Strip {
                request: false,
                response: true,
            },
            ByteSize::from_bytes(1024),
        ))
        .with_summary(jp_conversation::SummaryPolicy::generated("the gist"));

    let segments = segments_for_compactions(std::slice::from_ref(&compaction), &stream, "conv");
    let lines = timeline_lines(&segments, 0, true);

    assert_eq!(lines.len(), 1, "no item lines: {lines:?}");
    assert!(
        lines[0].contains("summary"),
        "summary wins the label: {}",
        lines[0]
    );
}

#[test]
fn timeline_does_not_itemize_a_rule_without_a_threshold() {
    // An unnarrowed rule reaches everything in range by definition, so listing
    // each call would be noise.
    let stream = stream_with_a_large_and_a_small_call();
    let compaction = Compaction::new(0, 0).with_tool_calls(ToolCallPolicy::Strip {
        request: false,
        response: true,
    });

    let segments = segments_for_compactions(std::slice::from_ref(&compaction), &stream, "conv");
    let lines = timeline_lines(&segments, 0, true);

    assert_eq!(lines, vec![
        "Would have compacted turns 1..1 (1 total, tool responses).".to_owned(),
    ]);
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
        items: None,
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
        items: None,
    });

    let lines = timeline_lines(&segments, 9, true);
    assert_eq!(lines, vec![
        "Would have kept turn 1.".to_owned(),
        "Compacted turns 2..6 (5 total, already compacted).".to_owned(),
        "Would have compacted turns 7..9 (3 total).".to_owned(),
        "Would have kept turn 10.".to_owned(),
    ]);
}
