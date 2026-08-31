//! Render parsed `sample(1)` output as a Markdown report.
//!
//! Four sections, all sized to fit comfortably in an assistant's context
//! window:
//!
//! 1. Headline stats: thread count, wall-clock duration, exit status.
//! 2. Hot code by *self* time across every thread — where cycles actually go.
//! 3. Inclusive totals on the main thread — which call paths dominate.
//! 4. The heaviest stacks on the main thread, with their frames.
//!
//! Section 2 spans all threads on purpose.
//! `jp` parallelizes with rayon, so on a workload like `conversation grep` the
//! main thread is parked on a latch and every frame worth reading lives on a
//! worker.
//! Idle parking frames are partitioned out of the leaderboard and reported as a
//! total plus a per-symbol breakdown, so the numbers still reconcile and a
//! thread genuinely blocked in `kevent` for the whole run stays visible.

use std::{fmt::Write as _, time::Duration};

use crate::debug_jp::util::{
    launch::LaunchResult,
    profile_sampling_parse::{Frame, Thread, is_idle_symbol, self_samples_by_symbol},
};

/// Top-N for each section.
/// Tuned to keep the rendered report under a couple of pages while still
/// showing enough symbols to spot patterns.
const TOP_SYMBOLS: usize = 30;
const TOP_IDLE: usize = 5;
const TOP_STACKS: usize = 15;
const STACK_FRAMES: usize = 10;

/// Render the report.
pub(crate) fn render(
    threads: &[Thread],
    launch: &LaunchResult,
    args: &[String],
    sample_path: &str,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# jp profile · sampling\n");
    let _ = writeln!(out, "**Command:** `jp {}`\n", args.join(" "));
    write_run_stats(&mut out, launch);
    write_headline(&mut out, threads);
    write_hot_code(&mut out, threads);
    write_inclusive_totals(&mut out, threads);
    write_hot_stacks(&mut out, threads);
    let _ = writeln!(out, "\n---\n\n*Raw `sample(1)` output: `{sample_path}`*");
    out
}

fn write_run_stats(out: &mut String, launch: &LaunchResult) {
    let _ = writeln!(out, "## Run");
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
    if !launch.success() && !launch.stderr.is_empty() {
        let _ = writeln!(out, "- **stderr (last 20 lines):**");
        let _ = writeln!(out, "  ```text");
        for line in launch
            .stderr
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .iter()
            .rev()
        {
            let _ = writeln!(out, "  {line}");
        }
        let _ = writeln!(out, "  ```");
    }
    let _ = writeln!(out);
}

fn write_headline(out: &mut String, threads: &[Thread]) {
    let _ = writeln!(out, "## Headline");
    let _ = writeln!(out);
    let Some(main) = threads.first() else {
        let _ = writeln!(out, "*No threads sampled — profile run was too short.*\n");
        return;
    };
    let total = main.frames.iter().map(|f| f.samples).max().unwrap_or(0);
    let _ = writeln!(out, "- **Threads:** {}", threads.len());
    let _ = writeln!(out, "- **Main thread:** `{}`", main.header);
    let _ = writeln!(out, "- **Main-thread samples (max frame):** {total}");
    let _ = writeln!(out);
}

/// Self-time leaderboard across every thread, with idle parking separated out.
fn write_hot_code(out: &mut String, threads: &[Thread]) {
    let _ = writeln!(
        out,
        "## Hot code (self time, all threads, top {TOP_SYMBOLS})"
    );
    let _ = writeln!(out);

    let all = self_samples_by_symbol(threads);
    if all.is_empty() {
        let _ = writeln!(out, "*No data.*\n");
        return;
    }

    let (idle, busy): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|(symbol, _)| is_idle_symbol(symbol));
    let idle_total: u64 = idle.iter().map(|entry| entry.1).sum();
    let busy_total: u64 = busy.iter().map(|entry| entry.1).sum();

    let _ = write!(
        out,
        "Working: **{busy_total}** samples. Parked/idle: {idle_total} (excluded below)"
    );
    // Name the top idle symbols: a command that is slow because it waits on
    // HTTP, MCP, or a subprocess parks in `kevent` / `mach_msg2_trap`, and a
    // bare total would hide which wait it was.
    if idle.is_empty() {
        let _ = writeln!(out, ".\n");
    } else {
        let breakdown: Vec<String> = idle
            .iter()
            .take(TOP_IDLE)
            .map(|(symbol, samples)| format!("`{symbol}` {samples}"))
            .collect();
        let _ = writeln!(out, ": {}.\n", breakdown.join(", "));
    }
    let _ = writeln!(out, "| Self | Share | Symbol |");
    let _ = writeln!(out, "| ---: | ----: | :----- |");
    for (symbol, samples) in busy.iter().take(TOP_SYMBOLS) {
        // Percent to one decimal, in integer arithmetic: sample counts are far
        // below the range where the f64 cast would matter, but the lint is right
        // that the cast has no business here.
        let tenths = samples
            .saturating_mul(1000)
            .checked_div(busy_total)
            .unwrap_or(0);
        let _ = writeln!(
            out,
            "| {samples} | {}.{}% | `{}` |",
            tenths / 10,
            tenths % 10,
            escape_pipes(symbol)
        );
    }
    let _ = writeln!(out);
}

