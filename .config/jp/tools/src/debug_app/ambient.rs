//! Borrowing and returning the state a driven run does not own.
//!
//! Which application is in front, and where the pointer is.
//! A step that synthesizes input has to take both: mouse events go to whatever
//! is on top at a coordinate, and the ordering between applications follows
//! activation.
//! Both belong to whoever is at the keyboard.
//!
//! Run-scoped, and it has to be: `jpdrive` runs one step per process, so
//! nothing on that side outlives a single step.
//! Restoring there would put focus back between every pair of steps and leave
//! the next one aiming at a window that is no longer in front.
//!
//! Deliberately not window geometry.
//! A step that resized a window did the thing it was asked to do, and putting
//! the window back would undo the effect the run was measuring.

use camino::Utf8Path;
use serde::Deserialize;

use crate::util::runner::ProcessRunner;

/// What a run borrowed, to be handed back when it ends.
///
/// Either field is `None` when the driver could not report it.
/// A restore skips what it does not know rather than guessing, because guessing
/// here moves the pointer of somebody who is using it.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Borrowed {
    frontmost: Option<String>,
    pointer: Option<(f64, f64)>,
}

#[derive(Deserialize)]
struct FrontmostReport {
    bundle_id: Option<String>,
}

#[derive(Deserialize)]
struct PointerReport {
    x: f64,
    y: f64,
}

/// Read what is about to be borrowed.
///
/// Never fails the run.
/// A driver that cannot report the frontmost application is a reason to leave
/// focus alone afterwards, not a reason to refuse to drive.
pub(crate) fn capture(bin: &Utf8Path, root: &Utf8Path, runner: &dyn ProcessRunner) -> Borrowed {
    Borrowed {
        frontmost: read::<FrontmostReport>(bin, &["frontmost"], root, runner)
            .and_then(|report| report.bundle_id),
        pointer: read::<PointerReport>(bin, &["pointer"], root, runner)
            .map(|report| (report.x, report.y)),
    }
}

/// Put back what was borrowed.
///
/// Focus first, then the pointer: activating an application does not move the
/// cursor, so the order only matters in that the pointer must not be placed and
/// then have an activation drag it elsewhere.
///
/// Silent about failure for the same reason as [`capture`]: this runs after the
/// work a caller asked for, and a complaint here would replace whatever the run
/// was reporting.
pub(crate) fn restore(
    borrowed: &Borrowed,
    bin: &Utf8Path,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) {
    if let Some(bundle_id) = &borrowed.frontmost {
        let _restored = runner.run(bin.as_str(), &["frontmost", "--set", bundle_id], root);
    }

    if let Some((x, y)) = borrowed.pointer {
        let point = format!("{x},{y}");
        let _restored = runner.run(bin.as_str(), &["pointer", "--set", &point], root);
    }
}

/// Run one driver subcommand and read its JSON document.
fn read<T: for<'de> Deserialize<'de>>(
    bin: &Utf8Path,
    args: &[&str],
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Option<T> {
    let output = runner.run(bin.as_str(), args, root).ok()?;
    serde_json::from_str(&output.stdout).ok()
}

#[cfg(all(test, unix))]
#[path = "ambient_tests.rs"]
mod tests;
