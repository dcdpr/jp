//! `debug_app_screenshot` — a picture of the app's window, for the questions
//! the accessibility tree cannot answer.
//!
//! Markdown rendering, scroll bar proportions, truncation, colour, overlapping
//! views: none of that reaches the tree, and all of it is plain in a PNG.
//! Everything else the app does is cheaper to read as text, so this is the
//! escalation path rather than the first look.
//!
//! What comes back is a path, not an image.
//! A tool result is a string all the way to the provider, so the file reaches
//! the assistant only when a human attaches it on a following turn:
//!
//! ```sh
//! jp query -a tmp/debug-app/<slot>/shot-<ms>.png "why is the header clipped?"
//! ```
//!
//! Capturing needs the Screen Recording grant, which is a different grant from
//! the Accessibility one the rest of these tools need.
//! Missing it, the window server answers with the desktop instead of the
//! window, so the grant is checked before anything is written: a picture of the
//! wrong thing is worse than an error.

use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_tool::Outcome;
use serde::Deserialize;

use crate::{
    Context, Error, Tool,
    debug_app::{
        driver,
        session::{Session, Slot},
    },
    util::{
        ToolResult, error,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// What the Screen Recording grant being absent means for a capture.
///
/// Shared with `debug_app_pixels`, which captures the same way and fails the
/// same way when the grant is missing.
pub(crate) const NO_SCREEN_RECORDING: &str =
    "The Screen Recording grant is missing, so a capture would photograph the desktop rather than \
     the app's window. Grant it to the terminal application running these tools, under System \
     Settings > Privacy & Security > Screen & System Audio Recording, then start a new terminal \
     session.\n\nThis is a separate grant from the Accessibility one the other `debug_app_*` \
     tools need: holding one says nothing about the other.";

/// What `jpdrive windowid` reports.
///
/// Shared with `debug_app_pixels`, which captures the same way and needs the
/// same distinctions when there is nothing to capture.
#[derive(Debug, Deserialize)]
pub(crate) struct WindowList {
    /// Whether the driver may read other applications' screen content.
    pub screen_recording: bool,

    /// The app's capturable windows, front to back.
    pub windows: Vec<Window>,

    /// Windows the app has on another Space.
    ///
    /// Absent from every on-screen enumeration and from the accessibility tree,
    /// so an app with only these looks exactly like an app with no window at
    /// all.
    #[serde(default)]
    pub other_spaces: Vec<Window>,
}

/// One window, as the window server numbers it.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Window {
    pub id: u32,
    pub title: Option<String>,
    pub width: u32,
    pub height: u32,
}

/// Tool entrypoint.
#[allow(clippy::unused_async, reason = "awaited by the debug_app dispatcher")]
pub(crate) async fn debug_app_screenshot(ctx: &Context, _t: &Tool) -> ToolResult {
    if ctx.action.is_format_arguments() {
        return Ok(format_preview().into());
    }

    if !cfg!(target_os = "macos") {
        return error(
            "debug_app_screenshot only supports macOS: it captures a window through the macOS \
             window server.",
        );
    }

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());

    let dir = Session::dir(&ctx.root, &Slot::for_context(ctx)?);
    run(&ctx.root, &dir, millis, &DuctProcessRunner)
}

fn format_preview() -> String {
    "`debug_app_screenshot`\n\nWill execute:\n\n```sh\njust build-drive\njpdrive windowid --pid \
     <pid>\nscreencapture -l <window> -o -x tmp/debug-app/<slot>/shot-<ms>.png\n```\n\nWrites a \
     PNG of the frontmost window of the app recorded in\n`tmp/debug-app/<slot>/session.json` and \
     returns its path. The image itself does not\nreach the assistant: attach the file on a \
     following turn to have it looked at.\n\nNeeds the Screen Recording grant, which is a \
     different grant from the Accessibility\none the other `debug_app_*` tools need.\n\nReads \
     only. Nothing about the app's state is changed.\n"
        .to_owned()
}

/// Where a capture taken at `millis` is written.
///
/// Timestamped rather than fixed, so a sequence of shots can be compared
/// against each other instead of each one erasing the last.
fn shot_path(dir: &Utf8Path, millis: u128) -> Utf8PathBuf {
    dir.join(format!("shot-{millis}.png"))
}

