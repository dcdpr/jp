use std::{env, fs, path::PathBuf};

use chrono::Utc;
use jp_config::{
    PartialAppConfig,
    conversation::compaction::{
        CompactionConfig, CompactionRuleConfig, PartialCompactionRuleConfig, PartialSummaryConfig,
        ReasoningMode, RuleBound, ToolCallsMode,
    },
};
use jp_conversation::{
    Compaction, CompactionRange, ConversationStream, RangeBound, ReasoningPolicy, SummaryPolicy,
    ToolCallPolicy,
    compaction::{extend_summary_range, resolve_range},
};
use jp_workspace::{ConversationHandle, ConversationMut, Workspace};
use tracing::warn;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
        query::apply_model,
        turn_range::{Bound, TurnRange},
    },
    ctx::{Ctx, IntoPartialAppConfig},
    format::compaction_policy_label,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Compact {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Preserve the first N turns (or turns within a duration).
    ///
    /// Accepts a turn count (e.g. `2`) or a duration (e.g. `5h`).
    /// Composes with `--first M`: the pair compacts the first M turns minus the
    /// preserved prefix, e.g. `--keep-first 1 --first 16` compacts turns 2
    /// through 16.
    #[arg(long, conflicts_with_all = ["from", "last", "turn"])]
    keep_first: Option<RuleBound>,

    /// Preserve the last N turns (or turns within a duration).
    ///
    /// Accepts a turn count (e.g. `3`) or a duration (e.g. `2h`).
    /// Composes with `--last M`: the pair compacts the last M turns minus the
    /// preserved suffix, e.g. `--keep-last 2 --last 16` compacts the 14 turns
    /// before the final 2.
    #[arg(long, conflicts_with_all = ["to", "first", "turn"])]
    keep_last: Option<RuleBound>,

    /// Which turns to compact.
    ///
    /// `--from`/`--to` bound the compacted range directly (overriding
    /// `--keep-first`/`--keep-last`); `--first N`/`--last N` compact the first
    /// or last N turns; `--turn N` compacts a single turn, or `--turn A..B` an
    /// inclusive range (e.g. `1..5` is turns 1-5).
    #[command(flatten)]
    range: TurnRange,

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
    /// Optionally accepts text passed to the summarizer as additional context,
    /// e.g. `--summarize "focus on the architectural design"`.
    #[arg(short, long, conflicts_with = "compact")]
    summarize: Option<Option<String>>,

    /// The model to summarize with.
    ///
    /// Accepts a model alias or a full `provider/name` ID, the same values as
    /// `jp query --model`.
    /// Overrides the summarizer model for every rule in this invocation,
    /// including rules that set their own
    /// `conversation.compaction.rules[].summary.model`.
    /// Only affects rules that generate a summary; without one, nothing calls
    /// an LLM.
    #[arg(short = 'm', long)]
    model: Option<String>,

    /// Preview what would change without applying.
    #[arg(long)]
    dry_run: bool,

    /// Remove compaction events from the stream.
    ///
    /// Without a value, removes every compaction event, restoring the raw event
    /// history so the LLM sees all original events.
    /// With a value (`--reset=2`), removes only that compaction event, numbered
    /// as `jp conversation show` lists them.
    /// Mutually exclusive with the policy, range, and DSL flags: `--reset`
    /// undoes compaction, it does not re-compact in the same invocation.
    /// Composes with `--dry-run` to preview the removal.
    #[arg(
        long,
        value_name = "INDEX",
        require_equals = true,
        value_parser = parse_compaction_index,
        conflicts_with_all = [
            "keep_first", "keep_last", "from", "to", "first", "last", "turn",
            "reasoning", "tools", "summarize", "compact", "model",
        ],
    )]
    reset: Option<Option<usize>>,

    /// Compact using an inline DSL rule.
    ///
    /// Mutually exclusive with the dedicated `--reasoning`/`--tools`/
    /// `--summarize` flags above: use either the flags or the DSL, not both.
    /// See `jp query --help` for DSL syntax.
    #[command(flatten)]
    compact_flag: crate::cmd::compact_flag::CompactFlag,
}

