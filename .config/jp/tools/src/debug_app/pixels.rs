//! `debug_app_pixels` — what colour something is, and how wide it is.
//!
//! The escalation from the accessibility tree for anything *drawn*.
//! A tree read gives the frame of every element, which settles where a text
//! field or a row sits, but a divider, a selection fill, a row separator and a
//! rounded border are not elements: they have no frame to ask for and no colour
//! to report.
//! A scanline across the window has all four in it.
//!
//! Reads in **pixels**, not points.
//! That is the unit the questions arrive in — "the line is 2px and should be
//! 2px" — and it is the unit that makes the retina factor visible instead of
//! hiding it: a one-point line reads as a run of two.
//! The window's size is reported both ways so the conversion is at hand.
//!
//! Captures with `screencapture` and scans with `jpdrive pixels`, which is
//! where the image decoding lives.
//! Each capture is kept, timestamped, beside the ones `debug_app_screenshot`
//! writes, so a scan can be re-read or attached.

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
        screenshot::{self, Window},
        session::{Session, Slot},
    },
    util::{
        ToolResult, error,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// Which way a scan runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Axis {
    /// Left to right, across one row.
    Row,

    /// Top to bottom, down one column.
    Column,
}

impl Axis {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Row => "row",
            Self::Column => "column",
        }
    }

    /// What the offsets along this scan measure.
    const fn along(self) -> &'static str {
        match self {
            Self::Row => "x",
            Self::Column => "y",
        }
    }
}

/// What the caller asked to be scanned.
#[derive(Debug)]
struct Args {
    /// Which way to scan.
    scan: Axis,

    /// The row or column to read, in pixels from the top or the left.
    at: u32,

    /// Where along the scan to start, in pixels.
    /// The near edge when absent.
    from: Option<u32>,

    /// Where along the scan to stop, inclusive.
    /// The far edge when absent.
    to: Option<u32>,

    /// A PNG to scan instead of capturing a fresh one.
    ///
    /// For re-reading a capture a previous call left behind, at another line or
    /// another range, without disturbing the app.
    image: Option<Utf8PathBuf>,
}

impl Args {
    fn from_tool(t: &Tool) -> Result<Self, Error> {
        Ok(Self {
            scan: t.req("scan")?,
            at: t.req("at")?,
            from: t.opt("from")?,
            to: t.opt("to")?,
            image: t.opt("image")?,
        })
    }
}

/// What `jpdrive pixels` reports.
#[derive(Debug, Deserialize)]
struct Scan {
    width: u32,
    height: u32,
    color_space: String,
    scan: String,
    at: u32,
    runs: Vec<Run>,
}

/// One stretch of identical pixels.
#[derive(Debug, Deserialize)]
struct Run {
    start: u32,
    count: u32,
    color: String,
}

/// Tool entrypoint.
#[allow(clippy::unused_async, reason = "awaited by the debug_app dispatcher")]
pub(crate) async fn debug_app_pixels(ctx: &Context, t: &Tool) -> ToolResult {
    let args = Args::from_tool(t)?;

    if ctx.action.is_format_arguments() {
        return Ok(format_preview(&args).into());
    }

    if !cfg!(target_os = "macos") {
        return error(
            "debug_app_pixels only supports macOS: it reads a window through the macOS window \
             server.",
        );
    }

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());

    let dir = Session::dir(&ctx.root, &Slot::for_context(ctx));
    run(&ctx.root, &dir, millis, &args, &DuctProcessRunner)
}

fn format_preview(args: &Args) -> String {
    format!(
        "`debug_app_pixels`\n\nWill execute:\n\n```sh\nscreencapture -l <window> -o -x \
         tmp/debug-app/<slot>/shot-<ms>.png\njpdrive pixels --image <png> --scan {} --at \
         {}\n```\n\nReports the colours along one row or column of the app's frontmost window, as \
         runs\nof identical pixels. Offsets and colours are in pixels and in the image's \
         own\ncolour space.\n\nReads only. Nothing about the app's state is changed.\n",
        args.scan.as_str(),
        args.at
    )
}

