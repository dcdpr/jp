//! Shared turn selection for commands that act on a subset of a conversation's
//! turns.
//!
//! [`TurnSelection`] owns the `--from`/`--to`/`--turn`/`--first`/`--last`/
//! `--keep-first`/`--keep-last` flags and the resolution behind them, so a
//! selection built for one command means the same thing in another.
//!
//! Resolution produces a [`TurnSet`]: an ordered, non-overlapping list of
//! inclusive 0-based turn windows.
//! Most selections are a single window; `--first N --last M` produces two,
//! skipping the turns in between.
//!
//! A bound never splits a turn.
//! The two ends resolve a time value differently: `--from <time>` starts at the
//! first turn to *begin* after the cutoff, while `--to <time>` ends at (and
//! includes) the turn that was running at it.
//!
//! Turn positions are 1-based on the CLI and 0-based in the stream; the
//! translation happens here.
//! See `docs/architecture/indexing-conventions.md`.

use std::{str::FromStr as _, time::Duration};

use chrono::{DateTime, NaiveDate, Utc};
use jp_config::conversation::compaction::RuleBound;
use jp_conversation::{
    ConversationStream, RangeBound, compaction::resolve_range, stream::TurnOrigin,
};

/// A `--from`/`--to` bound before stream resolution.
#[derive(Debug, Clone)]
pub(crate) enum CliRangeBound {
    /// A position, already expressed as a `RangeBound` (0-based for the core).
    Resolved(RangeBound),
    /// An instant — needs the stream to find the turn it falls in.
    At(DateTime<Utc>),
}

/// Whether `s` is the most-recent-compaction marker.
///
/// `last-compaction` is canonical; `last` is accepted as a deprecated alias.
fn is_last_compaction(s: &str) -> bool {
    s.eq_ignore_ascii_case("last-compaction") || s.eq_ignore_ascii_case("last")
}

/// Parse a `--from`/`--to` bound.
///
/// Accepts a 1-based turn number, `-N` (offset from the end, `-1` is the last
/// turn), a relative duration (`5h`), an RFC 3339 timestamp, a `YYYY-MM-DD`
/// date, or `last-compaction` (the turn after the most recent compaction;
/// `last` is accepted as a deprecated alias).
///
/// Integer forms are tried before the time forms, so a bare number is always a
/// turn number: `2026` is turn 2026, never the year.
fn parse_bound(s: &str) -> Result<CliRangeBound, String> {
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

    if let Ok(d) = humantime::Duration::from_str(s) {
        return Ok(CliRangeBound::At(Utc::now() - Duration::from(d)));
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(CliRangeBound::At(dt.with_timezone(&Utc)));
    }

    // Date-only, interpreted as midnight UTC.
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = date
            .and_hms_opt(0, 0, 0)
            .expect("midnight is valid")
            .and_utc();
        return Ok(CliRangeBound::At(dt));
    }

    Err(format!(
        "invalid turn bound `{s}`: expected a turn number (`3`), a from-end offset (`-1`), a \
         duration (`5h`), a date (`2026-01-01`), an RFC 3339 timestamp, or `last-compaction`"
    ))
}

/// Like [`parse_bound`], but rejects the `last-compaction` marker.
///
/// `last-compaction` (the turn after the most recent compaction) is only
/// meaningful as a start bound (`--from last-compaction`), so it is not
/// accepted for `--to`.
fn parse_to_bound(s: &str) -> Result<CliRangeBound, String> {
    if is_last_compaction(s) {
        return Err(
            "`last-compaction` is only valid for `--from` (it marks the most recent compaction)"
                .to_owned(),
        );
    }
    parse_bound(s)
}