/// Inclusive per-symbol totals on the main thread: which call paths dominate.
fn write_inclusive_totals(out: &mut String, threads: &[Thread]) {
    let _ = writeln!(out, "## Inclusive totals (main thread, top {TOP_SYMBOLS})");
    let _ = writeln!(out);
    let Some(main) = threads.first() else {
        let _ = writeln!(out, "*No data.*\n");
        return;
    };
    let aggregate = main.aggregate_by_symbol();
    let _ = writeln!(out, "| Samples | Symbol |");
    let _ = writeln!(out, "| ------: | :----- |");
    for (symbol, samples) in aggregate.iter().take(TOP_SYMBOLS) {
        let _ = writeln!(out, "| {samples} | `{}` |", escape_pipes(symbol));
    }
    let _ = writeln!(out);
}

fn write_hot_stacks(out: &mut String, threads: &[Thread]) {
    let _ = writeln!(
        out,
        "## Hot stacks (main thread, top {TOP_STACKS} frames by sample count)"
    );
    let _ = writeln!(out);
    let Some(main) = threads.first() else {
        let _ = writeln!(out, "*No data.*\n");
        return;
    };

    // Sort frames by sample count, pick top N as anchors, then for each
    // reconstruct the ancestry chain from the anchor up to the root.
    let mut indexed: Vec<(usize, &Frame)> = main.frames.iter().enumerate().collect();
    indexed.sort_by_key(|entry| std::cmp::Reverse(entry.1.samples));

    for (rank, (index, frame)) in indexed.iter().take(TOP_STACKS).enumerate() {
        let _ = writeln!(
            out,
            "### #{rank} — {samples} samples @ depth {depth}",
            rank = rank + 1,
            samples = frame.samples,
            depth = frame.depth,
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "```text");

        let ancestry = build_ancestry(&main.frames, *index, STACK_FRAMES);
        let base_depth = ancestry.first().map_or(frame.depth, |f| f.depth);
        let last = ancestry.len().saturating_sub(1);
        for (i, f) in ancestry.iter().enumerate() {
            let arrow = if i == last { ">" } else { " " };
            let _ = writeln!(
                out,
                "{arrow} {pad}{samples:>8}  {sym}",
                pad = "  ".repeat(f.depth.saturating_sub(base_depth)),
                samples = f.samples,
                sym = f.symbol,
            );
        }
        let _ = writeln!(out, "```");
        let _ = writeln!(out);
    }
}

/// Reconstruct the ancestry chain from `frames[anchor_idx]` up to the root,
/// limited to `max_depth` entries total (anchor included).
///
/// `sample(1)` emits frames in depth-first preorder, so an anchor's ancestors
/// are the closest preceding frames at each successively-shallower depth.
/// Walks backward looking for the first frame at `anchor_depth - 1`, then `-
/// 2`, and so on until the root is reached or `max_depth` is hit.
fn build_ancestry(frames: &[Frame], anchor_idx: usize, max_depth: usize) -> Vec<&Frame> {
    let anchor = &frames[anchor_idx];
    let mut path = vec![anchor];
    if anchor.depth == 0 || max_depth <= 1 {
        return path;
    }
    let mut needed = anchor.depth - 1;
    for i in (0..anchor_idx).rev() {
        if path.len() >= max_depth {
            break;
        }
        if frames[i].depth == needed {
            path.push(&frames[i]);
            if needed == 0 {
                break;
            }
            needed -= 1;
        }
    }
    path.reverse();
    path
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0} ms", secs * 1000.0)
    } else {
        format!("{secs:.2} s")
    }
}

fn escape_pipes(s: &str) -> String {
    s.replace('|', "\\|")
}

#[cfg(all(test, unix))]
#[path = "profile_sampling_render_tests.rs"]
mod tests;
