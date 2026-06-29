//! Shared turn-range selector for `print` and `compact`.
//!
//! Both commands need to name a subset of turns.
//! This module owns that selector — the `--last`/`--turn`/`--from`/`--to`
//! flags — and the parsing and stream resolution behind them, so a range built
//! for one command means the same thing in the other.
//!
//! Turn positions are 1-based on the CLI and 0-based in the stream; the
//! translation happens here.
//! See `docs/architecture/indexing-conventions.md`.

use std::{str::FromStr as _, time::Duration};

use chrono::{DateTime, Utc};
use jp_conversation::{ConversationStream, RangeBound};

/// A `--from`/`--to` bound before time-based resolution.
#[derive(Debug, Clone)]
pub(crate) enum CliRangeBound {
    /// Already resolved to a `RangeBound` (0-based for the core).
    Resolved(RangeBound),
    /// Duration ago — needs the stream to find the turn.
    Duration(DateTime<Utc>),
}

/// Whether `s` is the most-recent-compaction marker.
///
/// `last-compaction` is canonical; `last` is accepted as a deprecated alias.
fn is_last_compaction(s: &str) -> bool {
    s.eq_ignore_ascii_case("last-compaction") || s.eq_ignore_ascii_case("last")
}

/// Parse a `--from`/`--to` bound.
///
/// Accepts a 1-based turn index, `-N` (offset from the end, `-1` is the last
/// turn), a duration (`5h`), or `last-compaction` (after the most recent
/// compaction; `last` is accepted as a deprecated alias).
pub(crate) fn parse_bound(s: &str) -> Result<CliRangeBound, String> {
    if is_last_compaction(s) {
        return Ok(CliRangeBound::Resolved(RangeBound::AfterLastCompaction));
    }

    // From-end offset. `-1` is the last turn, so `-N` maps to `FromEnd(N - 1)`.
    if let Some(rest) = s.strip_prefix('-')
        && let Ok(n) = rest.parse::<usize>()
    {
        if n == 0 {
            return Err("from-end offsets are 1-based; use `-1` for the last turn".to_owned());
        }
        return Ok(CliRangeBound::Resolved(RangeBound::FromEnd(n - 1)));
    }

    // 1-based user index → 0-based core index.
    if let Ok(n) = s.parse::<usize>() {
        if n == 0 {
            return Err("turn numbers are 1-based; `0` is not a valid turn".to_owned());
        }
        return Ok(CliRangeBound::Resolved(RangeBound::Absolute(n - 1)));
    }

    humantime::Duration::from_str(s)
        .map(|d| CliRangeBound::Duration(Utc::now() - Duration::from(d)))
        .map_err(|e| format!("invalid range bound `{s}`: {e}"))
}

/// Like [`parse_bound`], but rejects the `last-compaction` marker.
///
/// `last-compaction` (the most recent compaction) is only meaningful as a start
/// bound (`--from last-compaction`), so it is not accepted for `--to`.
fn parse_to_bound(s: &str) -> Result<CliRangeBound, String> {
    if is_last_compaction(s) {
        return Err(
            "`last-compaction` is only valid for `--from` (it marks the most recent compaction)"
                .to_owned(),
        );
    }
    parse_bound(s)
}

/// Parse a 1-based turn number for `--turn`, rejecting `0`.
fn parse_one_based(s: &str) -> Result<usize, String> {
    match s.parse::<usize>() {
        Ok(0) => Err("turn numbers are 1-based; `0` is not a valid turn".to_owned()),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("invalid turn number `{s}`")),
    }
}

/// A `--turn` value: a single 1-based turn, or an inclusive 1-based range.
///
/// Either end of a range may be open: `10..` is turn 10 through the end, `..10`
/// is the first 10 turns, and `..` is the whole conversation.
#[derive(Debug, Clone)]
pub(crate) enum TurnSpec {
    /// A single turn.
    Single(usize),
    /// An inclusive range `from..to`.
    /// `None` on either side is open (the start or end of the conversation).
    Range(Option<usize>, Option<usize>),
}