impl Compact {
    /// Reject flag combinations clap cannot express.
    ///
    /// `--keep-first N --first M` means "compact the first M turns, minus the
    /// first N" (and `--keep-last`/`--last` the mirror image at the end): with
    /// N greater than M the preserved turns swallow the whole selection, so the
    /// pair is rejected rather than silently selecting nothing.
    /// Only count-based bounds are comparable up front; durations and the other
    /// bound forms resolve against the stream and may legitimately come up
    /// empty.
    fn validate(&self) -> Result<(), String> {
        if let (Some(RuleBound::Turns(keep)), Some(first)) = (&self.keep_first, self.range.first())
            && *keep > first
        {
            return Err(format!(
                "--keep-first {keep} is greater than --first {first}: nothing would remain to \
                 compact"
            ));
        }
        if let (Some(RuleBound::Turns(keep)), Some(last)) = (&self.keep_last, self.range.last())
            && *keep > last
        {
            return Err(format!(
                "--keep-last {keep} is greater than --last {last}: nothing would remain to compact"
            ));
        }
        Ok(())
    }

    /// Returns `true` if any dedicated policy flag is set.
    ///
    /// Policy flags (`--reasoning`/`--tools`/`--summarize`) build a single
    /// ad-hoc rule.
    /// Range flags (`--keep-first`/`--keep-last`/`--from`/`--to`) are
    /// deliberately excluded: they are applied at runtime as range overrides on
    /// the active rules, not as a rule of their own.
    fn has_policy_overrides(&self) -> bool {
        self.reasoning || self.tools.is_some() || self.summarize.is_some()
    }
}

impl Compact {
    /// Resolve the effective compaction rules for this invocation.
    ///
    /// Dedicated policy flags (`--reasoning`/`--tools`/`--summarize`) build one
    /// ad-hoc rule; inline DSL specs (`-k SPEC`) each build one. clap makes the
    /// two mutually exclusive (the policy flags `conflicts_with` the `compact`
    /// flag), so at most one side is ever populated.
    /// These explicit rules replace the configured rules, unless a bare
    /// `--compact` is also present, in which case they are appended to the
    /// configured rules.
    ///
    /// Range flags (`--keep-first`/`--keep-last`/`--from`/`--to`) are NOT
    /// rules: `compact_one` applies them as range overrides on whichever rules
    /// end up active (see `resolve_from`/`resolve_to`), so a range-only
    /// invocation narrows the configured rules instead of replacing them with a
    /// policy-less no-op.
    fn effective_rules(
        &self,
        cfg: &jp_config::AppConfig,
    ) -> Result<Vec<CompactionRuleConfig>, jp_config::ConfigError> {
        let mut explicit: Vec<PartialCompactionRuleConfig> = Vec::new();

        if self.has_policy_overrides() {
            let mut rule = PartialCompactionRuleConfig::default();
            if self.reasoning {
                rule.reasoning = Some(ReasoningMode::Strip);
            }
            rule.tool_calls = self.tools;
            if let Some(context) = &self.summarize {
                rule.summary = Some(PartialSummaryConfig {
                    context: context.clone(),
                    ..PartialSummaryConfig::default()
                });
            }
            explicit.push(rule);
        }

        explicit.extend(self.compact_flag.dsl_rules());

        let explicit = CompactionConfig::finalize_rules(explicit)?;
        let mut rules = crate::cmd::compact_flag::combine_rules(
            &cfg.conversation.compaction.rules,
            self.compact_flag.use_config_rules,
            explicit,
        );

        if self.model.is_some() {
            redirect_summaries_to_assistant_model(&mut rules, cfg);
        }

        Ok(rules)
    }
}

