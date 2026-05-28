//! Render parsed `sample(1)` output as a Markdown report.
//!
//! Three sections — all sized to fit comfortably in an assistant's context
//! window:
//!
//! 1. Headline stats: total samples, wall-clock duration, exit status.
//! 2. Top hot leaves across the main thread, aggregated by demangled symbol.
//! 3. The deepest stacks on the main thread, with their frames.
//!
//! "Main thread" here means the first thread emitted by `sample(1)`, which is
//! the one tied to the process's primary dispatch queue.
//! Worker threads parked in `kevent` / `pthread_cond_wait` carry no signal for
//! a CLI profiling run, so we deliberately omit them.

use std::{fmt::Write as _, time::Duration};

use crate::debug_jp::util::{
    launch::LaunchResult,
    profile_sampling_parse::{Frame, Thread},
};

/// Top-N for each section.
/// Tuned to keep the rendered report under a couple of pages while still
/// showing enough leaves to spot patterns.
const TOP_LEAVES: usize = 30;
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
    write_hot_leaves(&mut out, threads);
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

fn write_hot_leaves(out: &mut String, threads: &[Thread]) {
    let _ = writeln!(
        out,
        "## Hot leaves (main thread, top {TOP_LEAVES} by self-sum)"
    );
    let _ = writeln!(out);
    let Some(main) = threads.first() else {
        let _ = writeln!(out, "*No data.*\n");
        return;
    };
    let aggregate = main.aggregate_by_symbol();
    let _ = writeln!(out, "| Samples | Symbol |");
    let _ = writeln!(out, "| ------: | :----- |");
    for (symbol, samples) in aggregate.iter().take(TOP_LEAVES) {
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
/// Walks backward looking for the first frame at `anchor_depth - 1`, then
/// `- 2`, and so on until the root is reached or `max_depth` is hit.
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

#[cfg(test)]
#[path = "profile_sampling_render_tests.rs"]
mod tests;