/// Parse a `--turn` value: `N` (a single turn) or a range `A..B`.
///
/// The separator is `..` and both ends are inclusive, matching the compaction
/// DSL (`1..5` is turns 1 through 5).
/// Either end may be omitted: `10..`, `..10`, or `..` (all turns).
fn parse_turn(s: &str) -> Result<TurnSpec, String> {
    if let Some((a, b)) = s.split_once("..") {
        let from = if a.is_empty() {
            None
        } else {
            Some(parse_one_based(a)?)
        };
        let to = if b.is_empty() {
            None
        } else {
            Some(parse_one_based(b)?)
        };
        return Ok(TurnSpec::Range(from, to));
    }
    Ok(TurnSpec::Single(parse_one_based(s)?))
}

/// The resolution of one range bound (`from` or `to`) against a stream.
///
/// Separates "no bound configured for this side" (use the side's default) from
/// "this bound selects no turns" (so the whole selection is empty), which a
/// plain `Option<RangeBound>` conflates.
#[derive(Debug, Clone)]
pub(crate) enum Bound {
    /// No bound configured; the range defaults to the start (`from`) or end
    /// (`to`) of the conversation.
    Default,
    /// The bound resolves to a concrete `RangeBound`.
    At(RangeBound),
    /// The bound falls outside the conversation such that nothing is selected.
    Empty,
}

