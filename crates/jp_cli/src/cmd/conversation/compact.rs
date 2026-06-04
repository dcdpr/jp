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
    Compaction, CompactionRange, ConversationStream, RangeBound, ReasoningPolicy, SummaryPolicy,
    ToolCallPolicy,
    compaction::{extend_summary_range, resolve_range},
};
use jp_workspace::{ConversationHandle, ConversationMut};

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
    /// Accepts a turn count (e.g.
    /// `2`) or a duration (e.g.
    /// `5h`).
    #[arg(long)]
    keep_first: Option<RuleBound>,

    /// Preserve the last N turns (or turns within a duration).
    ///
    /// Accepts a turn count (e.g.
    /// `3`) or a duration (e.g.
    /// `2h`).
    #[arg(long)]
    keep_last: Option<RuleBound>,

    /// Start compacting from a specific turn or time.
    ///
    /// Accepts an absolute turn index, a duration (e.g.
    /// `5h`), or `last` to start after the most recent compaction.
    /// Overrides `--keep-first`.
    #[arg(long, value_parser = parse_bound, conflicts_with = "keep_first")]
    from: Option<CliRangeBound>,

    /// Stop compacting at a specific turn or time.
    ///
    /// Accepts an absolute turn index or a duration.
    /// Overrides `--keep-last`.
    #[arg(long, value_parser = parse_bound, conflicts_with = "keep_last")]
    to: Option<CliRangeBound>,

    /// Strip reasoning (thinking) blocks from the compacted range.
    #[arg(short, long, conflicts_with = "compact")]
    reasoning: bool,

    /// Strip tool call content from the compacted range.
    ///
    /// Used without a value, strips both requests and responses.
    /// Otherwise one of (with short aliases):
    ///
    /// - `strip` (`s`): strip request arguments and response content
    /// - `strip-requests` (`sreq`): strip request arguments only
    /// - `strip-responses` (`sres`): strip response content only
    /// - `omit` (`o`): remove tool call pairs entirely
    #[arg(
        short,
        long,
        value_parser = parse_tool_calls_mode,
        num_args = 0..=1,
        default_missing_value = "strip",
        conflicts_with = "compact",
    )]
    tools: Option<ToolCallsMode>,

    /// Generate an LLM summary for the compacted range.
    ///
    /// When enabled, the compacted turns are replaced with a single
    /// LLM-generated summary.
    #[arg(short, long, conflicts_with = "compact")]
    summarize: bool,

    /// Preview what would change without applying.
    #[arg(long)]
    dry_run: bool,

    /// Remove all compaction events from the stream.
    ///
    /// Restores the raw event history so the LLM sees all original events.
    /// Mutually exclusive with the policy, range, and DSL flags: `--reset`
    /// undoes compaction, it does not re-compact in the same invocation.
    /// Composes with `--dry-run` to preview the removal.
    #[arg(
        long,
        conflicts_with_all = [
            "keep_first", "keep_last", "from", "to", "reasoning", "tools",
            "summarize", "compact",
        ],
    )]
    reset: bool,

    /// Compact using an inline DSL rule.
    ///
    /// Mutually exclusive with the dedicated `--reasoning`/`--tools`/
    /// `--summarize` flags above: use either the flags or the DSL, not both.
    /// See `jp query --help` for DSL syntax.
    #[command(flatten)]
    compact_flag: crate::cmd::compact_flag::CompactFlag,
}

impl Compact {
    /// Returns `true` if any dedicated policy flag is set.
    ///
    /// Policy flags (`--reasoning`/`--tools`/`--summarize`) build a single
    /// ad-hoc rule.
    /// Range flags (`--keep-first`/`--keep-last`/`--from`/`--to`) are
    /// deliberately excluded: they are applied at runtime as range overrides on
    /// the active rules, not as a rule of their own.
    fn has_policy_overrides(&self) -> bool {
        self.reasoning || self.tools.is_some() || self.summarize
    }
}