/// Capture the window if needed, scan it, and report the runs.
fn run(
    root: &Utf8Path,
    dir: &Utf8Path,
    millis: u128,
    args: &Args,
    runner: &dyn ProcessRunner,
) -> ToolResult {
    let bin = driver::locate(root, runner)?;

    let (image, window) = if let Some(path) = &args.image {
        (root.join(path), None)
    } else {
        let session = Session::resolve(dir)?;
        let list = screenshot::windows(&bin, session.pid, root, runner)?;

        if !list.screen_recording {
            return error(screenshot::NO_SCREEN_RECORDING);
        }

        let Some(window) = list.windows.first().cloned() else {
            return error(screenshot::no_window(session.pid, &list, "read"));
        };

        let path = dir.join(format!("shot-{millis}.png"));
        capture(window.id, &path, root, runner)?;
        (path, Some(window))
    };

    let scan = scan(&bin, &image, args, root, runner)?;
    Ok(Outcome::Success {
        content: report(root, &image, window.as_ref(), args, &scan),
    })
}

/// Write a PNG of window `id`.
///
/// `-o` leaves out the drop shadow, which would otherwise put a wide band of
/// transparent pixels at the start of every scan.
fn capture(
    id: u32,
    path: &Utf8Path,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Result<(), Error> {
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

    if fs::metadata(path).map(|m| m.len()).unwrap_or_default() == 0 {
        return Err(format!(
            "`screencapture` reported success but left nothing at {path}. The window may have \
             closed while it was being read."
        )
        .into());
    }

    Ok(())
}

/// Ask the driver for the runs along the requested line.
fn scan(
    bin: &Utf8Path,
    image: &Utf8Path,
    args: &Args,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Result<Scan, Error> {
    let at = args.at.to_string();
    let mut argv = vec![
        "pixels",
        "--image",
        image.as_str(),
        "--scan",
        args.scan.as_str(),
        "--at",
        &at,
    ];

    let from = args.from.map(|value| value.to_string());
    if let Some(from) = &from {
        argv.extend(["--from", from]);
    }

    let to = args.to.map(|value| value.to_string());
    if let Some(to) = &to {
        argv.extend(["--to", to]);
    }

    let output = runner
        .run(bin.as_str(), &argv, root)
        .map_err(|e| format!("Failed to spawn {bin}: {e}"))?;

    if !output.success() {
        return Err(format!(
            "`jpdrive pixels` refused to scan {image}:\n\n```\n{}\n```",
            output.stdout.trim_end()
        )
        .into());
    }

    serde_json::from_str(&output.stdout)
        .map_err(|e| format!("Failed to parse the scan `jpdrive` reported: {e}").into())
}

/// Render the scan report.
///
/// The runs are a table because that is how they are read: a reader is looking
/// for where one colour stops and the next begins, and comparing an offset
/// against a frame from the accessibility tree.
fn report(
    root: &Utf8Path,
    image: &Utf8Path,
    window: Option<&Window>,
    args: &Args,
    scan: &Scan,
) -> String {
    let shown = image.strip_prefix(root).unwrap_or(image);
    let mut report = format!(
        "Scanned {} {} of a {}x{} pixel image, in {}.\n\n",
        scan.scan, scan.at, scan.width, scan.height, scan.color_space
    );

    if let Some(window) = window {
        let scale = scan.width.checked_div(window.width).unwrap_or_default();
        report.push_str(&format!(
            "The window is {}x{} points, so the image is {scale}x: one point is {scale} \
             pixels.\n\n",
            window.width, window.height
        ));
    }

    report.push_str(&format!(
        "| {} | count | colour |\n| --- | --- | --- |\n",
        args.scan.along()
    ));

    for run in &scan.runs {
        report.push_str(&format!(
            "| {} | {} | `{}` |\n",
            run.start, run.count, run.color
        ));
    }

    if scan.runs.is_empty() {
        report.push_str("| | | *nothing in range* |\n");
    }

    report.push_str(&format!(
        "\nThe capture is at `{shown}`. Pass it as `image` to scan another line of the same \
         picture without disturbing the app.\n"
    ));

    report
}

#[cfg(test)]
#[path = "pixels_tests.rs"]
mod tests;