/// Parse a `--keep-last` bound, rejecting the `last-compaction` marker.
///
/// `keep_last_bound` maps `AfterLastCompaction` to [`Bound::Default`], which
/// leaves the end of the selection untrimmed.
/// Accepting the marker here would make an explicit `--keep-last
/// last-compaction` a silent no-op, and in `compact` it would additionally
/// suppress the rule's own configured `keep_last` — compacting *more* than
/// omitting the flag.
/// `--keep-first last-compaction` is meaningful and stays accepted.
fn parse_keep_last(s: &str) -> Result<RuleBound, String> {
    let bound = RuleBound::from_str(s).map_err(|e| e.to_string())?;
    if bound == RuleBound::AfterLastCompaction {
        return Err(
            "`last-compaction` is only valid for `--from` and `--keep-first` (it marks the most \
             recent compaction)"
                .to_owned(),
        );
    }
    Ok(bound)
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

/// The resolution of one window bound (`from` or `to`) against a stream.
///
/// Separates "this side is unconstrained" (so a caller with its own default for
/// that side may fill it in) from "this bound selects no turns" (so the window
/// is empty), which a plain `Option<RangeBound>` conflates.
#[derive(Debug, Clone)]
pub(crate) enum Bound {
    /// Unconstrained; the window extends to the start (`from`) or end (`to`) of
    /// the conversation unless a caller supplies its own default.
    Default,
    /// The bound resolves to a concrete `RangeBound`.
    At(RangeBound),
    /// The bound falls outside the conversation such that nothing is selected.
    Empty,
}

/// One selected window, before keep-trimming, with unresolved bounds.
#[derive(Debug, Clone)]
pub(crate) struct BoundWindow {
    pub from: Bound,
    pub to: Bound,
}

/// An inclusive, 0-based range of turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnWindow {
    pub from: usize,
    pub to: usize,
}

/// The turns a command acts on: ordered, non-overlapping, inclusive, 0-based.
///
/// Empty when the selection names no turn — an out-of-range bound, `--first
/// 0`, keep flags that protect everything selected, or a conversation with no
/// turns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnSet(Vec<TurnWindow>);

impl TurnSet {
    /// Sort and coalesce windows so no turn is selected twice.
    ///
    /// Adjacent windows merge as well as overlapping ones, so `--first 2 --last
    /// 2` on a 4-turn conversation is a single window rather than two abutting
    /// ones.
    fn new(mut windows: Vec<TurnWindow>) -> Self {
        windows.sort_by_key(|w| (w.from, w.to));

        let mut merged: Vec<TurnWindow> = Vec::with_capacity(windows.len());
        for window in windows {
            match merged.last_mut() {
                Some(last) if window.from <= last.to.saturating_add(1) => {
                    last.to = last.to.max(window.to);
                }
                _ => merged.push(window),
            }
        }
        Self(merged)
    }

    /// Whether the selection names no turn at all.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The selected windows, in ascending turn order.
    #[cfg(test)]
    pub(crate) fn windows(&self) -> &[TurnWindow] {
        &self.0
    }

    /// Whether the 0-based turn `index` falls in any window.
    pub(crate) fn contains(&self, index: usize) -> bool {
        self.0.iter().any(|w| w.from <= index && index <= w.to)
    }

    /// Whether any window overlaps the raw turns a projected turn stands for.
    ///
    /// A summary turn stands in for a range, so it is selected when the
    /// selection touches any turn it replaces.
    pub(crate) fn overlaps_origin(&self, origin: TurnOrigin) -> bool {
        self.0.iter().any(|w| origin.overlaps(w.from, w.to))
    }
}

/// Resolve a `--from <bound>` cutoff to a `Bound`.
///
/// A time bound starts the window at the first turn to *begin* after the
/// cutoff, so `--from 5h` selects the turns started in the last five hours
/// rather than also pulling in the older turn that was still running at the
/// cutoff.
/// A cutoff before the conversation starts selects from the beginning.
fn resolve_cli_from(bound: &CliRangeBound, events: &ConversationStream) -> Bound {
    match bound {
        CliRangeBound::Resolved(b) => Bound::At(b.clone()),
        CliRangeBound::At(dt) => match events.turn_at_time(*dt) {
            Some(turn) => Bound::At(RangeBound::Absolute(turn.index() + 1)),
            None => Bound::At(RangeBound::Absolute(0)),
        },
    }
}

/// Resolve a `--to <bound>` cutoff to a `Bound`.
///
/// A time bound ends the window at (and includes) the turn it falls in; a
/// cutoff preceding the conversation selects nothing.
fn resolve_cli_to(bound: &CliRangeBound, events: &ConversationStream) -> Bound {
    match bound {
        CliRangeBound::Resolved(b) => Bound::At(b.clone()),
        CliRangeBound::At(dt) => match events.turn_at_time(*dt) {
            Some(turn) => Bound::At(RangeBound::Absolute(turn.index())),
            None => Bound::Empty,
        },
    }
}

