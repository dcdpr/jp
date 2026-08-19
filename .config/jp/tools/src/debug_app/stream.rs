//! The intervals the app timed about itself, read whole rather than summarized.
//!
//! Two sources, one timeline.
//! The live stream in the app's state directory holds this run; the archived
//! streams beside the recordings hold earlier ones.
//! Both are the same JSON-per-line format, every line carries a wall clock, so
//! they concatenate and sort into one sequence that spans every run a slot has
//! kept.
//!
//! Read directly, never through [`Session`].
//! The offset that tool holds on the stream is what `debug_app_snapshot` uses
//! to report deltas, and a second reader consuming it would silently turn every
//! snapshot's trace section empty.
//!
//! Counts, not milliseconds.
//! View-body evaluations and FFI calls are deterministic for the same steps, so
//! two runs of the same list can be compared on them and a fix can be asserted
//! against them.
//! Wall clock cannot: it is noisy within one run and not comparable between
//! two, which is why every view here leads on a count and carries a duration
//! beside it rather than the other way round.
//!
//! [`Session`]: super::session::Session

use std::collections::BTreeMap;

use camino::Utf8Path;
use chrono::DateTime;
use serde_json::Value;

use crate::{
    Error,
    debug_app::{
        capture,
        session::{state_dir, trace_path},
    },
    util::trace::{TraceEvent, parse_lines},
};

/// The field an interval's duration arrives in.
const DURATION_FIELD: &str = "duration_ms";

/// The field a memory sample arrives in.
const FOOTPRINT_FIELD: &str = "footprint_mb";

/// What the app attributes its own work to when it crosses into Rust.
///
/// Nested inside the Swift interval that caused it, so an FFI count is a count
/// of calls made under the step being measured.
pub(crate) const FFI_TARGET: &str = "JP.FFI";

/// Suffix on the name of an interval that timed a view body evaluating.
const BODY_SUFFIX: &str = ".body";

/// One interval the app timed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Interval {
    /// What the app called the work.
    pub name: String,

    /// What the app attributed it to.
    pub target: String,

    /// How long it took.
    pub duration_ms: f64,

    /// When it ended, in milliseconds since the epoch.
    ///
    /// What the app writes: an interval is recorded when it closes.
    pub at_ms: u64,

    /// When it began, in milliseconds since the epoch.
    ///
    /// Derived, because the app records only the end and the duration.
    /// This is the moment that attributes the work: an interval belongs to
    /// whatever was happening when it started, and a selection that takes 85ms
    /// routinely ends after the step that asked for it has been reported.
    pub started_ms: u64,

    /// What the process occupied when it ended, in MiB.
    pub footprint_mb: Option<u64>,

    /// The enclosing interval names, root first.
    pub spans: Vec<String>,
}

impl Interval {
    /// Whether this timed a view body evaluating.
    pub(crate) fn is_view_body(&self) -> bool {
        self.name.ends_with(BODY_SUFFIX)
    }

    /// Whether this timed work on the Rust side of the FFI boundary.
    pub(crate) fn is_ffi(&self) -> bool {
        self.target == FFI_TARGET
    }

    /// Whether nothing the app timed encloses this.
    pub(crate) fn is_top_level(&self) -> bool {
        self.spans.is_empty()
    }
}

/// Every interval a slot has kept, in the order the work began.
///
/// Archived streams first, then the live one, which is also chronological: a
/// stream is archived at the launch that replaced it.
///
/// Sorted by start rather than by end, so an enclosing interval reads ahead of
/// the work it contains rather than after it.
pub(crate) fn load(dir: &Utf8Path) -> Vec<Interval> {
    let mut intervals = Vec::new();

    for path in capture::streams(dir) {
        intervals.extend(read(&path));
    }
    intervals.extend(read(&trace_path(&state_dir(dir))));

    intervals.sort_by_key(|interval| interval.started_ms);
    intervals
}

/// The intervals in one stream.
fn read(path: &Utf8Path) -> Vec<Interval> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };

    parse_lines(&raw).iter().filter_map(interval).collect()
}

/// One event as an interval, or `None` when it timed nothing.
///
/// The app writes events that are not intervals — `trace.origin` carries the
/// pair that lines this timeline up with a mach one — and those have no
/// duration.
///
/// The one place that decides what an interval is, so a snapshot's one-line
/// summary and a report's table cannot disagree about it.
pub(crate) fn interval(event: &TraceEvent) -> Option<Interval> {
    let duration_ms = event.fields.get(DURATION_FIELD).and_then(Value::as_f64)?;

    let at_ms = millis(&event.timestamp)?;

    // Truncated towards zero and floored at the epoch: a duration is milliseconds
    // to three decimal places, and the sub-millisecond part cannot move which
    // step a start falls in.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded above by at_ms and below by zero"
    )]
    let elapsed_ms = duration_ms.max(0.0).trunc() as u64;

    Some(Interval {
        name: event.message.clone(),
        target: event.target.clone(),
        duration_ms,
        at_ms,
        started_ms: at_ms.saturating_sub(elapsed_ms),
        footprint_mb: event.fields.get(FOOTPRINT_FIELD).and_then(Value::as_u64),
        spans: event.spans.clone(),
    })
}