/// Point every rule that names its own summary model at `assistant.model.id`.
///
/// A rule with no `summary.model` already summarizes on the assistant model, so
/// it needs nothing here.
/// A rule that names one would otherwise outrank `--model`, which is applied to
/// `assistant.model`.
/// Only the ID moves: the rule keeps its own summary parameters (max tokens,
/// temperature, ...).
fn redirect_summaries_to_assistant_model(
    rules: &mut [CompactionRuleConfig],
    cfg: &jp_config::AppConfig,
) {
    for summary in rules.iter_mut().filter_map(|rule| rule.summary.as_mut()) {
        if let Some(model) = summary.model.as_mut() {
            model.id = cfg.assistant.model.id.clone();
        }
    }
}

impl IntoPartialAppConfig for Compact {
    fn apply_cli_config(
        &self,
        _: Option<&Workspace>,
        mut partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        apply_model(&mut partial, self.model.as_deref(), merged_config);

        Ok(partial)
    }
}

fn parse_tool_calls_mode(s: &str) -> Result<ToolCallsMode, String> {
    s.parse().map_err(|_| {
        "expected one of: strip (s), strip-requests (sreq), strip-responses (sres), omit (o)"
            .to_string()
    })
}

/// Parse the `--reset=INDEX` value: a 1-based position among the conversation's
/// compaction events.
fn parse_compaction_index(s: &str) -> Result<usize, String> {
    match s.parse() {
        Ok(0) => Err("compaction indices are 1-based; `0` is not a valid index".to_owned()),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("invalid compaction index '{s}'")),
    }
}

/// Look up the compaction event a 1-based `--reset=INDEX` addresses.
///
/// Returns its 0-based position among the stream's compaction events, plus a
/// `turns X..Y` label for the removal message (turn numbers 1-based, as the
/// user sees them elsewhere).
/// An index naming no event is an error rather than a silent no-op, matching
/// how `--turn` treats an out-of-range turn.
fn resolve_reset_index(
    events: &ConversationStream,
    index: usize,
) -> Result<(usize, String), String> {
    let Some(position) = index.checked_sub(1) else {
        return Err("compaction indices are 1-based; `0` is not a valid index".to_owned());
    };

    let Some(compaction) = events.compactions().nth(position) else {
        let count = events.compactions().count();
        return Err(format!(
            "compaction {index} out of range (conversation has {count} compaction event(s))"
        ));
    };

    Ok((
        position,
        format!(
            "turns {}..{}",
            compaction.from_turn + 1,
            compaction.to_turn + 1
        ),
    ))
}