/// Resolve a window's bounds to concrete turn indices.
///
/// `None` when either side is empty, or when the two bounds cross so the window
/// names no turn.
pub(crate) fn resolve_window(
    window: &BoundWindow,
    events: &ConversationStream,
) -> Option<TurnWindow> {
    let from = match &window.from {
        Bound::Empty => return None,
        Bound::Default => None,
        Bound::At(b) => Some(b.clone()),
    };
    let to = match &window.to {
        Bound::Empty => return None,
        Bound::Default => None,
        Bound::At(b) => Some(b.clone()),
    };

    resolve_range(events, from, to).map(|r| TurnWindow {
        from: r.from_turn,
        to: r.to_turn,
    })
}

/// Resolve a start bound to a concrete 0-based turn index.
///
/// Routed through `resolve_range` so the asymmetric clamping rules for the two
/// ends (a start past the last turn selects nothing; an end is clamped onto the
/// last turn) are defined in exactly one place.
fn from_index(bound: RangeBound, events: &ConversationStream) -> Option<usize> {
    resolve_range(events, Some(bound), None).map(|r| r.from_turn)
}

/// Resolve an end bound to a concrete 0-based turn index.
fn to_index(bound: RangeBound, events: &ConversationStream) -> Option<usize> {
    resolve_range(events, None, Some(bound)).map(|r| r.to_turn)
}

