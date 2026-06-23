//! `git_blame`: show which commits last touched each line in a range.
//!
//! Output groups contiguous lines that share a blamed commit so token cost
//! scales with the number of distinct commits, not the number of lines.
//! Each group also carries the porcelain `previous <sha> <path>` field as
//! `previous`, giving the assistant a free drill-down to the prior owner of the
//! line without a second subprocess.

use std::{collections::HashMap, fmt::Write};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{FixedOffset, TimeZone};
use serde_json::{Map, Value};

use super::env_from_options;
use crate::{
    Result,
    util::{
        ToolResult, error,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// Upper bound on the requested line range.
/// A blame request with a wider range is rejected.
/// This mirrors the rationale behind `git_diff_commit`'s `paths` requirement:
/// keep the worst-case output size bounded.
const MAX_RANGE: usize = 200;

/// Length of a full git SHA-1.
/// Used to distinguish porcelain header lines (which start with a 40-char hex
/// sha) from metadata lines.
const SHA_LEN: usize = 40;

#[derive(Debug, Default, PartialEq)]
struct CommitMetadata {
    author: String,
    date: String,
    summary: String,
}

#[derive(Debug, PartialEq)]
struct BlameLine {
    sha: String,
    final_line: usize,
    content: String,
    /// Commit that owned this line before the change attributed to `sha`, taken
    /// from porcelain's `previous <sha> <path>` field.
    /// Kept per-line (not per-commit) because that field is path-origin
    /// metadata: a single commit can legitimately have *different* prior
    /// commits for different line groups when copy/move detection is in play,
    /// and porcelain re-emits `previous` for every group of such commits.
    previous: Option<String>,
}

#[derive(Debug, PartialEq)]
struct BlameOutput {
    file: String,
    revision: Option<String>,
    start_line: usize,
    end_line: usize,
    commits: HashMap<String, CommitMetadata>,
    lines: Vec<BlameLine>,
}

pub(crate) async fn git_blame(
    root: Utf8PathBuf,
    path: String,
    start_line: usize,
    end_line: usize,
    revision: Option<String>,
    ignore_whitespace: Option<bool>,
    options: &Map<String, Value>,
) -> ToolResult {
    let env = env_from_options(options);

    git_blame_impl(
        &root,
        &path,
        start_line,
        end_line,
        revision.as_deref(),
        ignore_whitespace.unwrap_or(false),
        &DuctProcessRunner,
        &env,
    )
}

fn git_blame_impl<R: ProcessRunner>(
    root: &Utf8Path,
    path: &str,
    start_line: usize,
    end_line: usize,
    revision: Option<&str>,
    ignore_whitespace: bool,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    if start_line == 0 {
        return error("`start_line` must be greater than 0.");
    }

    if start_line > end_line {
        return error("`start_line` must be less than or equal to `end_line`.");
    }

    let range = end_line - start_line + 1;
    if range > MAX_RANGE {
        return error(format!(
            "Requested range ({range} lines) exceeds the cap of {MAX_RANGE}. Narrow the range or \
             split into multiple calls."
        ));
    }

    // `git blame` mishandles the `--end-of-options <rev> -- <path>` form: its
    // argument DWIM treats the first token after the consumed options as the
    // `--` position and reorders the path into the revision slot, so the path
    // is then rejected as a bad revision. Resolve the revision to a full SHA
    // up front instead. `git rev-parse` honors `--end-of-options` correctly,
    // so an option-shaped value (e.g. `--contents=<file>`, which would let a
    // caller read arbitrary files) still reaches it as a positional and fails
    // resolution rather than executing. The resulting 40-char hex SHA can't be
    // mistaken for an option, so the blame call needs no `--end-of-options`.
    let resolved_rev = match revision {
        Some(rev) => {
            let rev_args = ["rev-parse", "--verify", "--end-of-options", rev];
            let output = runner.run_with_env("git", &rev_args, root, env)?;
            if !output.status.is_success() {
                return error(format!("git rev-parse failed: {}", output.stderr.trim()));
            }
            Some(output.stdout.trim().to_string())
        }
        None => None,
    };

    let range_arg = format!("-L{start_line},{end_line}");
    let mut args: Vec<&str> = vec!["blame", "--porcelain", &range_arg];

    if ignore_whitespace {
        args.push("-w");
    }

    if let Some(rev) = resolved_rev.as_deref() {
        args.push(rev);
    }

    args.push("--");
    args.push(path);

    let output = runner.run_with_env("git", &args, root, env)?;

    if !output.status.is_success() {
        return error(format!("git blame failed: {}", output.stderr.trim()));
    }

    let blame = parse_porcelain(&output.stdout, path, revision, start_line, end_line)?;

    if blame.lines.is_empty() {
        return Ok("No blame information returned for the specified range.".into());
    }

    Ok(format_blame(&blame)?.into())
}

/// Parse `git blame --porcelain` output.
///
/// Porcelain format (per `git-blame(1)`):
///
/// - Header line: `<sha> <orig-line> <final-line> <group-size>`.
/// - First appearance of a sha is followed by metadata lines: `author`,
///   `author-mail`, `author-time`, `author-tz`, `committer*`, `summary`,
///   optional `previous <sha> <path>`, `filename <path>`.
/// - Subsequent appearances of the same sha emit only the header line.
/// - Content line: a single tab character followed by the raw source line.
fn parse_porcelain(
    stdout: &str,
    path: &str,
    revision: Option<&str>,
    start_line: usize,
    end_line: usize,
) -> Result<BlameOutput> {
    let mut commits: HashMap<String, CommitMetadata> = HashMap::new();
    let mut lines: Vec<BlameLine> = Vec::new();

    let mut current_sha: Option<String> = None;
    let mut current_final_line: usize = 0;
    let mut current_meta = CommitMetadata::default();
    let mut current_author_time: Option<i64> = None;
    let mut current_author_tz: Option<String> = None;
    // `previous` for the block being parsed *right now*. Reset at every
    // header line and updated when a `previous` metadata line is seen.
    let mut current_block_previous: Option<String> = None;
    // Whether the current block emitted a `filename` line. Per
    // `builtin/blame.c::emit_porcelain_details`, porcelain emits
    // `filename` whenever it emits path-origin metadata — i.e. on first
    // appearance of a commit OR on any appearance when
    // `MORE_THAN_ONE_PATH` is set. Its presence is the signal that this
    // block's `previous` is fully specified, including its meaningful
    // *absence* ("no prior commit for this origin"). Its absence means
    // porcelain suppressed path-origin metadata because it would be
    // redundant (single-path-repeat), and we should inherit the commit's
    // last known `previous` instead.
    let mut current_block_has_filename: bool = false;
    // Last `previous` value seen for each commit, used only as the
    // fallback for single-path-repeat blocks (blocks without their own
    // `filename` line).
    let mut last_known_previous: HashMap<String, Option<String>> = HashMap::new();

    for raw in stdout.lines() {
        if let Some(content) = raw.strip_prefix('\t') {
            let sha = current_sha
                .as_deref()
                .ok_or("malformed porcelain output: content line before header")?
                .to_string();

            if !commits.contains_key(&sha) {
                if let (Some(secs), Some(tz)) = (current_author_time, current_author_tz.as_deref())
                {
                    current_meta.date = format_author_date(secs, tz);
                }
                commits.insert(sha.clone(), std::mem::take(&mut current_meta));
                current_author_time = None;
                current_author_tz = None;
            }

            // Resolve the effective `previous` for this line. If the
            // block emitted its own path-origin metadata (a `filename`
            // line), its `previous` is fully specified — use it directly,
            // even when that value is `None` ("no prior commit for this
            // origin"), and record it as the latest known for the commit.
            // Otherwise this is a single-path-repeat block: porcelain
            // suppressed path-origin metadata as redundant, and we
            // inherit the commit's last known `previous`.
            let previous = if current_block_has_filename {
                last_known_previous.insert(sha.clone(), current_block_previous.clone());
                current_block_previous.clone()
            } else {
                last_known_previous.get(&sha).cloned().unwrap_or(None)
            };

            lines.push(BlameLine {
                sha,
                final_line: current_final_line,
                content: content.to_string(),
                previous,
            });

            continue;
        }

        // Header: starts with a 40-char hex SHA followed by a space. Header
        // fields are `<sha> <orig> <final> <group>`. We only need `final`.
        let mut parts = raw.splitn(4, ' ');
        let first = parts.next().unwrap_or_default();
        if is_sha(first) {
            let _orig = parts.next();
            let final_line = parts
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or_else(|| format!("malformed porcelain header: `{raw}`"))?;

            current_sha = Some(first.to_string());
            current_final_line = final_line;
            // Reset the metadata buffer for the next "first appearance" of a
            // sha. Already-known shas don't re-emit metadata so the buffer
            // simply stays unused until the next new sha.
            current_meta = CommitMetadata::default();
            current_author_time = None;
            current_author_tz = None;
            // `previous` and `filename` are path-origin metadata and can
            // be re-emitted for already-seen shas (multi-path commits).
            // Reset both at every header; if the block re-emits them
            // we'll pick the new values up. If `filename` is absent for
            // the rest of the block, that's a single-path-repeat and we
            // fall back to `last_known_previous`.
            current_block_previous = None;
            current_block_has_filename = false;

            continue;
        }

        // Metadata key/value line. `key value` with a single space delimiter.
        let Some((key, value)) = raw.split_once(' ') else {
            continue;
        };

        match key {
            "author" => current_meta.author = value.to_string(),
            "author-time" => current_author_time = value.parse().ok(),
            "author-tz" => current_author_tz = Some(value.to_string()),
            "summary" => current_meta.summary = value.to_string(),
            "previous" => {
                // `previous <sha> <path>` — keep just the sha for drill-down.
                let sha = value.split_whitespace().next().unwrap_or_default();
                if is_sha(sha) {
                    current_block_previous = Some(sha.to_string());
                }
            }
            "filename" => current_block_has_filename = true,
            _ => {}
        }
    }

    Ok(BlameOutput {
        file: path.to_string(),
        revision: revision.map(str::to_string),
        start_line,
        end_line,
        commits,
        lines,
    })
}

fn is_sha(s: &str) -> bool {
    s.len() == SHA_LEN && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Convert porcelain's `author-time` (epoch seconds) + `author-tz` (`±HHMM`) to
/// ISO 8601, matching the format used by `git_log` (`%aI`).
fn format_author_date(secs: i64, tz: &str) -> String {
    parse_tz(tz)
        .and_then(|offset| offset.timestamp_opt(secs, 0).single())
        .map_or_else(
            || format!("{secs} {tz}"),
            |dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        )
}

fn parse_tz(tz: &str) -> Option<FixedOffset> {
    let bytes = tz.as_bytes();
    if bytes.len() != 5 {
        return None;
    }
    let sign = match bytes[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours: i32 = tz.get(1..3)?.parse().ok()?;
    let mins: i32 = tz.get(3..5)?.parse().ok()?;
    FixedOffset::east_opt(sign * (hours * 3600 + mins * 60))
}

struct LineGroup<'a> {
    sha: String,
    previous: Option<String>,
    lines: Vec<&'a BlameLine>,
}

/// Group consecutive lines that share a sha AND the same `previous` AND are
/// line-number contiguous.
/// A gap in line numbers, a different sha, or a different prior origin all
/// start a new group, so the rendered output doesn't imply a continuous block
/// where there isn't one.
/// With porcelain's `-L <start>,<end>` range lines are always line-number
/// contiguous; the `previous` check is what actually splits multi-path commits
/// where the same sha has different prior origins for different line groups.
fn group_lines(lines: &[BlameLine]) -> Vec<LineGroup<'_>> {
    let mut groups: Vec<LineGroup<'_>> = Vec::new();

    for line in lines {
        let extend = groups.last().is_some_and(|g| {
            g.sha == line.sha
                && g.previous == line.previous
                && g.lines
                    .last()
                    .is_some_and(|prev| prev.final_line + 1 == line.final_line)
        });

        if extend {
            groups.last_mut().expect("checked above").lines.push(line);
        } else {
            groups.push(LineGroup {
                sha: line.sha.clone(),
                previous: line.previous.clone(),
                lines: vec![line],
            });
        }
    }

    groups
}

fn format_blame(blame: &BlameOutput) -> Result<String> {
    let mut out = String::from("<git_blame>\n");
    writeln!(out, "  <file>{}</file>", blame.file)?;
    let rev = blame.revision.as_deref().unwrap_or("working tree");
    writeln!(out, "  <revision>{rev}</revision>")?;
    writeln!(
        out,
        "  <range>{}-{}</range>",
        blame.start_line, blame.end_line
    )?;

    for group in group_lines(&blame.lines) {
        let meta = blame
            .commits
            .get(&group.sha)
            .ok_or_else(|| format!("missing metadata for sha {} in porcelain output", group.sha))?;

        writeln!(out, "  <hunk>")?;
        writeln!(out, "    hash: {}", group.sha)?;
        writeln!(out, "    short_hash: {}", short_hash(&group.sha))?;
        if let Some(prev) = &group.previous {
            writeln!(out, "    previous: {prev}")?;
        }
        if !meta.author.is_empty() {
            writeln!(out, "    author: {}", meta.author)?;
        }
        if !meta.date.is_empty() {
            writeln!(out, "    date: {}", meta.date)?;
        }
        if !meta.summary.is_empty() {
            writeln!(out, "    summary: {}", meta.summary)?;
        }
        writeln!(out, "    lines:")?;
        for line in group.lines {
            writeln!(out, "      {}: {}", line.final_line, line.content)?;
        }
        writeln!(out, "  </hunk>")?;
    }

    out.push_str("</git_blame>");
    Ok(out)
}

fn short_hash(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

#[cfg(test)]
#[path = "blame_tests.rs"]
mod tests;