/// An RFC 3339 timestamp as milliseconds since the epoch.
fn millis(timestamp: &str) -> Option<u64> {
    let parsed = DateTime::parse_from_rfc3339(timestamp).ok()?;

    u64::try_from(parsed.timestamp_millis()).ok()
}

/// What a set of intervals amounts to.
///
/// Counts first, because they are what two runs can be compared on.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Counts {
    /// How many intervals there were.
    pub intervals: usize,

    /// How many of them timed a view body evaluating.
    pub view_bodies: usize,

    /// How many crossed into Rust.
    pub ffi_calls: usize,

    /// How long the intervals nothing else encloses took, added up.
    ///
    /// Only the outermost, so work timed inside another interval is not counted
    /// twice.
    pub traced_ms: f64,

    /// What the process occupied at the last sample, in MiB.
    pub footprint_mb: Option<u64>,
}

/// Reduce `intervals` to what can be compared.
pub(crate) fn count(intervals: &[&Interval]) -> Counts {
    let mut counts = Counts::default();
    let mut sampled_at = 0;

    for interval in intervals {
        counts.intervals += 1;

        if interval.is_view_body() {
            counts.view_bodies += 1;
        }
        if interval.is_ffi() {
            counts.ffi_calls += 1;
        }
        if interval.is_top_level() {
            counts.traced_ms += interval.duration_ms;
        }

        // The latest sample by the moment it was taken, which is the end of an
        // interval rather than its start. Taking whichever came last in the slice
        // would report an enclosed interval's footprint as the enclosing one's,
        // since these are ordered by start.
        if let Some(footprint) = interval.footprint_mb
            && interval.at_ms >= sampled_at
        {
            counts.footprint_mb = Some(footprint);
            sampled_at = interval.at_ms;
        }
    }

    counts
}

/// One named piece of work, across every time it ran.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Tally {
    pub count: usize,
    pub total_ms: f64,
    pub max_ms: f64,
}

impl Tally {
    /// The mean duration, or zero when nothing ran.
    pub(crate) fn mean_ms(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }

        #[allow(clippy::cast_precision_loss, reason = "display only")]
        let count = self.count as f64;

        self.total_ms / count
    }
}

/// Group `intervals` by name, busiest first.
///
/// Ordered by count rather than by time, which is the ordering an agent can act
/// on: a body evaluating 148 times is a fact about the code, and the 1.1ms it
/// averages is a fact about the machine.
pub(crate) fn tally(intervals: &[&Interval]) -> Vec<(String, Tally)> {
    let mut by_name: BTreeMap<String, Tally> = BTreeMap::new();

    for interval in intervals {
        let tally = by_name.entry(interval.name.clone()).or_default();
        tally.count += 1;
        tally.total_ms += interval.duration_ms;
        tally.max_ms = tally.max_ms.max(interval.duration_ms);
    }

    let mut out: Vec<(String, Tally)> = by_name.into_iter().collect();

    // Name breaks the tie, so the same input always renders the same table.
    out.sort_by(|(left_name, left), (right_name, right)| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left_name.cmp(right_name))
    });
    out
}

/// A duration in milliseconds, with a decimal only where one carries meaning.
///
/// Sub-millisecond work is routine here — a view body evaluating is tens of
/// microseconds — and rounding all of it to `0 ms` would hide which of two
/// bodies is the expensive one.
pub(crate) fn millis_label(duration: f64) -> String {
    if duration < 10.0 {
        format!("{duration:.1} ms")
    } else {
        format!("{} ms", duration.round())
    }
}

/// Whether this slot has a stream at all, live or archived.
///
/// Distinguishes "the app has never run here" from "the app ran and timed
/// nothing", which are different problems with different fixes.
pub(crate) fn is_present(dir: &Utf8Path) -> bool {
    trace_path(&state_dir(dir)).exists() || !capture::streams(dir).is_empty()
}

/// Fail with what a slot holds instead of a stream.
pub(crate) fn missing(dir: &Utf8Path) -> Error {
    format!(
        "No traced intervals in {}. The app writes them only when it is launched with a state \
         directory, which `debug_app_launch` does, so this slot has either never run an app or \
         ran one built without the instrumentation.",
        state_dir(dir)
    )
    .into()
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
