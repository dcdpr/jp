//! When each driven step ran.
//!
//! The app's stream says how long a piece of work took; it says nothing about
//! what asked for that work.
//! `debug_app_drive` knows, so it writes a line per step naming the step and
//! the wall-clock window it occupied, and a report intersects the two.
//!
//! Beside the app's stream rather than inside it.
//! That file belongs to the app: it is opened by the process being observed,
//! appended to from whichever thread ends an interval, and a second writer
//! would interleave with it.
//!
//! An interval belongs to the step whose window holds the moment it *began*,
//! not the moment it ended.
//! A selection's read runs on its own task, so the harness sees the sidebar
//! change and moves on while the transcript is still loading: attributing on
//! the end instead files an 85ms selection under the following step, or under
//! no step at all when it outlives the run.
//!
//! Windows overlap what the harness did as well as what the app did — reading
//! the accessibility tree between steps takes longer than most steps do — so a
//! window bounds attribution rather than measuring it.
//! What it bounds is enough: the app traces nothing while nobody is driving it,
//! so work that began inside a step's window was asked for by that step.
//!
//! Kept across runs, and swept on the same terms as everything else a slot
//! holds.
//! A step number repeats between runs, so each run of the harness stamps its
//! lines with an id of its own and a report scopes by that.

use std::{fs, time::Duration};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::{Error, debug_app::capture::unix_millis};

/// Where the marks live, inside a slot's directory.
///
/// Outside the state directory on purpose: a launch with `fresh` empties that,
/// and losing every earlier run's step boundaries with it would leave the
/// archived streams unattributable.
const MARKS_FILE: &str = "steps.jsonl";

/// One step, and the window it occupied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Mark {
    /// The run of the harness this step belonged to.
    pub run: String,

    /// Its position in that run's list, counting from one.
    pub step: usize,

    /// The step as a report names it.
    pub label: String,

    /// When the harness began the step, in milliseconds since the epoch.
    pub began_ms: u64,

    /// When it finished reading the result, in milliseconds since the epoch.
    pub ended_ms: u64,
}

impl Mark {
    /// Whether `at_ms` falls inside this step's window.
    ///
    /// Called with the moment an interval began.
    /// See the module documentation for why that rather than the moment it
    /// ended.
    pub(crate) const fn holds(&self, at_ms: u64) -> bool {
        at_ms >= self.began_ms && at_ms <= self.ended_ms
    }
}

/// An id for a run of the harness.
pub(crate) fn new_run() -> String {
    format!("drive-{}", unix_millis())
}

/// The current wall clock, in milliseconds since the epoch.
pub(crate) fn now_ms() -> u64 {
    unix_millis()
}

/// Where a slot keeps its step boundaries.
pub(crate) fn path(dir: &Utf8Path) -> Utf8PathBuf {
    dir.join(MARKS_FILE)
}

/// Append `marks` to the slot's record of what has been driven.
pub(crate) fn append(dir: &Utf8Path, marks: &[Mark]) -> Result<(), Error> {
    if marks.is_empty() {
        return Ok(());
    }

    let mut lines = String::new();
    for mark in marks {
        lines.push_str(&serde_json::to_string(mark)?);
        lines.push('\n');
    }

    fs::create_dir_all(dir)?;
    let path = path(dir);
    let existing = fs::read_to_string(&path).unwrap_or_default();

    fs::write(&path, format!("{existing}{lines}"))
        .map_err(|e| format!("Failed to write {path}: {e}").into())
}

/// Drop the marks older than `window`, and report how many went.
///
/// Lines rather than the file, because one file holds every run a slot has
/// driven: deleting it would take the runs still inside the window with it.
/// Rewritten only when something actually expired, so the ordinary sweep of a
/// slot driven today touches nothing.
pub(crate) fn sweep(dir: &Utf8Path, window: Duration) -> usize {
    let held = load(dir);
    if held.is_empty() {
        return 0;
    }

    let cutoff = unix_millis().saturating_sub(window.as_millis().try_into().unwrap_or(u64::MAX));
    let kept: Vec<&Mark> = held.iter().filter(|mark| mark.ended_ms >= cutoff).collect();

    let expired = held.len() - kept.len();
    if expired == 0 {
        return 0;
    }

    let mut lines = String::new();
    for mark in kept {
        if let Ok(line) = serde_json::to_string(mark) {
            lines.push_str(&line);
            lines.push('\n');
        }
    }

    // A failed rewrite leaves the file as it was, which is the safe direction:
    // stale marks attribute nothing to a step that no longer exists, and the next
    // sweep tries again.
    if fs::write(path(dir), lines).is_err() {
        return 0;
    }

    expired
}

/// Every mark this slot has, oldest first.
///
/// A malformed line is skipped rather than fatal, the same way the trace parser
/// treats one: a truncated trailing line should not lose the report.
pub(crate) fn load(dir: &Utf8Path) -> Vec<Mark> {
    let Ok(raw) = fs::read_to_string(path(dir)) else {
        return Vec::new();
    };

    let mut marks: Vec<Mark> = raw
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    marks.sort_by_key(|mark| mark.began_ms);
    marks
}

/// The marks belonging to the most recent run of the harness.
pub(crate) fn latest_run(marks: &[Mark]) -> Vec<Mark> {
    let Some(run) = marks.last().map(|mark| mark.run.clone()) else {
        return Vec::new();
    };

    marks
        .iter()
        .filter(|mark| mark.run == run)
        .cloned()
        .collect()
}

/// The marks whose windows overlap `[from_ms, to_ms]`.
pub(crate) fn overlapping(marks: &[Mark], from_ms: u64, to_ms: u64) -> Vec<Mark> {
    marks
        .iter()
        .filter(|mark| mark.began_ms <= to_ms && mark.ended_ms >= from_ms)
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "marks_tests.rs"]
mod tests;
