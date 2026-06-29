use std::fmt::Write;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use super::env_from_options;
use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessRunner},
};

/// Maximum number of untracked files to list before truncating.
///
/// Tracked changes are always shown in full; only the untracked listing is
/// capped, so a large untracked directory (e.g. non-ignored build output) can't
/// bury the tracked edits this guard exists to surface.
const MAX_UNTRACKED: usize = 500;

/// A single entry from `git status --porcelain`.
#[derive(Debug, PartialEq)]
struct StatusEntry {
    /// The two-character XY status code (e.g.
    /// `" M"`, `"??"`, `"A "`).
    code: String,
    /// The path, or `old -> new` for renames and copies.
    path: String,
}

pub(crate) async fn git_status(root: Utf8PathBuf, options: &Map<String, Value>) -> ToolResult {
    let env = env_from_options(options);
    git_status_impl(&root, &DuctProcessRunner, &env)
}

fn git_status_impl<R: ProcessRunner>(
    root: &Utf8Path,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    // `core.quotePath=false` keeps non-ASCII paths readable instead of
    // octal-escaped. `--untracked-files=all` expands untracked directories to
    // individual files (the default collapses them to `dir/`), so the guard
    // reports the actual paths an assistant needs to rule out local edits.
    let output = runner.run_with_env(
        "git",
        &[
            "-c",
            "core.quotePath=false",
            "status",
            "--porcelain",
            "--untracked-files=all",
        ],
        root,
        env,
    )?;

    if !output.status.is_success() {
        return error(format!("git status failed: {}", output.stderr.trim()));
    }

    let entries = parse_status(&output.stdout);
    Ok(format_status(&entries).into())
}

/// Parse `git status --porcelain` v1 output into per-file entries.
///
/// Each line is `XY<space>PATH`, where `XY` is the two-character status code
/// and `PATH` is `old -> new` for renames and copies.
fn parse_status(stdout: &str) -> Vec<StatusEntry> {
    stdout
        .lines()
        .filter_map(|line| {
            // Shortest valid line is `XY P` (code, space, one path char).
            if line.len() < 4 {
                return None;
            }
            let (code, rest) = line.split_at(2);
            Some(StatusEntry {
                code: code.to_string(),
                // Strip exactly the one separator space after the XY code; a
                // path that itself begins with spaces must survive intact.
                path: rest.strip_prefix(' ').unwrap_or(rest).to_string(),
            })
        })
        .collect()
}

fn format_status(entries: &[StatusEntry]) -> String {
    if entries.is_empty() {
        return "Working tree clean.".to_string();
    }

    let (untracked, tracked): (Vec<&StatusEntry>, Vec<&StatusEntry>) =
        entries.iter().partition(|e| e.code == "??");

    let mut out = String::from("<git_status>\n");

    for e in tracked {
        let _ = writeln!(out, "  - {} ({})", e.path, describe(&e.code));
    }

    for e in untracked.iter().take(MAX_UNTRACKED) {
        let _ = writeln!(out, "  - {} ({})", e.path, describe(&e.code));
    }

    if untracked.len() > MAX_UNTRACKED {
        let _ = writeln!(
            out,
            "  ... and {} more untracked files not shown (output truncated).",
            untracked.len() - MAX_UNTRACKED
        );
    }

    out.push_str("</git_status>");
    out
}

/// Decode a porcelain XY status code into a human-readable description.
///
/// The index (staged) and worktree (unstaged) columns are reported separately,
/// so `MM` becomes "modified, staged; modified, unstaged".
fn describe(code: &str) -> String {
    if code == "??" {
        return "untracked".to_string();
    }
    if code == "!!" {
        return "ignored".to_string();
    }

    let mut chars = code.chars();
    let index = chars.next().unwrap_or(' ');
    let worktree = chars.next().unwrap_or(' ');

    let mut parts = Vec::new();
    if let Some(word) = describe_char(index) {
        parts.push(format!("{word}, staged"));
    }
    if let Some(word) = describe_char(worktree) {
        parts.push(format!("{word}, unstaged"));
    }

    if parts.is_empty() {
        code.trim().to_string()
    } else {
        parts.join("; ")
    }
}

fn describe_char(c: char) -> Option<&'static str> {
    match c {
        'M' => Some("modified"),
        'A' => Some("added"),
        'D' => Some("deleted"),
        'R' => Some("renamed"),
        'C' => Some("copied"),
        'U' => Some("unmerged"),
        'T' => Some("type changed"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
