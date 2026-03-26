use std::fmt::{self, Write};

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use super::env_from_options;
use crate::{
    Result,
    util::{
        ToolResult, error,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// NUL-separated format for the metadata header.
/// Fields: full hash, short hash, author name, author date (ISO), body.
/// The body (%B) must be last since it can contain newlines.
const SHOW_FORMAT: &str = "%H%x00%h%x00%an%x00%aI%x00%B";

/// Sentinel separating the formatted header from `--numstat` output.
/// Must not contain NUL bytes (can't pass those in process args) and
/// must be unlikely to appear in commit messages.
const STAT_SEPARATOR: &str = "<<--JP-NUMSTAT-->>";

#[derive(Debug, PartialEq)]
struct ShowOutput {
    hash: String,
    short_hash: String,
    author: String,
    date: String,
    message: String,
    files: Vec<FileStat>,
}

#[derive(Debug, PartialEq)]
struct FileStat {
    path: String,
    insertions: String,
    deletions: String,
}

impl fmt::Display for FileStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.insertions == "-" && self.deletions == "-" {
            return write!(f, "- {} (binary)", self.path);
        }

        let mut stat = String::new();
        if self.insertions != "0" {
            write!(stat, "+{}", self.insertions)?;
        }
        if self.deletions != "0" {
            if !stat.is_empty() {
                stat.push(',');
            }
            write!(stat, "-{}", self.deletions)?;
        }
        if stat.is_empty() {
            write!(f, "- {}", self.path)
        } else {
            write!(f, "- {} ({stat})", self.path)
        }
    }
}

pub(crate) async fn git_show(
    root: Utf8PathBuf,
    revision: String,
    options: &Map<String, Value>,
) -> ToolResult {
    let env = env_from_options(options);
    git_show_impl(&root, &revision, &DuctProcessRunner, &env)
}

fn git_show_impl<R: ProcessRunner>(
    root: &Utf8Path,
    revision: &str,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    // Append a sentinel after the format so we can reliably split the formatted
    // header from the --numstat output that follows.
    let format_arg = format!("--format={SHOW_FORMAT}{STAT_SEPARATOR}");

    let output = runner.run_with_env(
        "git",
        &["show", &format_arg, "--numstat", revision],
        root,
        env,
    )?;

    if !output.status.is_success() {
        return error(format!("git show failed: {}", output.stderr.trim()));
    }

    let show = parse_show_output(&output.stdout)?;
    Ok(format_show_output(&show)?.into())
}

fn format_show_output(show: &ShowOutput) -> Result<String> {
    let mut out = String::from("<git_show>\n");
    writeln!(out, "  <hash>{}</hash>", show.hash)?;
    writeln!(out, "  <short_hash>{}</short_hash>", show.short_hash)?;
    writeln!(out, "  <author>{}</author>", show.author)?;
    writeln!(out, "  <date>{}</date>", show.date)?;

    out.push_str("  <message>\n");
    for line in show.message.lines() {
        writeln!(out, "    {line}")?;
    }
    out.push_str("  </message>\n");

    if !show.files.is_empty() {
        out.push_str("  <files>\n");
        for f in &show.files {
            writeln!(out, "    {f}")?;
        }
        out.push_str("  </files>\n");
    }

    out.push_str("</git_show>");

    Ok(out)
}

fn parse_show_output(stdout: &str) -> Result<ShowOutput> {
    let (header, stat_section) = stdout
        .split_once(STAT_SEPARATOR)
        .ok_or("unexpected git show output: missing stat separator")?;

    let parts: Vec<&str> = header.splitn(5, '\0').collect();
    if parts.len() < 5 {
        return Err(format!(
            "unexpected git show header format: expected 5 fields, got {}",
            parts.len()
        )
        .into());
    }

    let message = parts[4].trim().to_string();
    let files = parse_numstat_lines(stat_section);

    Ok(ShowOutput {
        hash: parts[0].to_string(),
        short_hash: parts[1].to_string(),
        author: parts[2].to_string(),
        date: parts[3].to_string(),
        message,
        files,
    })
}

/// Parse `--numstat` output lines into structured file stats.
///
/// Each line is tab-separated: `insertions\tdeletions\tpath`
/// Binary files show as: `-\t-\tpath`
fn parse_numstat_lines(stat_section: &str) -> Vec<FileStat> {
    stat_section
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }

            let mut parts = line.splitn(3, '\t');
            let insertions = parts.next()?.to_string();
            let deletions = parts.next()?.to_string();
            let path = parts.next()?.to_string();

            Some(FileStat {
                path,
                insertions,
                deletions,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "show_tests.rs"]
mod tests;
