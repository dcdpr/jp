//! Shared `--compact` / `-k` flag for compaction across commands.
//!
//! Used by `query`, `fork`, and `compact`.
//! Supports bare `--compact` (apply config rules) and `--compact=SPEC` (inline
//! DSL rules).

use std::str::FromStr;

use clap::{Arg, ArgAction, ArgMatches, Command};
use jp_config::{
    PartialAppConfig,
    conversation::compaction::{
        PartialCompactionConfig, PartialCompactionRuleConfig, PartialSummaryConfig, ReasoningMode,
        RuleBound, ToolCallsMode,
    },
    types::vec::MergeableVec,
};

/// Shared compaction flag that can be embedded in any command.
///
/// Supports two forms:
///
/// - `--compact` (bare): apply compaction rules from the resolved config.
/// - `--compact=SPEC` (with value): apply an inline DSL rule.
///
/// Both compose: bare `--compact` includes config rules, each `--compact=SPEC`
/// adds a DSL rule.
/// When only specs are present (no bare `--compact`), config rules are not
/// included.
#[derive(Debug, Default)]
pub(crate) struct CompactFlag {
    /// True if bare `--compact` (no value) was specified.
    pub use_config_rules: bool,
    /// DSL specs from `--compact=SPEC` values.
    pub specs: Vec<CompactSpec>,
}

impl CompactFlag {
    /// Whether compaction should be applied at all.
    pub fn should_compact(&self) -> bool {
        self.use_config_rules || !self.specs.is_empty()
    }

    /// The inline DSL specs converted to partial compaction rules.
    pub(crate) fn dsl_rules(&self) -> Vec<PartialCompactionRuleConfig> {
        self.specs
            .iter()
            .map(CompactSpec::to_partial_rule)
            .collect()
    }

    /// Apply DSL specs to the config partial.
    ///
    /// - If only specs (no bare `--compact`): replace the rules array.
    /// - If bare `--compact` + specs: append DSL rules to the config rules.
    /// - If bare `--compact` only: leave config unchanged (rules apply as-is).
    pub fn apply_to_config(&self, partial: &mut PartialAppConfig) {
        let rules = self.dsl_rules();
        if rules.is_empty() {
            return;
        }

        if self.use_config_rules {
            append_config_rules(partial, rules);
        } else {
            partial.conversation.compaction.rules = MergeableVec::Vec(rules);
        }
    }
}

/// Append `rules` to the partial's compaction rules under bare `--compact`
/// ("config rules plus these rules") semantics.
///
/// The config rules are not materialized until finalize, so appending to an
/// otherwise-empty list would let `PartialCompactionConfig::fill_from` treat
/// the result as user-supplied and skip the built-in default rule.
/// Seed the built-in defaults first when the config provides none.
pub(crate) fn append_config_rules(
    partial: &mut PartialAppConfig,
    rules: Vec<PartialCompactionRuleConfig>,
) {
    if partial.conversation.compaction.rules.is_empty() {
        partial.conversation.compaction.rules =
            MergeableVec::Vec(PartialCompactionConfig::builtin_rules());
    }
    partial.conversation.compaction.rules.extend(rules);
}

impl clap::Args for CompactFlag {
    fn augment_args(cmd: Command) -> Command {
        cmd.arg(
            Arg::new("compact")
                .short('k')
                .long("compact")
                .help("Run conversation compaction rules")
                .long_help(
                    "Compact the conversation.\n\nWithout a value, applies the compaction rules \
                     from the resolved configuration.\n\nWith a DSL value (e.g. \
                     `--compact=s:..-3`), applies an inline compaction rule. Multiple \
                     `--compact=SPEC` flags add multiple rules.\n\nBoth forms compose: bare \
                     `--compact` includes config rules, each `--compact=SPEC` adds a DSL \
                     rule.\n\nDSL format: POLICIES[:RANGE]\n\nPolicies are joined with `+`:\n- \
                     `r` / `reasoning`: strip reasoning blocks\n- `s` / `summarize`: generate an \
                     LLM summary\n- `t` / `tools` (or `t=MODE`): strip tool calls; bare strips \
                     both, or MODE is one of `strip`/`s`, `strip-requests`/`sreq`, \
                     `strip-responses`/`sres`, `omit`/`o`\n\nRange: FROM..TO, single number, or \
                     .. for all\n\nExamples: s:..-3, r+t, t=sreq:5..-3, r:-20",
                )
                .action(ArgAction::Append)
                .num_args(0..=1)
                .default_missing_value(""),
        )
    }

    fn augment_args_for_update(cmd: Command) -> Command {
        Self::augment_args(cmd)
    }
}

impl clap::FromArgMatches for CompactFlag {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let values: Vec<String> = matches
            .get_many("compact")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();

        let mut flag = CompactFlag::default();
        for val in values {
            if val.is_empty() {
                flag.use_config_rules = true;
            } else {
                let spec = val.parse::<CompactSpec>().map_err(|e| {
                    clap::Error::raw(
                        clap::error::ErrorKind::InvalidValue,
                        format!("invalid compact spec '{val}': {e}\n"),
                    )
                })?;
                flag.specs.push(spec);
            }
        }

