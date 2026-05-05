//! Shared `--compact` / `-k` flag for compaction across commands.
//!
//! Used by `query`, `fork`, and `compact`. Supports bare `--compact` (apply
//! config rules) and `--compact=SPEC` (inline DSL rules).

use std::str::FromStr;

use clap::{Arg, ArgAction, ArgMatches, Command};
use jp_config::{
    PartialAppConfig,
    conversation::compaction::{
        PartialCompactionRuleConfig, PartialSummaryConfig, ReasoningMode, RuleBound, ToolCallsMode,
    },
    types::vec::MergeableVec,
};

/// Shared compaction flag that can be embedded in any command.
///
/// Supports two forms:
/// - `--compact` (bare): apply compaction rules from the resolved config.
/// - `--compact=SPEC` (with value): apply an inline DSL rule.
///
/// Both compose: bare `--compact` includes config rules, each `--compact=SPEC`
/// adds a DSL rule. When only specs are present (no bare `--compact`), config
/// rules are not included.
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

    /// Apply DSL specs to the config partial.
    ///
    /// - If only specs (no bare `--compact`): replace the rules array.
    /// - If bare `--compact` + specs: append DSL rules to existing config rules.
    /// - If bare `--compact` only: leave config unchanged (rules apply as-is).
    pub fn apply_to_config(&self, partial: &mut PartialAppConfig) {
        if self.specs.is_empty() {
            return;
        }

        let rules: Vec<PartialCompactionRuleConfig> = self
            .specs
            .iter()
            .map(CompactSpec::to_partial_rule)
            .collect();

        if self.use_config_rules {
            partial.conversation.compaction.rules.extend(rules);
        } else {
            partial.conversation.compaction.rules = MergeableVec::Vec(rules);
        }
    }
}

impl clap::Args for CompactFlag {
    fn augment_args(cmd: Command) -> Command {
        cmd.arg(
            Arg::new("compact")
                .short('k')
                .long("compact")
                .help("Compact the conversation before proceeding")
                .long_help(
                    "Compact the conversation.\n\nWithout a value, applies the compaction rules \
                     from the resolved configuration.\n\nWith a DSL value (e.g. \
                     `--compact=s:..-3`), applies an inline compaction rule. Multiple \
                     `--compact=SPEC` flags add multiple rules.\n\nBoth forms compose: bare \
                     `--compact` includes config rules, each `--compact=SPEC` adds a DSL \
                     rule.\n\nDSL format: POLICIES[:RANGE]\nPolicies: r (reasoning), t (tools), s \
                     (summarize), joined with +\nRange: FROM..TO, single number, or .. for \
                     all\nExamples: s:..-3, r+t, s:5..-3, r:-20",
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
    pub tools: bool,
    pub summarize: bool,
    /// `None` = use config defaults for range.
    pub range: Option<DslRange>,
}

/// A parsed DSL range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DslRange {
    /// Left bound: turns to preserve at the start. `None` = 0.
    pub keep_first: Option<usize>,
    /// Right bound: turns to preserve at the end. `None` = 0.
    pub keep_last: Option<usize>,
}

impl CompactSpec {
    fn to_partial_rule(&self) -> PartialCompactionRuleConfig {
        let mut rule = PartialCompactionRuleConfig::default();

        if self.reasoning {
            rule.reasoning = Some(ReasoningMode::Strip);
        }
        if self.tools {
            rule.tool_calls = Some(ToolCallsMode::Strip);
        }
        if self.summarize {
            rule.summary = Some(PartialSummaryConfig::default());
        }

        if let Some(range) = &self.range {
            rule.keep_first = Some(RuleBound::Turns(range.keep_first.unwrap_or(0)));
            rule.keep_last = Some(RuleBound::Turns(range.keep_last.unwrap_or(0)));
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
        let mut tools = false;
        let mut summarize = false;

        for policy in policies_str.split('+') {
            match policy.trim() {
                "r" | "reasoning" => reasoning = true,
                "t" | "tools" => tools = true,
                "s" | "summarize" => summarize = true,
                "" => return Err("empty policy".into()),
                other => return Err(format!("unknown policy '{other}'")),
            }
        }

        if !reasoning && !tools && !summarize {
            return Err("at least one policy required (r, t, s)".into());
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

fn parse_dsl_range(s: &str) -> Result<DslRange, String> {
    // Full range: FROM..TO
    if let Some((left, right)) = s.split_once("..") {
        let keep_first = if left.is_empty() {
            None
        } else {
            let n: usize = left
                .parse()
                .map_err(|_| format!("invalid left bound '{left}'"))?;
            Some(n)
        };

        let keep_last = if right.is_empty() {
            None
        } else if let Some(rest) = right.strip_prefix('-') {
            let n: usize = rest
                .parse()
                .map_err(|_| format!("invalid right bound '-{rest}'"))?;
            Some(n)
        } else {
            return Err(format!(
                "right bound must be negative (from end), got '{right}'"
            ));
        };

        return Ok(DslRange {
            keep_first,
            keep_last,
        });
    }

    // Single number shorthand
    if let Some(rest) = s.strip_prefix('-') {
        let n: usize = rest
            .parse()
            .map_err(|_| format!("invalid range '-{rest}'"))?;
        Ok(DslRange {
            keep_first: None,
            keep_last: Some(n),
        })
    } else {
        let n: usize = s.parse().map_err(|_| format!("invalid range '{s}'"))?;
        Ok(DslRange {
            keep_first: Some(n),
            keep_last: None,
        })
    }
}

#[cfg(test)]
#[path = "compact_flag_tests.rs"]
mod tests;
