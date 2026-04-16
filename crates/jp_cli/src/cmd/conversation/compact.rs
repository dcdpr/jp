use std::{str::FromStr as _, time::Duration};

use chrono::{DateTime, Utc};
use jp_config::{
    PartialAppConfig,
    conversation::compaction::{
        CompactionRuleConfig, PartialCompactionRuleConfig, PartialSummaryConfig, ReasoningMode,
        RuleBound, ToolCallsMode,
    },
    types::vec::MergeableVec,
};
use jp_conversation::{
    Compaction, ConversationStream, RangeBound, ReasoningPolicy, SummaryPolicy, ToolCallPolicy,
    compaction::{extend_summary_range, resolve_range},
};
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
    },
    ctx::{Ctx, IntoPartialAppConfig},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Compact {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Preserve the first N turns (or turns within a duration).
    ///
    /// Accepts a turn count (e.g. `2`) or a duration (e.g. `5h`).
    #[arg(long)]
    keep_first: Option<RuleBound>,

    /// Preserve the last N turns (or turns within a duration).
    ///
    /// Accepts a turn count (e.g. `3`) or a duration (e.g. `2h`).
    #[arg(long)]
    keep_last: Option<RuleBound>,

    /// Start compacting from a specific turn or time.
    ///
    /// Accepts an absolute turn index, a duration (e.g. `5h`), or `last`
    /// to start after the most recent compaction. Overrides `--keep-first`.
    #[arg(long, value_parser = parse_bound, conflicts_with = "keep_first")]
    from: Option<CliRangeBound>,

    /// Stop compacting at a specific turn or time.
    ///
    /// Accepts an absolute turn index or a duration. Overrides `--keep-last`.
    #[arg(long, value_parser = parse_bound, conflicts_with = "keep_last")]
    to: Option<CliRangeBound>,

    /// Strip reasoning (thinking) blocks from the compacted range.
    #[arg(long)]
    reasoning: Option<bool>,

    /// Strip tool call arguments and responses in the compacted range.
    #[arg(long)]
    tools: Option<bool>,

    /// Generate an LLM summary for the compacted range.
    ///
    /// When enabled, the compacted turns are replaced with a single
    /// LLM-generated summary.
    #[arg(long)]
    summarize: Option<Option<bool>>,

    /// Preview what would change without applying.
    #[arg(long)]
    dry_run: bool,

    /// Remove all compaction events from the stream.
    ///
    /// Restores the raw event history so the LLM sees all original events.
    #[arg(long)]
    reset: bool,

    /// Compact using an inline DSL rule.
    ///
    /// Can be used alongside the dedicated flags above, or on its own.
    /// See `jp query --help` for DSL syntax.
    #[command(flatten)]
    compact_flag: crate::cmd::compact_flag::CompactFlag,
}

impl Compact {
    /// Returns `true` if any flag that overrides compaction rule config is set.
    ///
    /// When true, the rules array is replaced with a single ad-hoc rule built
    /// from the CLI flags via [`IntoPartialAppConfig`].
    fn has_rule_overrides(&self) -> bool {
        self.keep_first.is_some()
            || self.keep_last.is_some()
            || self.reasoning.is_some()
            || self.tools.is_some()
            || self.summarize.is_some()
    }
}

impl IntoPartialAppConfig for Compact {
    fn apply_cli_config(
        &self,
        _workspace: Option<&jp_workspace::Workspace>,
        mut partial: PartialAppConfig,
        _merged_config: Option<&PartialAppConfig>,
        _handles: &[jp_workspace::ConversationHandle],
    ) -> Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        // Dedicated flags build a single ad-hoc rule.
        if self.has_rule_overrides() {
            let mut rule = PartialCompactionRuleConfig::default();

            if let Some(bound) = &self.keep_first {
                rule.keep_first = Some(bound.clone());
            }
            if let Some(bound) = &self.keep_last {
                rule.keep_last = Some(bound.clone());
            }
            if let Some(true) = self.reasoning {
                rule.reasoning = Some(ReasoningMode::Strip);
            }
            if let Some(true) = self.tools {
                rule.tool_calls = Some(ToolCallsMode::Strip);
            }
            if let Some(summarize) = self.summarize
                && summarize.unwrap_or(true)
            {
                rule.summary = Some(PartialSummaryConfig::default());
            }

            partial.conversation.compaction.rules = MergeableVec::Vec(vec![rule]);
        }