/// Convert a `keep_first` bound to the first turn it leaves unprotected.
pub(crate) fn keep_first_bound(bound: &RuleBound, events: &ConversationStream) -> Bound {
    match bound {
        // "Keep first N" means the first unprotected turn is N.
        RuleBound::Turns(n) => Bound::At(RangeBound::Absolute(*n)),
        // `Absolute` is the 1-based user value; the stream is 0-based.
        RuleBound::Absolute(n) => Bound::At(RangeBound::Absolute(n.saturating_sub(1))),
        RuleBound::FromEnd(n) => Bound::At(RangeBound::FromEnd(*n)),
        RuleBound::Duration(d) => {
            // Protect the opening `d` window: the first unprotected turn is the
            // one after `conversation_start + d`. A window covering the whole
            // conversation protects everything.
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

/// Convert a `keep_last` bound to the last turn it leaves unprotected.
pub(crate) fn keep_last_bound(bound: &RuleBound, events: &ConversationStream) -> Bound {
    match bound {
        // "Keep last N" means the last unprotected turn is N turns before the
        // end.
        RuleBound::Turns(n) | RuleBound::FromEnd(n) => Bound::At(RangeBound::FromEnd(*n)),
        // `Absolute` is the 1-based user value; the stream is 0-based.
        RuleBound::Absolute(n) => Bound::At(RangeBound::Absolute(n.saturating_sub(1))),
        RuleBound::Duration(d) => {
            // Protect the trailing `d` window. A window covering the whole
            // conversation protects everything.
            match events.turn_at_time(Utc::now() - *d) {
                Some(turn) => Bound::At(RangeBound::Absolute(turn.index())),
                None => Bound::Empty,
            }
        }
        RuleBound::AfterLastCompaction => Bound::Default,
    }
}

/// The turns a command acts on.
///
/// One of `--from`/`--to`, `--turn`, or `--first`/`--last` names the base
/// selection (the whole conversation when none is given); `--keep-first` and
/// `--keep-last` then protect turns at either end from it.
#[derive(Debug, Clone, Default, clap::Args)]
pub(crate) struct TurnSelection {
    /// Select the first N turns.
    /// Without a value, selects the first turn.
    ///
    /// Composes with `--last`: the pair selects the leading and trailing
    /// windows and skips the turns in between.
    #[arg(long, short = 'f', num_args = 0..=1, default_missing_value = "1", conflicts_with_all = ["turn", "from", "to"])]
    first: Option<usize>,

    /// Select the last N turns.
    /// Without a value, selects the last turn.
    ///
    /// Composes with `--first`: the pair selects the leading and trailing
    /// windows and skips the turns in between.
    #[arg(long, short = 'l', num_args = 0..=1, default_missing_value = "1", conflicts_with_all = ["turn", "from", "to"])]
    last: Option<usize>,

    /// Select turns by number (1-based): a single turn (`3`), an inclusive
    /// range (`1..5`), or an open range like `10..` (turn 10 onward), `..10`
    /// (the first 10), or `..` (all).
    /// Stable across new turns.
    #[arg(long, value_parser = parse_turn, conflicts_with_all = ["first", "last", "from", "to"])]
    turn: Option<TurnSpec>,

    /// Start of the range, inclusive.
    ///
    /// Accepts a 1-based turn number (`3`), a from-end offset (`-1` is the last
    /// turn), a relative duration (`5h`), a date (`2026-01-01`), an RFC 3339
    /// timestamp, or `last-compaction` (the turn after the most recent
    /// compaction).
    ///
    /// A time-based value starts the selection at the first turn to *begin*
    /// after the given instant, so the turn that was already running at that
    /// instant is excluded.
    /// A bound never splits a turn.
    // `allow_negative_numbers` keeps the from-end form usable as a
    // space-separated value (`--from -3`), which clap would otherwise read as an
    // unknown short flag.
    #[arg(long, value_parser = parse_bound, allow_negative_numbers = true)]
    from: Option<CliRangeBound>,

    /// End of the range, inclusive.
    ///
    /// Accepts the same forms as `--from`, except `last-compaction`, which only
    /// makes sense as a start bound.
    ///
    /// A time-based value ends the selection at (and includes) the turn that
    /// was running at the given instant — the mirror of `--from`, which
    /// excludes it.
    /// A bound never splits a turn.
    #[arg(long, value_parser = parse_to_bound, allow_negative_numbers = true)]
    to: Option<CliRangeBound>,

    /// Protect the first N turns from the selection.
    ///
    /// Accepts a turn count (`2`), an absolute 1-based turn (`@3`), a from-end
    /// offset (`-3`), or a duration (`5h`).
    /// Composes with every other selector: the selection starts no earlier than
    /// the first unprotected turn.
    #[arg(long, allow_negative_numbers = true)]
    keep_first: Option<RuleBound>,

    /// Protect the last N turns from the selection.
    ///
    /// Accepts the same forms as `--keep-first`, except `last-compaction`,
    /// which only makes sense at the start.
    /// Composes with every other selector: the selection ends no later than the
    /// last unprotected turn.
    #[arg(long, value_parser = parse_keep_last, allow_negative_numbers = true)]
    keep_last: Option<RuleBound>,
}

impl TurnSelection {
    /// Build a selection from explicit `--last`/`--turn` values.
    #[cfg(test)]
    pub(crate) fn from_last_turn(last: Option<usize>, turn: Option<usize>) -> Self {
        Self {
            last,
            turn: turn.map(TurnSpec::Single),
            ..Self::default()
        }
    }

    /// Whether any selector flag is set.
    ///
    /// Callers use this to tell "act on the whole conversation" apart from "the
    /// user asked for a subset", which matters where the two differ (leaving a
    /// stream untouched instead of filtering it to the same content).
    pub(crate) fn is_set(&self) -> bool {
        self.first.is_some()
            || self.last.is_some()
            || self.turn.is_some()
            || self.from.is_some()
            || self.to.is_some()
            || self.keep_first.is_some()
            || self.keep_last.is_some()
    }

    /// The `--keep-first` bound, if set.
    pub(crate) fn keep_first(&self) -> Option<&RuleBound> {
        self.keep_first.as_ref()
    }

    /// The `--keep-last` bound, if set.
    pub(crate) fn keep_last(&self) -> Option<&RuleBound> {
        self.keep_last.as_ref()
    }

    /// Reject flag combinations clap cannot express.
    ///
    /// `--keep-first N --first M` selects the first M turns minus the first N,
    /// e.g. `--keep-first 1 --first 16` selects turns 2 through 16: with N
    /// greater than M the protected turns swallow the whole selection, so the
    /// pair is rejected rather than silently selecting nothing.
    /// Only count-based bounds are comparable up front; durations and the other
    /// bound forms resolve against the stream and may legitimately come up
    /// empty.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if let (Some(RuleBound::Turns(keep)), Some(first)) = (&self.keep_first, self.first)
            && *keep > first
        {
            return Err(format!(
                "--keep-first {keep} is greater than --first {first}: nothing would remain to \
                 select"
            ));
        }
        if let (Some(RuleBound::Turns(keep)), Some(last)) = (&self.keep_last, self.last)
            && *keep > last
        {
            return Err(format!(
                "--keep-last {keep} is greater than --last {last}: nothing would remain to select"
            ));
        }
        Ok(())
    }

    /// Reject a `--turn` endpoint outside `1..=count`.
    ///
    /// `--turn` names specific turns, so an endpoint past the conversation is
    /// an error rather than a clamped selection (unlike `--first`/`--last`).
    pub(crate) fn check_turn_range(&self, count: usize) -> Result<(), String> {
        let oob = |n: usize| n == 0 || n > count;
        let ends = match self.turn.as_ref() {
            Some(TurnSpec::Single(n)) => [Some(*n), None],
            Some(TurnSpec::Range(a, b)) => [*a, *b],
            None => return Ok(()),
        };

        match ends.into_iter().flatten().find(|&n| oob(n)) {
            Some(n) => Err(format!(
                "turn {n} out of range (conversation has {count} turns)"
            )),
            None => Ok(()),
        }
    }

    /// The base windows named by the positive selectors, with unresolved
    /// bounds.
    ///
    /// One window for `--turn`, for `--from`/`--to`, and for no selector at all
    /// (the whole conversation); two when `--first` and `--last` are combined.
    /// `--first 0` and `--last 0` contribute no window.
    ///
    /// Keep flags are not applied here — see [`Self::trim`].
    pub(crate) fn windows(&self, events: &ConversationStream) -> Vec<BoundWindow> {
        if let Some(spec) = &self.turn {
            let (from, to) = match spec {
                TurnSpec::Single(n) => {
                    let at = RangeBound::Absolute(n.saturating_sub(1));
                    (at.clone(), at)
                }
                TurnSpec::Range(a, b) => (
                    // Open start (`--turn ..B`) begins at the first turn.
                    RangeBound::Absolute(a.map_or(0, |a| a.saturating_sub(1))),
                    // Open end (`--turn A..`) runs through the last turn.
                    b.map_or(RangeBound::FromEnd(0), |b| {
                        RangeBound::Absolute(b.saturating_sub(1))
                    }),
                ),
            };
            return vec![BoundWindow {
                from: Bound::At(from),
                to: Bound::At(to),
            }];
        }

        if self.first.is_some() || self.last.is_some() {
            let first = self.first.filter(|n| *n > 0);
            let last = self.last.filter(|n| *n > 0);

            // Two windows that meet or overlap are one window. Collapsing here
            // rather than after resolution keeps every consumer consistent:
            // `compact` acts per window, so a stale overlap would compact — and
            // for a summary rule, re-summarize — the shared turns twice.
            if let (Some(f), Some(l)) = (first, last)
                && f.saturating_add(l) >= events.turn_count()
            {
                return vec![BoundWindow {
                    from: Bound::At(RangeBound::Absolute(0)),
                    to: Bound::At(RangeBound::FromEnd(0)),
                }];
            }

            let mut windows = Vec::with_capacity(2);
            if let Some(n) = first {
                windows.push(BoundWindow {
                    from: Bound::At(RangeBound::Absolute(0)),
                    to: Bound::At(RangeBound::Absolute(n - 1)),
                });
            }
            if let Some(n) = last {
                windows.push(BoundWindow {
                    from: Bound::At(RangeBound::FromEnd(n - 1)),
                    to: Bound::At(RangeBound::FromEnd(0)),
                });
            }
            return windows;
        }

        let from = match &self.from {
            Some(bound) => resolve_cli_from(bound, events),
            None => Bound::Default,
        };
        let to = match &self.to {
            Some(bound) => resolve_cli_to(bound, events),
            None => Bound::Default,
        };
        vec![BoundWindow { from, to }]
    }

    /// Narrow a resolved window to the turns the keep flags leave unprotected.
    ///
    /// Returns `None` when the protected turns cover the whole window.
    /// The keep flags clamp rather than shift: turns already outside the window
    /// need no protecting, so `--keep-first 1 --from 3` still starts at turn 3.
    pub(crate) fn trim(
        &self,
        window: TurnWindow,
        events: &ConversationStream,
    ) -> Option<TurnWindow> {
        let mut from = window.from;
        let mut to = window.to;

        if let Some(bound) = &self.keep_first {
            match keep_first_bound(bound, events) {
                Bound::Empty => return None,
                Bound::Default => {}
                Bound::At(b) => from = from.max(from_index(b, events)?),
            }
        }
        if let Some(bound) = &self.keep_last {
            match keep_last_bound(bound, events) {
                Bound::Empty => return None,
                Bound::Default => {}
                Bound::At(b) => to = to.min(to_index(b, events)?),
            }
        }

        (from <= to).then_some(TurnWindow { from, to })
    }

    /// Resolve the selection against `events`.
    pub(crate) fn resolve(&self, events: &ConversationStream) -> TurnSet {
        let windows = self
            .windows(events)
            .into_iter()
            .filter_map(|window| resolve_window(&window, events))
            .filter_map(|window| self.trim(window, events))
            .collect();

        TurnSet::new(windows)
    }
}

#[cfg(test)]
#[path = "turn_selection_tests.rs"]
mod tests;
