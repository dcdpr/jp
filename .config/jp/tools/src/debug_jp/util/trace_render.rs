//! Render trace events in a compact, scannable format inspired by
//! [`tracing_subscriber::fmt::format::Compact`].
//!
//! One line per event:
//!
//! ```text
//! HH:MM:SS.mmm  LEVEL  target  message  k=v k="quoted v"  [span > span]
//! ```
//!
//! - Timestamp trimmed to time-of-day with millisecond precision; the date is
//!   fixed for a single profile run and shown in the report headline.
//! - Level left-padded to 5 chars so subsequent columns align.
//! - Target padded to the width of the widest target in the rendered subset
//!   (capped at [`TARGET_PAD_CAP`]).
//! - Fields rendered as `k=v` pairs in their original insertion order.
//!   `v` is unquoted when it's a simple token; quoted with backslash-escaped
//!   quotes otherwise.
//!   Values are never truncated — reports go to an assistant that can parse
//!   long lines.
//! - Span stack appended as `[outer > inner]` when present.
//! - Runs of consecutive byte-identical events (same level/target/message/
//!   fields/spans, timestamp ignored) collapse into one line suffixed `× N`.
//!   Different values — same target+message with different fields, for
//!   instance — are kept distinct so the noise of "same call, varying args"
//!   remains visible.
//!
//! [`tracing_subscriber::fmt::format::Compact`]: https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/format/struct.Compact.html

use std::{fmt::Write as _, time::Duration};

use serde_json::Value;

use crate::debug_jp::util::{
    launch::LaunchResult,
    trace_parse::{self, TraceEvent},
};

/// Hard ceiling on target column padding.
/// Targets longer than this are printed at full width, breaking alignment for
/// that row but not blowing the column out for the whole report.
const TARGET_PAD_CAP: usize = 40;

/// Hard cap on event lines emitted to the report.
/// Beyond this we truncate with a hint to narrow the filter; the full JSONL is
/// still on disk.
const SOFT_CAP: usize = 400;

/// Paths to the sidecar files written alongside the rendered report.
///
/// Bundled to keep [`render`]'s signature readable.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputPaths<'a> {
    pub trace: &'a str,
    pub stdout: &'a str,
    pub stderr: &'a str,
}

/// Render the report.
pub(crate) fn render(
    events: &[TraceEvent],
    total: usize,
    launch: &LaunchResult,
    args: &[String],
    paths: OutputPaths<'_>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# jp debug · trace\n");
    let _ = writeln!(out, "**Command:** `jp {}`\n", args.join(" "));
    write_body(&mut out, events, total, launch, "##");
    write_footer(&mut out, paths);
    out
}

/// A single command's rendered inputs within a sequence.
pub(crate) struct CommandRun<'a> {
    pub args: &'a [String],
    pub events: &'a [TraceEvent],
    pub total: usize,
    pub launch: &'a LaunchResult,
    pub paths: OutputPaths<'a>,
}