/// Resolve a `--from <bound>` cutoff: the range starts at the first turn after
/// the cutoff.
///
/// No turn after the cutoff (an old conversation) means an empty selection; a
/// cutoff before the conversation starts selects from the beginning.
pub(crate) fn resolve_cli_from(bound: &CliRangeBound, events: &ConversationStream) -> Bound {
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

/// Resolve a `--to <bound>` cutoff: the range stops at (and includes) the turn
/// active at the cutoff.
///
/// A cutoff preceding the conversation means an empty selection.
pub(crate) fn resolve_cli_to(bound: &CliRangeBound, events: &ConversationStream) -> Bound {
    match bound {
        CliRangeBound::Resolved(b) => Bound::At(b.clone()),
        CliRangeBound::Duration(dt) => match events.turn_at_time(*dt) {
            Some(turn) => Bound::At(RangeBound::Absolute(turn.index())),
            None => Bound::Empty,
        },
    }
}

/// A positive turn-range selector shared by `print` and `compact`.
///
/// Names the turns a command acts on.
/// The selectors are mutually constrained so only one way of expressing the
/// range is given at a time.
#[derive(Debug, Clone, Default, clap::Args)]
pub(crate) struct TurnRange {
    /// Select the first N turns.
    /// Without a value, selects the first turn.
    #[arg(long, num_args = 0..=1, default_missing_value = "1", conflicts_with_all = ["last", "turn", "from", "to"])]
    first: Option<usize>,

    /// Select the last N turns.
    /// Without a value, selects the last turn.
    #[arg(long, num_args = 0..=1, default_missing_value = "1", conflicts_with_all = ["first", "turn", "from", "to"])]
    last: Option<usize>,

    /// Select turns by number (1-based): a single turn (`3`), an inclusive
    /// range (`1..5`), or an open range like `10..` (turn 10 onward), `..10`
    /// (the first 10), or `..` (all).
    /// Stable across new turns.
    #[arg(long, value_parser = parse_turn, conflicts_with_all = ["first", "last", "from", "to"])]
    turn: Option<TurnSpec>,

    /// Start of the range: a 1-based turn index, `-N` from the end, a duration
    /// (e.g.
    /// `5h`), or `last-compaction` (after the most recent compaction).
    #[arg(long, value_parser = parse_bound)]
    from: Option<CliRangeBound>,

    /// End of the range: a 1-based turn index, `-N` from the end, or a duration
    /// (e.g.
    /// `2h`).
    #[arg(long, value_parser = parse_to_bound)]
    to: Option<CliRangeBound>,
}

impl TurnRange {
    /// Build a selector from explicit `--last`/`--turn` values.
    #[cfg(test)]
    pub(crate) fn from_last_turn(last: Option<usize>, turn: Option<usize>) -> Self {
        Self {
            first: None,
            last,
            turn: turn.map(TurnSpec::Single),
            from: None,
            to: None,
        }
    }

    /// The first `--turn` endpoint outside `1..=count`, if any.
    ///
    /// `--turn` names specific turns, so an endpoint past the conversation is
    /// an error rather than a clamped selection (unlike `--first`/`--last`).
    pub(crate) fn turn_out_of_range(&self, count: usize) -> Option<usize> {
        let oob = |n: usize| n == 0 || n > count;
        let ends = match self.turn.as_ref()? {
            TurnSpec::Single(n) => [Some(*n), None],
            TurnSpec::Range(a, b) => [*a, *b],
        };
        ends.into_iter().flatten().find(|&n| oob(n))
    }

    /// Whether the selector explicitly names an empty range (`--last 0` or
    /// `--first 0`).
    pub(crate) fn is_empty(&self) -> bool {
        self.last == Some(0) || self.first == Some(0)
    }

    /// The `from` bound as a CLI bound, folding the positive selectors.
    ///
    /// `--first`/`--last`/`--turn` are complete selectors that set both bounds;
    /// `--from` is the explicit start of a range.
    fn cli_from_bound(&self) -> Option<CliRangeBound> {
        if let Some(spec) = &self.turn {
            let bound = match spec {
                TurnSpec::Single(n) => RangeBound::Absolute(n.saturating_sub(1)),
                // Open start (`--turn ..B`) begins at the first turn.
                TurnSpec::Range(Some(a), _) => RangeBound::Absolute(a.saturating_sub(1)),
                TurnSpec::Range(None, _) => RangeBound::Absolute(0),
            };
            return Some(CliRangeBound::Resolved(bound));
        }
        if let Some(n) = self.last {
            return Some(CliRangeBound::Resolved(RangeBound::FromEnd(
                n.saturating_sub(1),
            )));
        }
        if self.first.is_some() {
            // `--first N` starts at the first turn.
            return Some(CliRangeBound::Resolved(RangeBound::Absolute(0)));
        }
        self.from.clone()
    }

    /// The `to` bound as a CLI bound, folding the positive selectors.
    ///
    /// `--first`/`--last`/`--turn` are complete selectors that set both bounds;
    /// `--to` is the explicit end of a range.
    fn cli_to_bound(&self) -> Option<CliRangeBound> {
        if let Some(spec) = &self.turn {
            let bound = match spec {
                TurnSpec::Single(n) => RangeBound::Absolute(n.saturating_sub(1)),
                // Open end (`--turn A..`) runs through the last turn.
                TurnSpec::Range(_, Some(b)) => RangeBound::Absolute(b.saturating_sub(1)),
                TurnSpec::Range(_, None) => RangeBound::FromEnd(0),
            };
            return Some(CliRangeBound::Resolved(bound));
        }
        if self.last.is_some() {
            // `--last N` ends at the last turn.
            return Some(CliRangeBound::Resolved(RangeBound::FromEnd(0)));
        }
        if let Some(n) = self.first {
            return Some(CliRangeBound::Resolved(RangeBound::Absolute(
                n.saturating_sub(1),
            )));
        }
        self.to.clone()
    }

    /// Resolve the `from` override against the stream.
    /// Returns [`Bound::Default`] when no `from`-affecting flag is set.
    pub(crate) fn resolve_from(&self, events: &ConversationStream) -> Bound {
        match self.cli_from_bound() {
            Some(bound) => resolve_cli_from(&bound, events),
            None => Bound::Default,
        }
    }

    /// Resolve the `to` override against the stream.
    /// Returns [`Bound::Default`] when no `to`-affecting flag is set.
    pub(crate) fn resolve_to(&self, events: &ConversationStream) -> Bound {
        match self.cli_to_bound() {
            Some(bound) => resolve_cli_to(&bound, events),
            None => Bound::Default,
        }
    }
}

#[cfg(test)]
#[path = "turn_range_tests.rs"]
mod tests;
