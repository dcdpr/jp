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
/// asks for.
/// Keeps context size bounded.
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
    content: Option<String>,
    content_regex: Option<bool>,
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
        content.as_deref(),
        content_regex.unwrap_or(false),
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
    content: Option<&str>,
    content_regex: bool,
    paths: &[&str],
    count: usize,
    since: Option<&str>,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    if content_regex && content.is_none() {
        return error("`content_regex` requires the `content` parameter to be set.");
    }

    let format_arg = format!("--format={LOG_FORMAT}");
    let count_str = count.to_string();
    let grep_arg = query.map(|q| format!("--grep={q}"));
    let pickaxe_arg = content.map(|c| {
        if content_regex {
            // Regex mode: match commits where an added or removed diff line
            // matches the pattern.
            format!("-G{c}")
        } else {
            // Literal mode: match commits where the number of occurrences of
            // the string changes.
            format!("-S{c}")
        }
    });
    let since_arg = since.map(|s| format!("--since={s}"));

    let mut args: Vec<&str> = vec!["log", &format_arg, "-n", &count_str];

    if let Some(ref g) = grep_arg {
        args.push("--fixed-strings");
        args.push(g);
    }

    if let Some(ref p) = pickaxe_arg {
        args.push(p);
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
        return Ok(empty_result_message(query, content).into());
    }

    Ok(format_log_entries(&entries)?.into())
}

/// Message returned when no commits match.
/// When the caller used `query` without `content`, remind them that `query`
/// only searches commit messages, since callers commonly expect it to search
/// diff contents.
fn empty_result_message(query: Option<&str>, content: Option<&str>) -> String {
    let mut msg = String::from("No commits found matching the given filters.");
    if query.is_some() && content.is_none() {
        msg.push_str(
            " Note: `query` matches commit *messages* only. To find commits whose *diff* adds or \
             removes a string, use the `content` parameter instead.",
        );
    }
    msg
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