impl IntoPartialAppConfig for Compact {
    fn apply_cli_config(
        &self,
        _workspace: Option<&jp_workspace::Workspace>,
        mut partial: PartialAppConfig,
        _merged_config: Option<&PartialAppConfig>,
    ) -> Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        // Dedicated policy flags build a single ad-hoc rule; inline DSL specs
        // each build one. clap makes the two mutually exclusive (the policy
        // flags `conflicts_with` the `compact` flag), so at most one side is
        // ever populated here.
        //
        // Range flags are NOT rules — `compact_one` applies them as range
        // overrides on whichever rules end up active (see `resolve_from` /
        // `resolve_to`), so a range-only invocation narrows the configured
        // rules instead of replacing them with a policy-less no-op.
        let mut rules: Vec<PartialCompactionRuleConfig> = Vec::new();

        if self.has_policy_overrides() {
            let mut rule = PartialCompactionRuleConfig::default();
            if self.reasoning {
                rule.reasoning = Some(ReasoningMode::Strip);
            }
            rule.tool_calls = self.tools;
            if self.summarize {
                rule.summary = Some(PartialSummaryConfig::default());
            }
            rules.push(rule);
        }

        rules.extend(self.compact_flag.dsl_rules());

        if !rules.is_empty() {
            // Explicit policy/DSL rules replace the configured rules, unless a
            // bare `--compact` is also present, in which case they append to
            // the config rules (seeding the built-in default when the config
            // provides none).
            if self.compact_flag.use_config_rules {
                crate::cmd::compact_flag::append_config_rules(&mut partial, rules);
            } else {
                partial.conversation.compaction.rules = MergeableVec::Vec(rules);
            }
        }

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

fn parse_tool_calls_mode(s: &str) -> Result<ToolCallsMode, String> {
    s.parse().map_err(|_| {
        "expected one of: strip (s), strip-requests (sreq), strip-responses (sres), omit (o)"
            .to_string()
    })
}

/// The resolution of one range bound (`from` or `to`) against a stream.
///
/// Separates "no bound configured for this side" (use the side's default) from
/// "this bound selects no turns" (so the whole compaction is a no-op), which a
/// plain `Option<RangeBound>` conflated — e.g. `keep_last = "30d"` covering
/// the entire conversation must preserve everything, not fall back to the
/// default `keep_last` and compact through the end.
#[derive(Debug, Clone)]
pub(crate) enum Bound {
    /// No bound configured; the range defaults to the start (`from`) or end
    /// (`to`) of the conversation.
    Default,
    /// The bound resolves to a concrete `RangeBound`.
    At(RangeBound),
    /// The bound falls outside the conversation such that nothing is compacted.
    Empty,
}

/// Resolve the turn range a single rule would compact.
///
/// Applies the runtime range overrides (`--from`/`--to`/`--keep-first`/
/// `--keep-last`) on top of the rule's own bounds and, for summary rules,
/// extends the range to subsume partially overlapping summaries.
///
/// Shared by the dry-run preview and [`build_compaction_event`] so the preview
/// and the actual mutation always agree on the range.
fn resolve_rule_range(
    events: &ConversationStream,
    rule: &CompactionRuleConfig,
    from_override: Bound,
    to_override: Bound,
) -> Option<CompactionRange> {
    // A CLI override (`--from`/`--to`/`--keep-first`/`--keep-last`) takes
    // precedence; otherwise fall back to the rule's own bound. Either side
    // resolving to `Empty` means nothing is compacted.
    let from = match from_override {
        Bound::Default => keep_first_to_bound(&rule.keep_first, events),
        other => other,
    };
    let to = match to_override {
        Bound::Default => keep_last_to_bound(&rule.keep_last, events),
        other => other,
    };

    let from = match from {
        Bound::Empty => return None,
        Bound::Default => None,
        Bound::At(b) => Some(b),
    };
    let to = match to {
        Bound::Empty => return None,
        Bound::Default => None,
        Bound::At(b) => Some(b),
    };

    let range = resolve_range(events, from, to)?;
    Some(if rule.summary.is_some() {
        extend_summary_range(events, range)
    } else {
        range
    })
}

/// Build a [`Compaction`] event from a resolved config rule.
///
/// `from_override` and `to_override` are runtime-resolved range bounds
/// (`--from`/`--to`/`--keep-first`/`--keep-last`) that take precedence over the
/// rule's `keep_first`/`keep_last`.
///
/// Returns `None` if the resolved range is empty (nothing to compact).
pub(crate) async fn build_compaction_event(
    events: &ConversationStream,
    cfg: &jp_config::AppConfig,
    rule: &CompactionRuleConfig,
    from_override: Bound,
    to_override: Bound,
    printer: &jp_printer::Printer,
) -> crate::Result<Option<Compaction>> {
    let Some(range) = resolve_rule_range(events, rule, from_override, to_override) else {
        return Ok(None);
    };

    let summary_text = if rule.summary.is_some() {
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
/// Each rule produces one `Compaction` event.
/// Runtime range overrides (`--from`/`--to`) apply to every rule.
pub(crate) async fn build_compaction_events_from_config(
    events: &ConversationStream,
    cfg: &jp_config::AppConfig,
    from_override: Bound,
    to_override: Bound,
    printer: &jp_printer::Printer,
) -> crate::Result<Vec<Compaction>> {
    // Accumulate generated compactions onto a working stream so each rule's
    // summary-overlap extension sees the compactions produced by earlier rules
    // in this same invocation, not just those already on the stream. Turn
    // iteration ignores compaction overlays, so this does not affect the events
    // a summarizer reads or the turn-count used for range resolution.
    let mut working = events.clone();
    let mut compactions = Vec::new();
    for rule in &cfg.conversation.compaction.rules {
        if let Some(c) = build_compaction_event(
            &working,
            cfg,
            rule,
            from_override.clone(),
            to_override.clone(),
            printer,
        )
        .await?
        {
            working.add_compaction(c.clone());
            compactions.push(c);
        }
    }

    Ok(compactions)
}

/// Append compaction events to the conversation, announcing each one.
///
/// The reported turn range is inclusive; `(N total)` is its turn count.
pub(crate) fn apply_compactions(
    conv: &ConversationMut,
    compactions: Vec<Compaction>,
    printer: &jp_printer::Printer,
) {
    for compaction in compactions {
        let from = compaction.from_turn;
        let to = compaction.to_turn;
        let count = to - from + 1;
        conv.update_events(|stream| stream.add_compaction(compaction));
        printer.println(format!("Compacted turns {from}..={to} ({count} total)."));
    }
}

/// Convert a `keep_first` rule bound to a `from` [`Bound`].
fn keep_first_to_bound(bound: &RuleBound, events: &ConversationStream) -> Bound {
    match bound {
        // "Keep first N" means compaction starts at turn N.
        RuleBound::Turns(n) | RuleBound::Absolute(n) => Bound::At(RangeBound::Absolute(*n)),
        RuleBound::FromEnd(n) => Bound::At(RangeBound::FromEnd(*n)),
        RuleBound::Duration(d) => {
            // Preserve the opening `d` window: start compacting at the first
            // turn after `conversation_start + d`. A window covering the whole
            // conversation preserves everything.
            let Some(first) = events.iter().next() else {
                return Bound::Empty;
            };
            let Ok(d) = chrono::Duration::from_std(*d) else {
                return Bound::Empty;
            };
            match events.turn_at_time(first.event.timestamp + d) {
                Some(turn) => Bound::At(RangeBound::Absolute(turn.index() + 1)),
                None => Bound::At(RangeBound::Absolute(0)),
            }
        }
        RuleBound::AfterLastCompaction => Bound::At(RangeBound::AfterLastCompaction),
    }
}

/// Convert a `keep_last` rule bound to a `to` [`Bound`].
fn keep_last_to_bound(bound: &RuleBound, events: &ConversationStream) -> Bound {
    match bound {
        // "Keep last N" means compaction stops N turns before the end.
        RuleBound::Turns(n) | RuleBound::FromEnd(n) => Bound::At(RangeBound::FromEnd(*n)),
        RuleBound::Absolute(n) => Bound::At(RangeBound::Absolute(*n)),
        RuleBound::Duration(d) => {
            // Compact turns older than the last `d` window. A window covering
            // the whole conversation preserves everything → nothing to compact.
            match events.turn_at_time(Utc::now() - *d) {
                Some(turn) => Bound::At(RangeBound::Absolute(turn.index())),
                None => Bound::Empty,
            }
        }
        RuleBound::AfterLastCompaction => Bound::Default,
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
            if self.dry_run {
                // Preview only — `--dry-run` must not mutate the conversation.
                let count = events_snapshot.compactions().count();
                if count > 0 {
                    ctx.printer
                        .println(format!("Would remove {count} compaction event(s)."));
                } else {
                    ctx.printer.println("No compaction events to remove.");
                }
            } else {
                let removed = conv.update_events(ConversationStream::remove_compactions);
                if removed > 0 {
                    ctx.printer
                        .println(format!("Removed {removed} compaction event(s)."));
                } else {
                    ctx.printer.println("No compaction events to remove.");
                }
            }
            return Ok(());
        }

        // Range overrides (`--from`/`--to`/`--keep-first`/`--keep-last`) are
        // resolved at runtime (they need the stream for duration and "last"
        // resolution) and apply on top of every active rule.
        let from_override = self.resolve_from(&events_snapshot);
        let to_override = self.resolve_to(&events_snapshot);

        if self.dry_run {
            // Preview using the same per-rule range resolution as the real run
            // (minus the summarizer and the mutation) so the reported ranges
            // match what would actually be applied.
            let mut working = events_snapshot.clone();
            let mut printed = false;
            for rule in &cfg.conversation.compaction.rules {
                let Some(range) =
                    resolve_rule_range(&working, rule, from_override.clone(), to_override.clone())
                else {
                    continue;
                };
                ctx.printer.println(format!(
                    "Would compact turns {}..={}",
                    range.from_turn, range.to_turn,
                ));
                // Mirror the real run's overlap accumulation so later summary
                // rules preview the same (possibly extended) ranges.
                if rule.summary.is_some() {
                    working.add_compaction(
                        Compaction::new(range.from_turn, range.to_turn).with_summary(
                            SummaryPolicy {
                                summary: String::new(),
                            },
                        ),
                    );
                }
                printed = true;
            }
            if !printed {
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

        apply_compactions(&conv, compactions, &ctx.printer);

        Ok(())
    }

    /// Resolve the `from` range override, preferring `--from` over
    /// `--keep-first`.
    /// Returns [`Bound::Default`] when neither is set.
    fn resolve_from(&self, events: &ConversationStream) -> Bound {
        if let Some(bound) = self.from.as_ref() {
            return resolve_cli_from(bound, events);
        }
        match &self.keep_first {
            Some(bound) => keep_first_to_bound(bound, events),
            None => Bound::Default,
        }
    }

    /// Resolve the `to` range override, preferring `--to` over `--keep-last`.
    /// Returns [`Bound::Default`] when neither is set.
    fn resolve_to(&self, events: &ConversationStream) -> Bound {
        if let Some(bound) = self.to.as_ref() {
            return resolve_cli_to(bound, events);
        }
        match &self.keep_last {
            Some(bound) => keep_last_to_bound(bound, events),
            None => Bound::Default,
        }
    }
}

/// Resolve a `--from <duration>` cutoff: compaction starts at the first turn
/// after the cutoff.
/// No turn after the cutoff (an old conversation) means nothing to compact; a
/// cutoff before the conversation starts compacts from the beginning.
fn resolve_cli_from(bound: &CliRangeBound, events: &ConversationStream) -> Bound {
    match bound {
        CliRangeBound::Resolved(b) => Bound::At(b.clone()),
        CliRangeBound::Duration(dt) => match events.turn_at_time(*dt) {
            Some(turn) => {
                let from = turn.index() + 1;
                if from >= events.turn_count() {
                    Bound::Empty
                } else {
                    Bound::At(RangeBound::Absolute(from))
                }
            }
            None => Bound::At(RangeBound::Absolute(0)),
        },
    }
}

/// Resolve a `--to <duration>` cutoff: compaction stops at (and includes) the
/// turn active at the cutoff.
/// A cutoff preceding the conversation means nothing to compact.
fn resolve_cli_to(bound: &CliRangeBound, events: &ConversationStream) -> Bound {
    match bound {
        CliRangeBound::Resolved(b) => Bound::At(b.clone()),
        CliRangeBound::Duration(dt) => match events.turn_at_time(*dt) {
            Some(turn) => Bound::At(RangeBound::Absolute(turn.index())),
            None => Bound::Empty,
        },
    }
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;
