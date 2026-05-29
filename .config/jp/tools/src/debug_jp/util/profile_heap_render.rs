//! Render a parsed dhat profile as a Markdown report.
//!
//! Three sections: headline stats, hot leaves aggregated by leaf frame, and the
//! top stacks by allocation count with their topmost frames.

use std::fmt::Write as _;

use crate::debug_jp::util::{
    launch::LaunchResult,
    profile_heap_parse::{Profile, ProgramPoint},
};

const TOP_LEAVES: usize = 40;
const TOP_STACKS: usize = 25;
const STACK_FRAMES: usize = 8;

/// Render the report.
pub(crate) fn render(
    profile: &Profile,
    launch: &LaunchResult,
    args: &[String],
    heap_path: &str,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# jp profile · heap (dhat)\n");
    let _ = writeln!(out, "**Command:** `jp {}`\n", args.join(" "));
    write_run_stats(&mut out, launch);
    write_headline(&mut out, profile);
    write_hot_leaves(&mut out, profile);
    write_hot_stacks(&mut out, profile);
    let _ = writeln!(out, "\n---\n\n*Raw dhat JSON: `{heap_path}`*");
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
    let _ = writeln!(out, "- **Wall clock:** {}", format_duration(launch.wall_duration));
    let _ = writeln!(out, "- **Status:** {status}");
    if !launch.success() && !launch.stderr.is_empty() {
        let _ = writeln!(out, "- **stderr (last 20 lines):**");
        let _ = writeln!(out, "  ```text");
        let tail: Vec<&str> = launch.stderr.lines().rev().take(20).collect();
        for line in tail.iter().rev() {
            let _ = writeln!(out, "  {line}");
        }
        let _ = writeln!(out, "  ```");
    }
    let _ = writeln!(out);
}

fn write_headline(out: &mut String, profile: &Profile) {
    let _ = writeln!(out, "## Headline");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "- **Total allocations:** {} blocks ({})",
        fmt_count(profile.total_blocks),
        fmt_bytes(profile.total_bytes)
    );
    let _ = writeln!(
        out,
        "- **At global peak:** {} blocks live ({})",
        fmt_count(profile.peak_blocks),
        fmt_bytes(profile.peak_bytes)
    );
    let _ = writeln!(
        out,
        "- **At program end:** {} blocks live ({})",
        fmt_count(profile.end_blocks),
        fmt_bytes(profile.end_bytes)
    );
    let _ = writeln!(
        out,
        "- **Unique stacks (PPs):** {}",
        fmt_count(profile.program_points.len() as u64)
    );
    if !profile.time_unit.is_empty() {
        let _ = writeln!(
            out,
            "- **Profile duration:** {} {}",
            fmt_count(profile.elapsed_units),
            profile.time_unit
        );
    }
    let _ = writeln!(out);
}

fn write_hot_leaves(out: &mut String, profile: &Profile) {
    let _ = writeln!(
        out,
        "## Hot leaves (top {TOP_LEAVES} by allocation count)"
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "| Blocks | Bytes | Sites | Symbol |");
    let _ = writeln!(out, "| -----: | ----: | ----: | :----- |");
    for leaf in profile.aggregate_by_leaf().iter().take(TOP_LEAVES) {
        let _ = writeln!(
            out,
            "| {} | {} | {} | `{}` |",
            fmt_count(leaf.total_blocks),
            fmt_bytes(leaf.total_bytes),
            leaf.sites,
            escape_pipes(&leaf.leaf),
        );
    }
    let _ = writeln!(out);
}

fn write_hot_stacks(out: &mut String, profile: &Profile) {
    let _ = writeln!(out, "## Hot stacks (top {TOP_STACKS} by allocation count)");
    let _ = writeln!(out);

    let mut indexed: Vec<&ProgramPoint> = profile.program_points.iter().collect();
    indexed.sort_by_key(|pp| std::cmp::Reverse(pp.total_blocks));

    for (rank, pp) in indexed.iter().take(TOP_STACKS).enumerate() {
        let _ = writeln!(
            out,
            "### #{rank} — {blocks} blocks, {bytes} (peak {peak})",
            rank = rank + 1,
            blocks = fmt_count(pp.total_blocks),
            bytes = fmt_bytes(pp.total_bytes),
            peak = fmt_bytes(pp.peak_bytes),
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "```text");
        // Drop the allocator-plumbing prefix (everything before the first
        // jp-prefixed frame). The literal leaf is always `<Global as
        // Allocator>::allocate` or similar; the interesting work is the
        // jp_ frame and its callers.
        let interesting = pp.interesting_leaf();
        let start = pp
            .frames
            .iter()
            .position(|f| f == interesting)
            .unwrap_or(0);
        let shown: Vec<&String> = pp.frames.iter().skip(start).take(STACK_FRAMES).collect();
        for (i, frame) in shown.iter().enumerate() {
            let arrow = if i == 0 { ">" } else { " " };
            let _ = writeln!(out, "{arrow} {frame}");
        }
        let total_remaining = pp.frames.len().saturating_sub(start + shown.len());
        if total_remaining > 0 {
            let _ = writeln!(out, "  ... ({total_remaining} more)");
        }
        if start > 0 {
            let _ = writeln!(out, "  (skipped {start} allocator/stdlib frames above leaf)");
        }
        let _ = writeln!(out, "```");
        let _ = writeln!(out);
    }
}

/// Formats a count in a compact, human-readable way.
///
/// The `as f64` casts on `u64` can lose precision above 2^53, but the formatted
/// output is already approximate (one or two decimal digits), so the lossy cast
/// doesn't affect what the user sees.
#[allow(clippy::cast_precision_loss)]
fn fmt_count(n: u64) -> String {
    if n < 10_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else if n < 1_000_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else {
        format!("{:.2}B", n as f64 / 1_000_000_000.0)
    }
}

/// Formats a byte count in a compact, human-readable way.
/// See [`fmt_count`] for the rationale on the lossy `u64 as f64` cast.
#[allow(clippy::cast_precision_loss)]
fn fmt_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let n_f = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if n_f < MIB {
        format!("{:.1} KiB", n_f / KIB)
    } else if n_f < GIB {
        format!("{:.1} MiB", n_f / MIB)
    } else {
        format!("{:.2} GiB", n_f / GIB)
    }
}

fn format_duration(d: std::time::Duration) -> String {
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
#[path = "profile_heap_render_tests.rs"]
mod tests;