/// Capture the app's frontmost window and report where it landed.
fn run(root: &Utf8Path, dir: &Utf8Path, millis: u128, runner: &dyn ProcessRunner) -> ToolResult {
    let session = Session::resolve(dir)?;
    let bin = driver::locate(root, runner)?;
    let list = windows(&bin, session.pid, root, runner)?;

    if !list.screen_recording {
        return error(NO_SCREEN_RECORDING);
    }

    let Some(window) = list.windows.first() else {
        return error(no_window(session.pid, &list, "capture"));
    };

    let path = shot_path(dir, millis);
    let size = capture(window.id, &path, root, runner)?;

    Ok(Outcome::Success {
        content: report(root, &session, window, list.windows.len(), &path, size),
    })
}

/// Why there is nothing to act on, and which of the two reasons it is.
///
/// A window on another Space and no window at all are indistinguishable from
/// every on-screen enumeration and from the accessibility tree alike, and the
/// difference is the whole of what to do next: switch desktop, or start the
/// app.
/// Told apart by asking the window server for windows on *every* Space and
/// subtracting the ones on this one.
pub(crate) fn no_window(pid: u32, list: &WindowList, verb: &str) -> String {
    if list.other_spaces.is_empty() {
        return format!(
            "The app (pid {pid}) has no window at all, so there is nothing to {verb}. A window \
             that is minimized or closed is absent from the window server's list entirely."
        );
    }

    format!(
        "The app (pid {pid}) has {} window(s), all of them on another Space, so there is nothing \
         to {verb} and the accessibility tree reports none either. Switch to the desktop the app \
         is on, or move its window to this one.",
        list.other_spaces.len()
    )
}

/// Ask the driver which windows the app has, and whether they can be captured.
pub(crate) fn windows(
    bin: &Utf8Path,
    pid: u32,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Result<WindowList, Error> {
    let output = runner
        .run(bin.as_str(), &["windowid", "--pid", &pid.to_string()], root)
        .map_err(|e| format!("Failed to spawn {bin}: {e}"))?;

    if !output.success() {
        return Err(driver::describe_failure(
            "windowid",
            bin,
            pid,
            root,
            runner,
            &output.stdout,
            &output.stderr,
        )
        .into());
    }

    parse(&output.stdout)
}

/// Read the driver's window list.
fn parse(stdout: &str) -> Result<WindowList, Error> {
    serde_json::from_str(stdout)
        .map_err(|e| format!("Failed to parse the window list `jpdrive` reported: {e}").into())
}

/// Write a PNG of window `id`, and answer how many bytes it holds.
///
/// `-o` leaves out the drop shadow, which is otherwise a wide transparent
/// margin around every window.
///
/// A zero-byte file is treated as a failure: `screencapture` reports success
/// for a window it could not read, and an empty PNG returned as a result reads
/// like a capture that worked.
fn capture(
    id: u32,
    path: &Utf8Path,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Result<u64, Error> {
    let output = runner
        .run(
            "screencapture",
            &["-l", &id.to_string(), "-o", "-x", path.as_str()],
            root,
        )
        .map_err(|e| format!("Failed to spawn `screencapture`: {e}"))?;

    if !output.success() {
        return Err(format!(
            "`screencapture` refused to capture window {id}: {}",
            output.stderr.trim_end()
        )
        .into());
    }

    let size = fs::metadata(path).map(|m| m.len()).unwrap_or_default();
    if size == 0 {
        return Err(format!(
            "`screencapture` reported success but left nothing at {path}. The window may have \
             closed while it was being read."
        )
        .into());
    }

    Ok(size)
}

/// Render the capture report.
fn report(
    root: &Utf8Path,
    session: &Session,
    window: &Window,
    count: usize,
    path: &Utf8Path,
    size: u64,
) -> String {
    let shown = path.strip_prefix(root).unwrap_or(path);
    let title = window
        .title
        .as_deref()
        .map_or(String::new(), |title| format!(" {title:?}"));

    let mut report = format!(
        "Captured window {}{title} of the app (pid {}), {}x{} points.\n\nWritten to `{shown}` ({} \
         KiB).\n",
        window.id,
        session.pid,
        window.width,
        window.height,
        size.div_ceil(1024),
    );

    if count > 1 {
        report.push_str(&format!(
            "\nThe app has {count} windows on screen. This is the frontmost one.\n"
        ));
    }

    report.push_str(&format!(
        "\nA tool result is text, so the image does not reach the assistant from here. Attach the \
         file on the next turn to have it looked at:\n\n```sh\njp query -a {shown} \"what is \
         wrong with this layout?\"\n```\n"
    ));

    report
}

#[cfg(all(test, unix))]
#[path = "screenshot_tests.rs"]
mod tests;