        Ok(flag)
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

// ── DSL types ───────────────────────────────────────────────────────────────

/// A parsed compaction DSL spec: `POLICIES[:RANGE]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactSpec {
    pub reasoning: bool,
    /// `None` = no tool-call policy.
    /// The mode mirrors the `--tools` flag.
    pub tools: Option<ToolCallsMode>,
    pub summarize: bool,
    /// `None` = use config defaults for range.
    pub range: Option<DslRange>,
}

/// A parsed DSL range, Python-slice style.
///
/// Each bound is an absolute turn index (positive in the DSL) or a from-end
/// offset (negative).
/// `None` means that end is open (the start or the end of the conversation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DslRange {
    /// Left bound (compaction start).
    /// `None` = start of the conversation.
    pub from: Option<RuleBound>,
    /// Right bound (compaction end).
    /// `None` = end of the conversation.
    pub to: Option<RuleBound>,
}

impl CompactSpec {
    fn to_partial_rule(&self) -> PartialCompactionRuleConfig {
        let mut rule = PartialCompactionRuleConfig::default();

        if self.reasoning {
            rule.reasoning = Some(ReasoningMode::Strip);
        }
        rule.tool_calls = self.tools;
        if self.summarize {
            rule.summary = Some(PartialSummaryConfig::default());
        }

        if let Some(range) = &self.range {
            // Open ends map to start / end: `Absolute(0)` is turn 0, `FromEnd(0)`
            // is the last turn.
            rule.keep_first = Some(range.from.clone().unwrap_or(RuleBound::Absolute(0)));
            rule.keep_last = Some(range.to.clone().unwrap_or(RuleBound::FromEnd(0)));
        }

        rule
    }
}

impl FromStr for CompactSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (policies_str, range_str) = match s.split_once(':') {
            Some((p, r)) => (p, Some(r)),
            None => (s, None),
        };

        let mut reasoning = false;
        let mut tools: Option<ToolCallsMode> = None;
        let mut summarize = false;

        for policy in policies_str.split('+') {
            let policy = policy.trim();
            let (key, value) = match policy.split_once('=') {
                Some((k, v)) => (k.trim(), Some(v.trim())),
                None => (policy, None),
            };

            match key {
                "r" | "reasoning" => {
                    if value.is_some() {
                        return Err("`reasoning` does not take a value".into());
                    }
                    reasoning = true;
                }
                "s" | "summarize" => {
                    if value.is_some() {
                        return Err("`summarize` does not take a value".into());
                    }
                    summarize = true;
                }
                "t" | "tools" => {
                    tools = Some(match value {
                        Some(v) => v.parse().map_err(|e| format!("{e}"))?,
                        // Bare `t` mirrors `--tools` without a value.
                        None => ToolCallsMode::Strip,
                    });
                }
                "" => return Err("empty policy".into()),
                other => return Err(format!("unknown policy '{other}'")),
            }
        }

        if !reasoning && tools.is_none() && !summarize {
            return Err("at least one policy required (r, t=MODE, s)".into());
        }

        let range = range_str.map(parse_dsl_range).transpose()?;

        Ok(CompactSpec {
            reasoning,
            tools,
            summarize,
            range,
        })
    }
}

/// Parse one DSL range bound: a positive integer is an absolute turn index, a
/// negative integer is an offset from the end.
fn parse_dsl_bound(s: &str) -> Result<RuleBound, String> {
    if let Some(rest) = s.strip_prefix('-') {
        let n = rest
            .parse()
            .map_err(|_| format!("invalid bound '-{rest}'"))?;
        Ok(RuleBound::FromEnd(n))
    } else {
        let n = s.parse().map_err(|_| format!("invalid bound '{s}'"))?;
        Ok(RuleBound::Absolute(n))
    }
}

fn parse_dsl_range(s: &str) -> Result<DslRange, String> {
    // Explicit range: FROM..TO (either side may be empty). Both ends are
    // Python-slice style: positive = absolute turn, negative = from the end.
    if let Some((left, right)) = s.split_once("..") {
        let from = if left.is_empty() {
            None
        } else {
            Some(parse_dsl_bound(left)?)
        };
        let to = if right.is_empty() {
            None
        } else {
            Some(parse_dsl_bound(right)?)
        };
        return Ok(DslRange { from, to });
    }

    // Single-number shorthand: positive `N` = `N..` (keep first N), negative
    // `-N` = `..-N` (keep last N).
    match parse_dsl_bound(s)? {
        bound @ RuleBound::FromEnd(_) => Ok(DslRange {
            from: None,
            to: Some(bound),
        }),
        bound => Ok(DslRange {
            from: Some(bound),
            to: None,
        }),
    }
}

#[cfg(test)]
#[path = "compact_flag_tests.rs"]
mod tests;
