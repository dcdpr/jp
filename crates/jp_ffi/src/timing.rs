//! How long the work inside one call took.
//!
//! Measuring happens here; writing does not.
//! The caller keeps one trace file and one ordering, and a writer on this side
//! of the boundary would produce a second timeline to be reconciled with the
//! first afterwards.
//!
//! Durations rather than timestamps, for the same reason: two clocks that
//! nearly agree are worse than one, so the caller places these against the
//! clock it already reads.

use std::{
    ffi::{CString, c_char},
    ptr,
    time::{Duration, Instant},
};

use serde::Serialize;

/// One measured piece of work.
#[derive(Debug, Serialize)]
struct Span {
    /// What the work is called.
    name: &'static str,

    /// How long it took, in milliseconds to the microsecond.
    duration_ms: f64,
}

/// What one call measured, in the order the work finished.
#[derive(Debug, Default)]
pub(crate) struct Timings {
    spans: Vec<Span>,
}

impl Timings {
    /// Run `work`, recording how long it took under `name`.
    pub(crate) fn measure<T>(&mut self, name: &'static str, work: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let value = work();
        self.record(name, started.elapsed());
        value
    }

    /// Record work that was timed elsewhere.
    pub(crate) fn record(&mut self, name: &'static str, elapsed: Duration) {
        self.spans.push(Span {
            name,
            duration_ms: milliseconds(elapsed),
        });
    }

    /// The spans as the JSON array the caller decodes.
    ///
    /// `None` when the array cannot be built, which leaves the caller without
    /// timings for the call rather than without its result.
    pub(crate) fn to_c_string(&self) -> Option<CString> {
        CString::new(serde_json::to_string(&self.spans).ok()?).ok()
    }
}

/// Hand the caller its timings, if it asked for them.
///
/// A non-null `slot` is always written, with null standing for timings that
/// could not be built, so a caller never reads back whatever it happened to
/// declare the variable with.
///
/// A written pointer is released with `jp_string_free`, like every other string
/// this library returns.
///
/// # Safety
///
/// `slot` must be null, or point to a writable `*mut c_char`.
pub(crate) unsafe fn publish(slot: *mut *mut c_char, timings: &Timings) {
    if slot.is_null() {
        return;
    }

    let json = timings
        .to_c_string()
        .map_or(ptr::null_mut(), CString::into_raw);

    // SAFETY: `slot` is non-null (checked above) and writable per this
    // function's contract, so the write lands in the caller's variable.
    unsafe { slot.write(json) };
}

/// A duration in milliseconds, rounded to the microsecond.
///
/// The resolution the caller records its own intervals at, so a span from this
/// side and the one around it on the other read in the same units.
fn milliseconds(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

#[cfg(test)]
#[path = "timing_tests.rs"]
mod tests;
