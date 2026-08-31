//! What the app said about its own work, reduced to a few lines.
//!
//! The app writes an interval per named piece of work to `trace.jsonl`, in the
//! same JSON-per-line format `jp` writes, and samples its memory footprint at
//! the end of each one.
//! A snapshot reports that stream the way it reports the console: only what is
//! new, and only as much as fits beside everything else it returns.
//!
//! Summary only, deliberately.
//! A snapshot that dumped a span log would stop being the cheap observation it
//! is used as, and the questions a full log answers — which call site, which
//! pass, what nested what — need a tool of their own.

use crate::{debug_app::stream, util::trace::TraceEvent};

/// What the trace delta amounts to.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Summary {
    /// How many intervals ended in the delta.
    pub intervals: usize,

    /// The longest interval, as its name and how long it took.
    pub slowest: Option<(String, f64)>,

    /// The most recent footprint sample, in MiB.
    pub footprint_mb: Option<u64>,

    /// Whether the delta held anything at all.
    pub is_empty: bool,
}

/// Reduce the events written since the last call.
///
/// Parsed through [`stream::interval`], so what counts as an interval and which
/// fields carry a duration and a footprint are decided in one place.
/// What is summarized differs: this is one line about a delta, and the report
/// is a table over a window.
pub(crate) fn summarize(events: &[TraceEvent]) -> Summary {
    let mut summary = Summary {
        // The raw events, not the intervals among them: a delta holding only
        // events that timed nothing is still a delta, and reporting it as empty
        // would say the app did nothing when it did.
        is_empty: events.is_empty(),
        ..Summary::default()
    };

    for interval in events.iter().filter_map(stream::interval) {
        summary.intervals += 1;

        if let Some(footprint) = interval.footprint_mb {
            summary.footprint_mb = Some(footprint);
        }

        if summary
            .slowest
            .as_ref()
            .is_none_or(|(_, slowest)| interval.duration_ms > *slowest)
        {
            summary.slowest = Some((interval.name, interval.duration_ms));
        }
    }

    summary
}

/// Render the summary block, or nothing when the app traced nothing.
///
/// `previous` is the footprint the last snapshot reported, which is what makes
/// the change a change since the caller last looked rather than since the first
/// sample in this delta.
pub(crate) fn render(summary: &Summary, previous: Option<u64>) -> Option<String> {
    if summary.is_empty {
        return None;
    }

    let mut block = match (summary.intervals, &summary.slowest) {
        (0, _) => "Trace, since the last call: no intervals.".to_owned(),
        (count, Some((name, duration))) => format!(
            "Trace, since the last call: {count} {}. Slowest `{name}` {}.",
            plural(count, "span"),
            stream::millis_label(*duration)
        ),
        (count, None) => format!(
            "Trace, since the last call: {count} {}.",
            plural(count, "span")
        ),
    };

    if let Some(footprint) = summary.footprint_mb {
        block.push_str(&format!("\nFootprint {footprint} MB"));
        if let Some(change) = previous.map(|before| footprint.cast_signed() - before.cast_signed())
            && change != 0
        {
            block.push_str(&format!(" ({change:+} MB)"));
        }
        block.push('.');
    }

    Some(block)
}

/// `word` pluralized for `count`.
fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        word.to_owned()
    } else {
        format!("{word}s")
    }
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
