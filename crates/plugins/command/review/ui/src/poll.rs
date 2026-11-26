//! When the UI asks the server again for what changed.
//!
//! A page polls every [`IDLE`] milliseconds, and every [`RUNNING`] while a pass
//! runs, so the assistant's replies show soon after they are written.
//! A hidden page doesn't poll, and one shown again polls at once.
//!
//! A page served from a bundle directory (`jp review serve --ui-dir`) also asks
//! every [`BUILD_EVERY`] milliseconds which build the directory holds, and
//! reloads once that is another than the one it started with.

/// Milliseconds between polls while no pass runs.
pub const IDLE: f64 = 2000.0;

/// Milliseconds between polls while a pass runs.
pub const RUNNING: f64 = 500.0;

/// Milliseconds between the checks of whether a poll is due.
pub const TICK: u64 = 250;

/// Milliseconds between asking which build a bundle directory holds.
pub const BUILD_EVERY: u64 = 1000;

/// Whether a page started with the build `loaded` is out of date once the
/// server names the build `now`; an empty `now` names none, as while `just
/// build-ui` writes one.
#[must_use]
pub fn rebuilt(loaded: &str, now: &str) -> bool {
    !now.is_empty() && now != loaded
}

/// Whether a page last polled at `last`, which has a pass `running`, is due to
/// poll at `now`; the first poll is always due.
///
/// Times are in milliseconds.
#[must_use]
pub fn due(now: f64, last: Option<f64>, running: bool) -> bool {
    let every = if running { RUNNING } else { IDLE };

    last.is_none_or(|last| now - last >= every)
}

/// The pass that ended between two polls: the one that ran at the first, when
/// the second shows another running, or none.
#[must_use]
pub fn ended(before: Option<u32>, now: Option<u32>) -> Option<u32> {
    before.filter(|pass| now != Some(*pass))
}

#[cfg(test)]
#[path = "poll_tests.rs"]
mod tests;