/// Render a sequence of commands that shared one sandbox into a single report.
///
/// One `## Command i/n` section per command, each with `###`-level subsections,
/// followed by a combined file footer.
/// State persists across the commands because they share the sandbox's
/// user-data directory.
pub(crate) fn render_multi(runs: &[CommandRun<'_>]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# jp debug · trace\n");
    let _ = writeln!(
        out,
        "Ran {} commands in sequence in one sandbox; state persisted across them.\n",
        runs.len()
    );
    for (i, run) in runs.iter().enumerate() {
        let _ = writeln!(
            out,
            "## Command {}/{}: `jp {}`\n",
            i + 1,
            runs.len(),
            run.args.join(" ")
        );
        if let Some(note) = run.launch.note() {
            let _ = writeln!(out, "> [!WARNING]\n> {note}\n");
        }
        write_body(&mut out, run.events, run.total, run.launch, "###");
    }
    write_multi_footer(&mut out, runs);
    out
}

/// One command's section body: run stats, streams, and the trace summary plus
/// events, written under `heading`-level subsections (`##` for a standalone
/// report, `###` when nested under a per-command header in a sequence).
fn write_body(
    out: &mut String,
    events: &[TraceEvent],
    total: usize,
    launch: &LaunchResult,
    heading: &str,
) {
    write_run_stats(out, launch, heading);
    write_streams(out, launch, heading);
    write_summary(out, events.len(), total, heading);
    write_events(out, events);
}

fn write_run_stats(out: &mut String, launch: &LaunchResult, heading: &str) {
    let _ = writeln!(out, "{heading} Run");
    let _ = writeln!(out);
    let status = match launch.exit_code {
        Some(0) => "success".to_owned(),
        Some(code) => format!("exit {code}"),
        None => "terminated by signal".to_owned(),
    };
    let _ = writeln!(
        out,
        "- **Wall clock:** {}",
        format_duration(launch.wall_duration)
    );
    let _ = writeln!(out, "- **Status:** {status}");
    let _ = writeln!(out);
}

fn write_summary(out: &mut String, shown: usize, total: usize, heading: &str) {
    let _ = writeln!(out, "{heading} Trace");
    let _ = writeln!(out);
    if shown == total {
        let _ = writeln!(out, "- **Events:** {total}");
    } else {
        let _ = writeln!(out, "- **Events:** {shown} shown / {total} total");
    }
    let _ = writeln!(out);
}

fn write_events(out: &mut String, events: &[TraceEvent]) {
    if events.is_empty() {
        let _ = writeln!(out, "*No events match the current filter.*\n");
        return;
    }

    let to_show = events.len().min(SOFT_CAP);
    let target_width = events[..to_show]
        .iter()
        .map(|e| e.target.len())
        .max()
        .unwrap_or(0)
        .min(TARGET_PAD_CAP);

    let _ = writeln!(out, "```text");
    let mut i = 0;
    while i < to_show {
        let event = &events[i];
        let mut run_end = i + 1;
        while run_end < to_show && events_identical(&events[run_end], event) {
            run_end += 1;
        }
        write_event(out, event, target_width, run_end - i);
        i = run_end;
    }
    if events.len() > to_show {
        let _ = writeln!(
            out,
            "… {} more events omitted (narrow with `level`, `target`, or `grep`)",
            events.len() - to_show
        );
    }
    let _ = writeln!(out, "```");
}

/// Two events are "identical" iff every field except `timestamp` matches.
fn events_identical(a: &TraceEvent, b: &TraceEvent) -> bool {
    a.level == b.level
        && a.target == b.target
        && a.message == b.message
        && a.spans == b.spans
        && a.fields == b.fields
}

fn write_event(out: &mut String, event: &TraceEvent, target_width: usize, run_count: usize) {
    let time = format_time(&event.timestamp);
    let _ = write!(out, "{time}  ");
    let _ = write!(out, "{:<5}  ", event.level.as_str());
    let _ = write!(out, "{:<width$}  ", event.target, width = target_width);
    let _ = write!(out, "{}", event.message);

    for (k, v) in &event.fields {
        let _ = write!(out, "  {k}={}", format_value(v));
    }

    if !event.spans.is_empty() {
        let _ = write!(out, "  [{}]", event.spans.join(" > "));
    }

    if run_count > 1 {
        let _ = write!(out, "  × {run_count}");
    }
    let _ = writeln!(out);
}

/// Render captured stdout and stderr, each in its own fenced section.
///
/// The marker line jp writes to stderr to advertise the trace log path is
/// stripped here — it's noise once you can already see the path in the report
/// footer.
fn write_streams(out: &mut String, launch: &LaunchResult, heading: &str) {
    if !launch.stdout.is_empty() {
        let _ = writeln!(out, "{heading} stdout\n");
        let _ = writeln!(out, "```text");
        let _ = writeln!(out, "{}", launch.stdout.trim_end());
        let _ = writeln!(out, "```\n");
    }

    let stderr = strip_trace_path_marker(&launch.stderr);
    if !stderr.trim().is_empty() {
        let _ = writeln!(out, "{heading} stderr\n");
        let _ = writeln!(out, "```text");
        let _ = writeln!(out, "{}", stderr.trim_end());
        let _ = writeln!(out, "```\n");
    }
}

fn write_footer(out: &mut String, paths: OutputPaths<'_>) {
    let _ = writeln!(out, "\n---\n");
    let _ = writeln!(out, "**Files:**\n");
    let _ = writeln!(out, "- Trace: `{}`", paths.trace);
    let _ = writeln!(out, "- Stdout: `{}`", paths.stdout);
    let _ = writeln!(out, "- Stderr: `{}`", paths.stderr);
}

fn write_multi_footer(out: &mut String, runs: &[CommandRun<'_>]) {
    let _ = writeln!(out, "\n---\n");
    let _ = writeln!(out, "**Files:**\n");
    for (i, run) in runs.iter().enumerate() {
        let _ = writeln!(out, "- Command {} trace: `{}`", i + 1, run.paths.trace);
        let _ = writeln!(out, "- Command {} stdout: `{}`", i + 1, run.paths.stdout);
        let _ = writeln!(out, "- Command {} stderr: `{}`", i + 1, run.paths.stderr);
    }
}

/// Drop the stderr line jp emits to announce the trace log path (text marker or
/// `trace_log` JSON field, depending on `--format`).
/// The path is already in the footer.
fn strip_trace_path_marker(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| !trace_parse::is_trace_path_marker_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract `HH:MM:SS.mmm` from an RFC3339 timestamp.
///
/// `tracing-subscriber`'s JSON output uses UTC with a `Z` suffix, e.g.
/// `2026-05-25T21:44:04.572200Z`.
/// We chop off the date (shown in the headline anyway) and trim fractional
/// seconds to milliseconds.
fn format_time(ts: &str) -> String {
    let after_t = ts.split_once('T').map_or(ts, |(_, t)| t);
    let without_z = after_t.trim_end_matches('Z');
    if let Some((sec, frac)) = without_z.split_once('.') {
        let frac_truncated: String = frac.chars().take(3).collect();
        format!("{sec}.{frac_truncated}")
    } else {
        without_z.to_owned()
    }
}

fn format_value(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string(v).unwrap_or_else(|_| "?".to_owned())
        }
    };
    if needs_quoting(&s) {
        format!("\"{}\"", escape(&s))
    } else {
        s
    }
}

fn needs_quoting(s: &str) -> bool {
    s.is_empty() || s.chars().any(|c| c.is_whitespace() || c == '=' || c == '"')
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0} ms", secs * 1000.0)
    } else {
        format!("{secs:.2} s")
    }
}

#[cfg(test)]
#[path = "trace_render_tests.rs"]
mod tests;