/// Resolve the turn range a single rule would compact.
///
/// `range_stream` is the baseline for resolving bounds, including
/// `AfterLastCompaction` (`--from last-compaction`): it must be the stream as
/// it existed at the start of the invocation, so every rule resolves it against
/// the same compactions and a rule generated earlier in the same invocation
/// doesn't shift the baseline for a later one.
///
/// `overlap_stream` is consulted only to extend summary ranges over partially
/// overlapping summaries; it accumulates the compactions generated so far in
/// this invocation so two summary rules can't be appended unextended.
///
/// For non-summary rules the two streams are interchangeable (only
/// `extend_summary_range` reads `overlap_stream`).
/// Shared by the dry-run preview and the real build so they always agree.
fn resolve_rule_range(
    range_stream: &ConversationStream,
    overlap_stream: &ConversationStream,
    rule: &CompactionRuleConfig,
    from_override: Bound,
    to_override: Bound,
) -> Option<CompactionRange> {
    // A CLI override (`--from`/`--to`/`--keep-first`/`--keep-last`) takes
    // precedence; otherwise fall back to the rule's own bound. Either side
    // resolving to `Empty` means nothing is compacted.
    let from = match from_override {
        Bound::Default => keep_first_to_bound(&rule.keep_first, range_stream),
        other => other,
    };
    let to = match to_override {
        Bound::Default => keep_last_to_bound(&rule.keep_last, range_stream),
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

    let range = resolve_range(range_stream, from, to)?;
    Some(if rule.summary.is_some() {
        extend_summary_range(overlap_stream, range)
    } else {
        range
    })
}

/// Generate the summary text (if any) and assemble a [`Compaction`] for an
/// already-resolved range.
///
/// The summarizer reads the raw events in `events` for the range.
async fn build_compaction_for_range(
    events: &ConversationStream,
    cfg: &jp_config::AppConfig,
    rule: &CompactionRuleConfig,
    range: CompactionRange,
    printer: Option<&jp_printer::Printer>,
) -> crate::Result<Compaction> {
    let summary_text = if rule.summary.is_some() {
        if let Some(printer) = printer {
            printer.println("Generating summary...");
        }
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

    Ok(compaction)
}

/// Build compaction events from the given resolved rules.
///
/// Each rule produces one `Compaction` event.
/// Runtime range overrides (`--from`/`--to`) apply to every rule.
pub(crate) async fn build_compaction_events(
    events: &ConversationStream,
    cfg: &jp_config::AppConfig,
    rules: &[CompactionRuleConfig],
    from_override: Bound,
    to_override: Bound,
    printer: Option<&jp_printer::Printer>,
) -> crate::Result<Vec<Compaction>> {
    // Two distinct baselines:
    //
    // - Range resolution uses the original `events` for every rule, so
    //   `AfterLastCompaction` (`--from last-compaction` / `keep_first =
    //   "last-compaction"`) resolves
    //   against the compactions present at invocation start and applies
    //   uniformly, rather than each rule starting after the previous rule's
    //   freshly generated compaction.
    // - `overlap` accumulates the compactions generated so far, so a later
    //   summary rule's overlap extension sees earlier summaries in this same
    //   invocation and can't be appended unextended.
    let mut overlap = events.clone();
    let mut compactions = Vec::new();
    for rule in rules {
        let Some(range) = resolve_rule_range(
            events,
            &overlap,
            rule,
            from_override.clone(),
            to_override.clone(),
        ) else {
            continue;
        };
        let compaction = build_compaction_for_range(events, cfg, rule, range, printer).await?;
        overlap.add_compaction(compaction.clone());
        compactions.push(compaction);
    }

    Ok(compactions)
}

/// Apply compaction events to the conversation stream.
///
/// Mutation only: callers that want to report the result render their own
/// timeline (see [`timeline_lines`]).
/// The `jp query --compact` path applies silently so compaction details don't
/// clutter the query output.
pub(crate) fn apply_compactions(conv: &ConversationMut, compactions: Vec<Compaction>) {
    for compaction in compactions {
        conv.update_events(|stream| stream.add_compaction(compaction));
    }
}

/// A compacted range plus a short label describing what was done to it.
///
/// `label` is `None` only for a (degenerate) compaction with no policy.
struct TimelineSegment {
    from: usize,
    to: usize,
    label: Option<String>,
    /// Whether this range was already compacted before this invocation.
    ///
    /// Existing compactions are reported factually ("Compacted") even under
    /// `--dry-run`, since they pre-date the previewed run.
    existing: bool,
}

/// Build timeline segments for the compactions about to be applied, spilling
/// each summary to a temp file so the timeline can link to it.
///
/// `conv_id` prefixes the temp-file names so summaries from different
/// conversations don't collide.
fn segments_for_compactions(compactions: &[Compaction], conv_id: &str) -> Vec<TimelineSegment> {
    compactions
        .iter()
        .map(|c| {
            let label = match &c.summary {
                Some(summary) => Some(
                    match write_summary_file(conv_id, c.from_turn, c.to_turn, &summary.summary) {
                        Some(path) => format!("summary: {}", path.display()),
                        None => "summary".to_owned(),
                    },
                ),
                None => compaction_policy_label(c),
            };
            TimelineSegment {
                from: c.from_turn,
                to: c.to_turn,
                label,
                existing: false,
            }
        })
        .collect()
}

/// Build timeline segments for compactions already present at invocation start.
///
/// Without these, the turns they cover would be reported as kept even though
/// the projected conversation still compacts them (most visibly with `--from
/// last`, which starts the new range after the existing compactions).
fn existing_segments(snapshot: &ConversationStream) -> Vec<TimelineSegment> {
    snapshot
        .compactions()
        .map(|c| TimelineSegment {
            from: c.from_turn,
            to: c.to_turn,
            label: Some("already compacted".to_owned()),
            existing: true,
        })
        .collect()
}

/// Write a generated summary to a temp file so the timeline can link to it.
///
/// The summary is also stored durably in the conversation stream; this file is
/// a convenience copy for immediate viewing.
/// Returns `None` (and logs) when the write fails — a missing convenience file
/// must not abort compaction.
fn write_summary_file(conv_id: &str, from: usize, to: usize, summary: &str) -> Option<PathBuf> {
    let path = env::temp_dir().join(format!("{conv_id}-summary-{from}-{to}.md"));
    match fs::write(&path, summary) {
        Ok(()) => Some(path),
        Err(err) => {
            warn!(%err, path = %path.display(), "Failed to write summary file.");
            None
        }
    }
}

/// Build the interleaved kept/compacted timeline lines for one invocation.
///
/// Compactions are sorted by start turn; a kept line is emitted for each gap
/// before, between, and after the compacted ranges.
/// Overlapping ranges collapse naturally — a gap is printed only where no
/// compaction covers it.
/// `dry_run` switches the verbs from "Compacted"/"Kept" to "Would have
/// compacted"/"Would have kept", except for segments already compacted before
/// this run, which always read "Compacted".
fn timeline_lines(segments: &[TimelineSegment], last_turn: usize, dry_run: bool) -> Vec<String> {
    let kept = if dry_run { "Would have kept" } else { "Kept" };

    let mut ordered: Vec<&TimelineSegment> = segments.iter().collect();
    ordered.sort_by_key(|s| s.from);

    let mut lines = Vec::new();
    // Highest turn covered by a compaction so far; `None` before the first.
    let mut covered: Option<usize> = None;
    for segment in ordered {
        let next_kept = covered.map_or(0, |c| c + 1);
        if segment.from > next_kept {
            lines.push(kept_line(kept, next_kept, segment.from - 1));
        }

        // Pre-existing compactions are factual even under `--dry-run`; only this
        // run's new compactions are hypothetical.
        let compacted = if dry_run && !segment.existing {
            "Would have compacted"
        } else {
            "Compacted"
        };

        let count = segment.to - segment.from + 1;
        // Stored indices are 0-based; turn numbers shown to the user are 1-based.
        lines.push(match &segment.label {
            Some(label) => format!(
                "{compacted} turns {}..{} ({count} total, {label}).",
                segment.from + 1,
                segment.to + 1,
            ),
            None => format!(
                "{compacted} turns {}..{} ({count} total).",
                segment.from + 1,
                segment.to + 1,
            ),
        });

        covered = Some(covered.map_or(segment.to, |c| c.max(segment.to)));
    }

    let tail = covered.map_or(0, |c| c + 1);
    if tail <= last_turn {
        lines.push(kept_line(kept, tail, last_turn));
    }

    lines
}

/// Format a single kept line for the inclusive range `[from, to]`.
fn kept_line(verb: &str, from: usize, to: usize) -> String {
    // Stored indices are 0-based; turn numbers shown to the user are 1-based.
    if from == to {
        format!("{verb} turn {}.", from + 1)
    } else {
        format!("{verb} turns {}..{}.", from + 1, to + 1)
    }
}

/// Convert a `keep_first` rule bound to a `from` [`Bound`].
fn keep_first_to_bound(bound: &RuleBound, events: &ConversationStream) -> Bound {
    match bound {
        // "Keep first N" means compaction starts at turn N.
        RuleBound::Turns(n) => Bound::At(RangeBound::Absolute(*n)),
        // `Absolute` is the 1-based user value; the stream is 0-based.
        RuleBound::Absolute(n) => Bound::At(RangeBound::Absolute(n.saturating_sub(1))),
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
        // `Absolute` is the 1-based user value; the stream is 0-based.
        RuleBound::Absolute(n) => Bound::At(RangeBound::Absolute(n.saturating_sub(1))),
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
        self.validate()?;
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
        let mut conv = lock.into_mut();
        let events_snapshot = conv.events().clone();

        if let Some(index) = self.reset {
            return self.run_reset(ctx, &mut conv, &events_snapshot, index);
        }

        // `--last 0` explicitly selects no turns.
        if self.range.is_empty() {
            ctx.printer.println("Nothing to compact.");
            return Ok(());
        }

        // `--turn` names specific turns; an out-of-range endpoint is an error
        // rather than an empty/clamped range (matching `print`).
        let count = events_snapshot.turn_count();
        if let Some(n) = self.range.turn_out_of_range(count) {
            return Err(format!("turn {n} out of range (conversation has {count} turns)").into());
        }

        // The effective rules combine the configured rules with any policy
        // flags / inline DSL (replace, or append under bare `--compact`).
        let rules = self
            .effective_rules(&cfg)
            .map_err(|e| crate::error::Error::Compaction(e.to_string()))?;

        // Range overrides (`--from`/`--to`/`--keep-first`/`--keep-last`) are
        // resolved at runtime (they need the stream for duration and "last"
        // resolution) and apply on top of every active rule.
        let from_override = self.resolve_from(&events_snapshot);
        let to_override = self.resolve_to(&events_snapshot);

        if self.dry_run {
            Self::preview_compaction(ctx, &events_snapshot, &rules, &from_override, &to_override);
            return Ok(());
        }

        let compactions = build_compaction_events(
            &events_snapshot,
            &cfg,
            &rules,
            from_override,
            to_override,
            Some(&ctx.printer),
        )
        .await?;

        if compactions.is_empty() {
            ctx.printer.println("Nothing to compact.");
            return Ok(());
        }

        let last_turn = events_snapshot.turn_count().saturating_sub(1);
        // Carry the pre-existing compactions so their turns aren't reported as
        // kept; the projected conversation still compacts them.
        let mut segments = existing_segments(&events_snapshot);
        segments.extend(segments_for_compactions(
            &compactions,
            &conv.id().to_string(),
        ));
        apply_compactions(&conv, compactions);
        // Write before reporting the timeline, so a failed write is an error
        // rather than a confirmation the user cannot trust.
        conv.flush()?;
        for line in timeline_lines(&segments, last_turn, false) {
            ctx.printer.println(line);
        }

        Ok(())
    }

    /// Handle `--reset[=INDEX]`: remove every compaction event, or just the one
    /// at `index` (1-based, in stream order).
    ///
    /// Under `--dry-run` nothing is mutated and the message describes what
    /// would be removed.
    /// An `index` past the last compaction event is an error rather than a
    /// silent no-op (matching `--turn`).
    fn run_reset(
        &self,
        ctx: &Ctx,
        conv: &mut ConversationMut,
        events: &ConversationStream,
        index: Option<usize>,
    ) -> Output {
        let Some(index) = index else {
            let count = if self.dry_run {
                events.compactions().count()
            } else {
                let removed = conv.update_events(ConversationStream::remove_compactions);
                // Write before reporting, so a failed write is an error rather
                // than a confirmation the user cannot trust.
                conv.flush()?;
                removed
            };

            ctx.printer.println(match (count, self.dry_run) {
                (0, _) => "No compaction events to remove.".to_owned(),
                (count, true) => format!("Would remove {count} compaction event(s)."),
                (count, false) => format!("Removed {count} compaction event(s)."),
            });
            return Ok(());
        };

        let (position, range) = resolve_reset_index(events, index)?;

        if self.dry_run {
            ctx.printer
                .println(format!("Would remove compaction {index} ({range})."));
            return Ok(());
        }

        conv.update_events(|stream| stream.remove_compaction(position));
        // Write before reporting, so a failed write is an error rather than a
        // confirmation the user cannot trust.
        conv.flush()?;
        ctx.printer
            .println(format!("Removed compaction {index} ({range})."));

        Ok(())
    }

    /// Preview the compaction timeline without mutating the conversation.
    ///
    /// Resolves the same per-rule ranges as the real run (minus the summarizer
    /// and the mutation), then prints the dry-run timeline.
    /// Summary rules show a bare `summary` label since no text is generated in
    /// a preview.
    fn preview_compaction(
        ctx: &Ctx,
        events_snapshot: &ConversationStream,
        rules: &[CompactionRuleConfig],
        from_override: &Bound,
        to_override: &Bound,
    ) {
        // Range resolution uses the original snapshot for every rule, while
        // `overlap` accumulates this run's summaries so later summary rules
        // preview the same (possibly extended) ranges as the real run.
        let mut overlap = events_snapshot.clone();
        let mut new_segments = Vec::new();
        for rule in rules {
            let Some(range) = resolve_rule_range(
                events_snapshot,
                &overlap,
                rule,
                from_override.clone(),
                to_override.clone(),
            ) else {
                continue;
            };
            let label = if rule.summary.is_some() {
                Some("summary".to_owned())
            } else {
                compaction_policy_label(&build_mechanical_compaction(
                    range.from_turn,
                    range.to_turn,
                    rule,
                ))
            };
            new_segments.push(TimelineSegment {
                from: range.from_turn,
                to: range.to_turn,
                label,
                existing: false,
            });
            if rule.summary.is_some() {
                overlap.add_compaction(
                    Compaction::new(range.from_turn, range.to_turn).with_summary(SummaryPolicy {
                        summary: String::new(),
                    }),
                );
            }
        }

        if new_segments.is_empty() {
            ctx.printer.println("Nothing to compact.");
            return;
        }

        // Prepend the pre-existing compactions so already-compacted turns aren't
        // previewed as kept; the projected conversation still compacts them.
        let mut segments = existing_segments(events_snapshot);
        segments.extend(new_segments);

        let last_turn = events_snapshot.turn_count().saturating_sub(1);
        for line in timeline_lines(&segments, last_turn, true) {
            ctx.printer.println(line);
        }
    }

    /// Resolve the `from` range override.
    ///
    /// The shared selector (`--from`/`--last`/`--turn`) takes precedence; when
    /// none is set it falls back to `--keep-first`, and to [`Bound::Default`]
    /// when that is also unset.
    /// `--first` is the exception: it composes with `--keep-first`, which then
    /// supplies the start of the range while `--first` caps its end (via
    /// [`resolve_to`]).
    ///
    /// [`resolve_to`]: Self::resolve_to
    fn resolve_from(&self, events: &ConversationStream) -> Bound {
        if let Some(bound) = &self.keep_first
            && self.range.first().is_some()
        {
            return keep_first_to_bound(bound, events);
        }
        match self.range.resolve_from(events) {
            Bound::Default => match &self.keep_first {
                Some(bound) => keep_first_to_bound(bound, events),
                None => Bound::Default,
            },
            other => other,
        }
    }

    /// Resolve the `to` range override.
    ///
    /// The shared selector (`--to`/`--turn`) takes precedence; when none is set
    /// it falls back to `--keep-last`, and to [`Bound::Default`] when that is
    /// also unset.
    /// `--last` is the exception: it composes with `--keep-last`, which then
    /// supplies the end of the range while `--last` sets its start (via
    /// [`resolve_from`]).
    ///
    /// [`resolve_from`]: Self::resolve_from
    fn resolve_to(&self, events: &ConversationStream) -> Bound {
        if let Some(bound) = &self.keep_last
            && self.range.last().is_some()
        {
            return keep_last_to_bound(bound, events);
        }
        match self.range.resolve_to(events) {
            Bound::Default => match &self.keep_last {
                Some(bound) => keep_last_to_bound(bound, events),
                None => Bound::Default,
            },
            other => other,
        }
    }
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;