        // DSL specs (from --compact=SPEC / -k) compose on top.
        self.compact_flag.apply_to_config(&mut partial);

        Ok(partial)
    }
}

/// A CLI range bound before time-based resolution.
#[derive(Debug, Clone)]
enum CliRangeBound {
    /// Already resolved to a `RangeBound`.
    Resolved(RangeBound),
    /// Duration ago — needs the stream to find the turn.
    Duration(DateTime<Utc>),
}

fn parse_bound(s: &str) -> Result<CliRangeBound, String> {
    if s.eq_ignore_ascii_case("last") {
        return Ok(CliRangeBound::Resolved(RangeBound::AfterLastCompaction));
    }

    // Negative integer → FromEnd.
    if let Some(rest) = s.strip_prefix('-')
        && let Ok(n) = rest.parse::<usize>()
    {
        return Ok(CliRangeBound::Resolved(RangeBound::FromEnd(n)));
    }

    // Positive integer → Absolute.
    if let Ok(n) = s.parse::<usize>() {
        return Ok(CliRangeBound::Resolved(RangeBound::Absolute(n)));
    }

    // Duration string → resolve to DateTime.
    humantime::Duration::from_str(s)
        .map(|d| CliRangeBound::Duration(Utc::now() - Duration::from(d)))
        .map_err(|e| format!("invalid range bound `{s}`: {e}"))
}

/// Build a [`Compaction`] event from a resolved config rule.
///
/// `from_override` and `to_override` are runtime-resolved range bounds
/// (`--from`/`--to`) that take precedence over the rule's `keep_first`/
/// `keep_last`.
///
/// Returns `None` if the resolved range is empty (nothing to compact).
pub(crate) async fn build_compaction_event(
    events: &ConversationStream,
    cfg: &jp_config::AppConfig,
    rule: &CompactionRuleConfig,
    from_override: Option<RangeBound>,
    to_override: Option<RangeBound>,
    printer: &jp_printer::Printer,
) -> crate::Result<Option<Compaction>> {
    let from = from_override.or_else(|| keep_first_to_bound(&rule.keep_first, events));
    let to = to_override.or_else(|| keep_last_to_bound(&rule.keep_last, events));

    let Some(range) = resolve_range(events, from, to) else {
        return Ok(None);
    };

    let should_summarize = rule.summary.is_some();

    // Auto-extend range if summary would partially overlap existing summaries.
    let range = if should_summarize {
        extend_summary_range(events, range)
    } else {
        range
    };

    let summary_text = if should_summarize {
        printer.println("Generating summary...");
        let text = super::summarize::generate_summary(
            events,
            range.from_turn,
            range.to_turn,
            rule.summary.as_ref(),
            cfg,
        )
        .await?;
        Some(text)
    } else {
        None
    };

    let mut compaction = build_mechanical_compaction(range.from_turn, range.to_turn, rule);

    if let Some(text) = summary_text {
        compaction = compaction.with_summary(SummaryPolicy { summary: text });
    }

    Ok(Some(compaction))
}

/// Build compaction events from all config rules.
///
/// Each rule produces one `Compaction` event. Runtime range overrides
/// (`--from`/`--to`) apply to every rule.
pub(crate) async fn build_compaction_events_from_config(
    events: &ConversationStream,
    cfg: &jp_config::AppConfig,
    from_override: Option<RangeBound>,
    to_override: Option<RangeBound>,
    printer: &jp_printer::Printer,
) -> crate::Result<Vec<Compaction>> {
    let mut compactions = Vec::new();
    for rule in &cfg.conversation.compaction.rules {
        if let Some(c) = build_compaction_event(
            events,
            cfg,
            rule,
            from_override.clone(),
            to_override.clone(),
            printer,
        )
        .await?
        {
            compactions.push(c);
        }
    }

    Ok(compactions)
}

/// Convert a `keep_first` rule bound to a `from` `RangeBound`.
fn keep_first_to_bound(bound: &RuleBound, events: &ConversationStream) -> Option<RangeBound> {
    match bound {
        RuleBound::Turns(n) => Some(RangeBound::Absolute(*n)),
        RuleBound::Duration(d) => {
            let dt = chrono::Utc::now() - *d;
            Some(RangeBound::Absolute(events.turn_at_time(dt)?.index()))
        }
        RuleBound::AfterLastCompaction => Some(RangeBound::AfterLastCompaction),
    }
}

