use std::fmt::Write;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use super::env_from_options;
use crate::{
    Result,
    util::{
        OneOrMany, ToolResult, error,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// Upper bound on how many commits we'll return, regardless of what the caller
/// asks for. Keeps context size bounded.
const MAX_COUNT: usize = 50;
const DEFAULT_COUNT: usize = 20;

/// NUL-separated format string for `git log --format`.
/// Fields: full hash, short hash, author name, author date (ISO), subject.
const LOG_FORMAT: &str = "%H%x00%h%x00%an%x00%aI%x00%s";

#[derive(Debug)]
struct LogEntry {
    hash: String,
    short_hash: String,
    author: String,
    date: String,
    subject: String,
}

pub(crate) async fn git_log(
    root: Utf8PathBuf,
    query: Option<String>,
    paths: Option<OneOrMany<String>>,
    count: Option<usize>,
    since: Option<String>,
    options: &Map<String, Value>,
) -> ToolResult {
    let env = env_from_options(options);
    let count = count.unwrap_or(DEFAULT_COUNT).min(MAX_COUNT);
    let paths = paths.unwrap_or_default();
    let paths = paths.iter().map(AsRef::as_ref).collect::<Vec<_>>();

    git_log_impl(
        &root,
        query.as_deref(),
        &paths,
        count,
        since.as_deref(),
        &DuctProcessRunner,
        &env,
    )
}

fn git_log_impl<R: ProcessRunner>(
    root: &Utf8Path,
    query: Option<&str>,
    paths: &[&str],
    count: usize,
    since: Option<&str>,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    let format_arg = format!("--format={LOG_FORMAT}");
    let count_str = count.to_string();
    let grep_arg = query.map(|q| format!("--grep={q}"));
    let since_arg = since.map(|s| format!("--since={s}"));

    let mut args: Vec<&str> = vec!["log", &format_arg, "-n", &count_str];

    if let Some(ref g) = grep_arg {
        args.push("--fixed-strings");
        args.push(g);
    }

    if let Some(ref s) = since_arg {
        args.push(s);
    }

    if !paths.is_empty() {
        args.push("--");
        args.extend(paths);
    }

    let output = runner.run_with_env("git", &args, root, env)?;

    if !output.status.is_success() {
        return error(format!("git log failed: {}", output.stderr.trim()));
    }

    let entries = parse_log_entries(&output.stdout);

    if entries.is_empty() {
        return Ok("No commits found matching the query.".into());
    }

    Ok(format_log_entries(&entries)?.into())
}

fn format_log_entries(entries: &[LogEntry]) -> Result<String> {
    let mut out = String::from("<git_log>\n");
    for entry in entries {
        writeln!(out, "  <commit>")?;
        writeln!(out, "    hash: {}", entry.hash)?;
        writeln!(out, "    short_hash: {}", entry.short_hash)?;
        writeln!(out, "    author: {}", entry.author)?;
        writeln!(out, "    date: {}", entry.date)?;
        writeln!(out, "    subject: {}", entry.subject)?;
        writeln!(out, "  </commit>")?;
    }
    out.push_str("</git_log>");

    Ok(out)
}

fn parse_log_entries(stdout: &str) -> Vec<LogEntry> {
    stdout
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(5, '\0').collect();
            if parts.len() < 5 {
                return None;
            }

            Some(LogEntry {
                hash: parts[0].to_string(),
                short_hash: parts[1].to_string(),
                author: parts[2].to_string(),
                date: parts[3].to_string(),
                subject: parts[4].to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod tests;
