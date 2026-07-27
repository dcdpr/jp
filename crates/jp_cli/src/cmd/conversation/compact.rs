use std::{env, fs, path::PathBuf};

use jp_config::{
    PartialAppConfig,
    conversation::compaction::{
        CompactionConfig, CompactionRuleConfig, PartialCompactionRuleConfig, PartialSummaryConfig,
        ReasoningMode, ToolCallsMode,
    },
};
use jp_conversation::{
    Compaction, CompactionRange, ConversationStream, ReasoningPolicy, SummaryPolicy,
    ToolCallPolicy, compaction::extend_summary_range,
};
use jp_workspace::{ConversationHandle, ConversationMut, Workspace};
use tracing::warn;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
        query::apply_model,
        turn_selection::{
            Bound, BoundWindow, TurnSelection, keep_first_bound, keep_last_bound, resolve_window,
        },
    },
    ctx::{Ctx, IntoPartialAppConfig},
    format::compaction_policy_label,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Compact {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Which turns to compact.
    ///
    /// `--from`/`--to` bound the compacted range; `--first N`/`--last N`
    /// compact the first or last N turns (both together compact each window and
    /// skip the middle); `--turn N` compacts a single turn, or `--turn A..B` an
    /// inclusive range (e.g. `1..5` is turns 1-5).
    /// `--keep-first`/`--keep-last` protect turns at either end from whichever
    /// range is selected.
    #[command(flatten)]
    range: TurnSelection,

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

    /// Remove all compaction events from the stream.
    ///
    /// Restores the raw event history so the LLM sees all original events.
    /// Mutually exclusive with the policy, range, and DSL flags: `--reset`
    /// undoes compaction, it does not re-compact in the same invocation.
    /// Composes with `--dry-run` to preview the removal.
    #[arg(
        long,
        conflicts_with_all = [
            "keep_first", "keep_last", "from", "to", "first", "last", "turn",
            "reasoning", "tools", "summarize", "compact", "model",
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

/// Resolve the turn range a single rule would compact within one selected
/// window.
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
    window: &BoundWindow,
    selection: &TurnSelection,
) -> Option<CompactionRange> {
    // The rule's own `keep_first`/`keep_last` fills a side the selection left
    // open, unless the matching CLI keep flag is set: an explicit
    // `--keep-first` replaces the configured bound rather than stacking on top
    // of it.
    let from = match (&window.from, selection.keep_first()) {
        (Bound::Default, None) => keep_first_bound(&rule.keep_first, range_stream),
        (side, _) => side.clone(),
    };
    let to = match (&window.to, selection.keep_last()) {
        (Bound::Default, None) => keep_last_bound(&rule.keep_last, range_stream),
        (side, _) => side.clone(),
    };

    let resolved = resolve_window(&BoundWindow { from, to }, range_stream)?;
    let trimmed = selection.trim(resolved, range_stream)?;
    let range = CompactionRange {
        from_turn: trimmed.from,
        to_turn: trimmed.to,
    };

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
/// Each rule produces one `Compaction` event per selected window, so a disjoint
/// selection (`--first N --last M`) compacts each window separately and leaves
/// the turns between them untouched.
pub(crate) async fn build_compaction_events(
    events: &ConversationStream,
    cfg: &jp_config::AppConfig,
    rules: &[CompactionRuleConfig],
    selection: &TurnSelection,
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
    let windows = selection.windows(events);
    let mut overlap = events.clone();
    let mut compactions = Vec::new();
    for rule in rules {
        for window in &windows {
            let Some(range) = resolve_rule_range(events, &overlap, rule, window, selection) else {
                continue;
            };
            let compaction = build_compaction_for_range(events, cfg, rule, range, printer).await?;
            overlap.add_compaction(compaction.clone());
            compactions.push(compaction);
        }
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
        self.range.validate()?;
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

        self.range.check_turn_range(events_snapshot.turn_count())?;

        // The effective rules combine the configured rules with any policy
        // flags / inline DSL (replace, or append under bare `--compact`).
        let rules = self
            .effective_rules(&cfg)
            .map_err(|e| crate::error::Error::Compaction(e.to_string()))?;

        if self.dry_run {
            Self::preview_compaction(ctx, &events_snapshot, &rules, &self.range);
            return Ok(());
        }

        let compactions = build_compaction_events(
            &events_snapshot,
            &cfg,
            &rules,
            &self.range,
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
        for line in timeline_lines(&segments, last_turn, false) {
            ctx.printer.println(line);
        }

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
        selection: &TurnSelection,
    ) {
        // Range resolution uses the original snapshot for every rule, while
        // `overlap` accumulates this run's summaries so later summary rules
        // preview the same (possibly extended) ranges as the real run.
        let windows = selection.windows(events_snapshot);
        let mut overlap = events_snapshot.clone();
        let mut new_segments = Vec::new();
        for rule in rules {
            for window in &windows {
                let Some(range) =
                    resolve_rule_range(events_snapshot, &overlap, rule, window, selection)
                else {
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
                        Compaction::new(range.from_turn, range.to_turn).with_summary(
                            SummaryPolicy {
                                summary: String::new(),
                            },
                        ),
                    );
                }
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
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;