/// Convert a `keep_last` rule bound to a `to` `RangeBound`.
fn keep_last_to_bound(bound: &RuleBound, events: &ConversationStream) -> Option<RangeBound> {
    match bound {
        RuleBound::Turns(n) => Some(RangeBound::FromEnd(*n)),
        RuleBound::Duration(d) => {
            let dt = chrono::Utc::now() - *d;
            Some(RangeBound::Absolute(events.turn_at_time(dt)?.index()))
        }
        RuleBound::AfterLastCompaction => None,
    }
}

/// Build a `Compaction` event from mechanical policies (no summary).
fn build_mechanical_compaction(
    from_turn: usize,
    to_turn: usize,
    rule: &CompactionRuleConfig,
) -> Compaction {
    let mut compaction = Compaction::new(from_turn, to_turn);

    if rule.reasoning.is_some() {
        compaction = compaction.with_reasoning(ReasoningPolicy::Strip);
    }

    if let Some(mode) = rule.tool_calls {
        compaction = compaction.with_tool_calls(match mode {
            ToolCallsMode::Strip => ToolCallPolicy::Strip {
                request: true,
                response: true,
            },
            ToolCallsMode::StripResponses => ToolCallPolicy::Strip {
                request: false,
                response: true,
            },
            ToolCallsMode::StripRequests => ToolCallPolicy::Strip {
                request: true,
                response: false,
            },
            ToolCallsMode::Omit => ToolCallPolicy::Omit,
        });
    }

    compaction
}

impl Compact {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        for handle in handles {
            self.compact_one(ctx, handle).await?;
        }
        Ok(())
    }

    async fn compact_one(&self, ctx: &mut Ctx, handle: ConversationHandle) -> Output {
        let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
            LockOutcome::Acquired(lock) => lock,
            LockOutcome::NewConversation | LockOutcome::ForkConversation(_) => {
                unreachable!("compact does not allow new/fork on contention")
            }
        };

        let cfg = ctx.config();
        let conv = lock.into_mut();
        let events_snapshot = conv.events().clone();

        if self.reset {
            let removed = conv.update_events(ConversationStream::remove_compactions);
            if removed > 0 {
                ctx.printer
                    .println(format!("Removed {removed} compaction event(s)."));
            } else {
                ctx.printer.println("No compaction events to remove.");
            }
            return Ok(());
        }

        // --from/--to are runtime-resolved range overrides (they need the
        // stream for duration and "last" resolution). They apply to all rules.
        let from_override = self.resolve_from(&events_snapshot);
        let to_override = self.resolve_to(&events_snapshot);

        if self.dry_run {
            let range = resolve_range(&events_snapshot, from_override.clone(), to_override.clone());
            if let Some(range) = range {
                ctx.printer.println(format!(
                    "Would compact turns {}..={}",
                    range.from_turn, range.to_turn,
                ));
            } else {
                ctx.printer.println("Nothing to compact.");
            }
            return Ok(());
        }

        let compactions = build_compaction_events_from_config(
            &events_snapshot,
            &cfg,
            from_override,
            to_override,
            &ctx.printer,
        )
        .await?;

        if compactions.is_empty() {
            ctx.printer.println("Nothing to compact.");
            return Ok(());
        }

        for compaction in compactions {
            let from = compaction.from_turn;
            let to = compaction.to_turn;
            conv.update_events(|stream| stream.add_compaction(compaction));
            ctx.printer
                .println(format!("Compacted turns {from}..={to}."));
        }

        Ok(())
    }

    /// Resolve `--from` to a `RangeBound`, if present.
    fn resolve_from(&self, events: &ConversationStream) -> Option<RangeBound> {
        resolve_cli_bound(self.from.as_ref()?, events)
    }

    /// Resolve `--to` to a `RangeBound`, if present.
    fn resolve_to(&self, events: &ConversationStream) -> Option<RangeBound> {
        resolve_cli_bound(self.to.as_ref()?, events)
    }
}

fn resolve_cli_bound(bound: &CliRangeBound, events: &ConversationStream) -> Option<RangeBound> {
    match bound {
        CliRangeBound::Resolved(b) => Some(b.clone()),
        CliRangeBound::Duration(dt) => {
            Some(RangeBound::Absolute(events.turn_at_time(*dt)?.index()))
        }
    }
}
