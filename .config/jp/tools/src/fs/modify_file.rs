// TODO:
//
// Look into using (parts of) <https://github.com/jbr/semantic-edit-mcp> for
// semantic edits with (in-memory) staged changes.

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs::{self},
    ops::{Deref, DerefMut},
};

use camino::{Utf8Path, Utf8PathBuf};
use fancy_regex::RegexBuilder;
use jp_tool::{Capability, Outcome, Question};
use serde::Deserialize;
use serde_json::{Map, Value};
use similar::ChangeTag;

use super::utils::{authorize, is_file_dirty_impl, resolve_workspace_path};
use crate::{
    Context, Error,
    util::{
        OneOrMany, ToolResult,
        diff::{colored_diff, text_diff, unified_diff},
        error, fail,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

pub(crate) async fn fs_modify_file(
    ctx: Context,
    answers: &Map<String, Value>,
    options: &Map<String, Value>,
    path: Option<String>,
    patterns: Vec<Pattern>,
    replace_using_regex: bool,
    replace_all: bool,
    case_sensitive: bool,
) -> ToolResult {
    fs_modify_file_impl(
        &ctx,
        answers,
        options,
        path.as_deref(),
        &patterns,
        replace_using_regex,
        replace_all,
        case_sensitive,
        &DuctProcessRunner,
    )
}

#[expect(clippy::too_many_lines)]
fn fs_modify_file_impl<R: ProcessRunner>(
    ctx: &Context,
    answers: &Map<String, Value>,
    options: &Map<String, Value>,
    path: Option<&str>,
    patterns: &[Pattern],
    replace_using_regex: bool,
    replace_all: bool,
    case_sensitive: bool,
    runner: &R,
) -> ToolResult {
    if let Err(msg) = validate_patterns(patterns) {
        return error(msg);
    }

    if let Err(msg) = validate_paths(path, patterns) {
        return error(msg);
    }

    // Reject known overly-broad regex patterns.
    if let Some(blocked) = find_blocked_regex_patterns(patterns, replace_using_regex) {
        let list = blocked
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(", ");

        if let Some(result) = guard_broad_replacement(
            answers,
            "Replacement rejected: regex pattern is overly broad.",
            format!(
                "Regex pattern(s) {list} will match almost every line. This is likely a mistake. \
                 Continue anyway?"
            ),
        ) {
            return result;
        }
    }

    // Apply patterns, tracking per-file content mutations.
    // Keys are relative paths; values are (original, current) content.
    let mut files: BTreeMap<Utf8PathBuf, (String, String)> = BTreeMap::new();
    let mut outcomes = Vec::with_capacity(patterns.len());

    for pattern in patterns {
        let targets: Vec<&str> = match &pattern.paths {
            Some(paths) => paths.iter().map(String::as_str).collect(),
            None => vec![path.expect("validated above")],
        };

        let use_regex = pattern.regex.unwrap_or(replace_using_regex);
        let mut applied_any = false;
        let mut all_matched: Vec<String> = Vec::new();
        let mut invalid = None;

        for target in &targets {
            let resolved = match resolve_workspace_path(&ctx.root, target, ctx.access.as_ref()) {
                Ok(r) => r,
                Err(msg) => return error(msg),
            };
            if let Err(msg) = authorize(ctx.access.as_ref(), Capability::Update, &resolved.relative)
            {
                return error(msg);
            }

            // Load file on first access.
            if !files.contains_key(&resolved.relative) {
                if !resolved.absolute.exists() {
                    return error(format!("File does not exist: {target}"));
                }
                if !resolved.absolute.is_file() {
                    return error(format!("Path is not a regular file: {target}"));
                }
                let content = fs::read_to_string(&resolved.absolute)?;
                files.insert(resolved.relative.clone(), (content.clone(), content));
            }

            let (_, current) = files.get_mut(&resolved.relative).unwrap();
            let contents = Content(current.clone());
            let result = if use_regex {
                contents.replace_regexp(&pattern.old, &pattern.new, replace_all, case_sensitive)
            } else {
                contents
                    .replace_literal(&pattern.old, &pattern.new, replace_all, case_sensitive)
                    .map(|content| Replacement {
                        content,
                        matched: vec![],
                    })
            };

            match result {
                Ok(Replacement { content, matched }) => {
                    *current = content;
                    applied_any = true;
                    all_matched.extend(matched);
                }
                Err(ReplaceError::NotFound) => {}
                Err(ReplaceError::Invalid(msg)) => invalid = Some(msg),
            }
        }

        outcomes.push(if applied_any {
            PatternOutcome::Applied {
                matches: tally_matches(&all_matched),
            }
        } else if let Some(msg) = invalid {
            PatternOutcome::Invalid(msg)
        } else {
            PatternOutcome::NotFound
        });
    }

    let changes: Vec<Change> = files
        .into_iter()
        .filter(|(_, (original, current))| original != current)
        .map(|(path, (original, current))| Change {
            path,
            before: original,
            after: current,
        })
        .collect();

    let report = format_pattern_report(patterns, &outcomes);

    if changes.is_empty() {
        if report.is_empty() {
            return Err("None of the patterns matched the file's content.".into());
        }
        return Ok(report.into());
    }

    if ctx.action.is_format_arguments() {
        let diff = format_changes(changes);
        if report.is_empty() {
            return Ok(diff.into());
        }

        return Ok(format!("{report}\n\n{diff}").into());
    }

    // Guard: flag changes that affect a large fraction of the file.
    if let Some(broad_files) = find_broad_changes(&changes) {
        let files = broad_files.join(", ");
        if let Some(result) = guard_broad_replacement(
            answers,
            "Replacement rejected: too many lines changed.",
            format!(
                "The replacement modifies more than {BROAD_CHANGE_MAX_PERCENT}% of lines in: \
                 {files}. This may be unintentional. Continue anyway?",
            ),
        ) {
            return result;
        }
    }

    let auto_approve = parse_auto_approve_config(options);
    let result = apply_changes(changes, &ctx.root, answers, &auto_approve, runner)?;

    Ok(append_report(result, &report))
}

/// A search-and-replace pattern.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Pattern {
    /// The string to find.
    pub old: String,

    /// The replacement string.
    pub new: String,

    /// Optional per-pattern file paths.
    /// Overrides the root-level `path`.
    #[serde(default)]
    pub paths: Option<OneOrMany<String>>,

    /// Optional per-pattern regex mode.
    /// Overrides the call-level `replace_using_regex`.
    #[serde(default)]
    pub regex: Option<bool>,
}

/// Result of applying a single pattern.
#[derive(Debug, PartialEq)]
enum PatternOutcome {
    /// The pattern was found and replaced.
    Applied {
        /// Distinct text the pattern bound to, with occurrence counts, most
        /// frequent first.
        ///
        /// Only populated for regex patterns: a literal pattern matches itself,
        /// so there is nothing the caller doesn't already know.
        /// For a regex, this is the one thing the author can't see — what the
        /// engine *actually* captured, as opposed to what the pattern was meant
        /// to capture.
        /// A quantifier that ran past its intended boundary shows up here as an
        /// unexpected entry, before the diff has to be read.
        matches: Vec<MatchTally>,
    },

    /// The pattern was not found in the content.
    NotFound,

    /// The pattern is not a valid regex.
    Invalid(String),
}

/// One distinct matched string and how many times it was replaced.
#[derive(Debug, PartialEq)]
struct MatchTally {
    text: String,
    count: usize,
}

impl PatternOutcome {
    /// An applied outcome that captured nothing, as a literal pattern always
    /// does.
    #[cfg(test)]
    fn applied() -> Self {
        Self::Applied { matches: vec![] }
    }
}

/// A successful replacement: the new content, plus what the pattern matched.
#[derive(Debug)]
struct Replacement {
    content: String,

    /// Every matched string, in the order encountered.
    /// Empty for literal patterns.
    matched: Vec<String>,
}

/// Group `matched` into distinct strings with counts, most frequent first.
///
/// Ties break on the text so the report is deterministic.
fn tally_matches(matched: &[String]) -> Vec<MatchTally> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for text in matched {
        *counts.entry(text.as_str()).or_default() += 1;
    }

    let mut tallies: Vec<MatchTally> = counts
        .into_iter()
        .map(|(text, count)| MatchTally {
            text: text.to_owned(),
            count,
        })
        .collect();

    tallies.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.text.cmp(&b.text)));
    tallies
}

/// Why a replacement could not be performed.
#[derive(Debug, PartialEq)]
enum ReplaceError {
    /// The pattern did not match the content.
    NotFound,

    /// The pattern could not be compiled or executed as a regex.
    Invalid(String),
}

impl From<fancy_regex::Error> for ReplaceError {
    fn from(err: fancy_regex::Error) -> Self {
        Self::Invalid(err.to_string())
    }
}

/// Validates the patterns for common errors.
///
/// Returns an error message if invalid, or `None` if all patterns are valid.
fn validate_patterns(patterns: &[Pattern]) -> Result<(), String> {
    if patterns.is_empty() {
        return Err("No patterns provided.".to_owned());
    }

    let identical: Vec<_> = patterns
        .iter()
        .enumerate()
        .filter(|(_, p)| p.old == p.new)
        .map(|(i, _)| format!("#{}", i + 1))
        .collect();

    if !identical.is_empty() {
        return Err(format!(
            "Pattern(s) {} have identical old and new strings.",
            identical.join(", ")
        ));
    }

    Ok(())
}

/// Validates that every pattern has at least one target path.
///
/// Per-path safety (relative, no `..`, within workspace root) is enforced when
/// each target is resolved via `resolve_workspace_path` in the main loop.
/// This function handles only structural concerns.
fn validate_paths(default_path: Option<&str>, patterns: &[Pattern]) -> Result<(), String> {
    if default_path.is_none() {
        let missing: Vec<_> = patterns
            .iter()
            .enumerate()
            .filter(|(_, p)| p.paths.is_none())
            .map(|(i, _)| format!("#{}", i + 1))
            .collect();

        if !missing.is_empty() {
            return Err(format!(
                "Pattern(s) {} have no target files. Provide `path` at the top level or `paths` \
                 in each pattern.",
                missing.join(", ")
            ));
        }
    }

    for (i, pattern) in patterns.iter().enumerate() {
        if let Some(paths) = &pattern.paths
            && paths.is_empty()
        {
            return Err(format!("Pattern #{} has an empty `paths` array.", i + 1));
        }
    }

    Ok(())
}

/// Formats a report of pattern outcomes.
///
/// Returns empty string when a single literal pattern succeeded and there is
/// nothing to say.
/// Shows a summary when there are multiple patterns, details which patterns
/// were not found or invalid, and lists what each regex pattern matched.
fn format_pattern_report(patterns: &[Pattern], outcomes: &[PatternOutcome]) -> String {
    let total = outcomes.len();
    let applied = outcomes
        .iter()
        .filter(|o| matches!(o, PatternOutcome::Applied { .. }))
        .count();
    let not_found: Vec<_> = patterns
        .iter()
        .zip(outcomes.iter())
        .enumerate()
        .filter(|(_, (_, o))| matches!(o, PatternOutcome::NotFound))
        .collect();
    let invalid: Vec<_> = patterns
        .iter()
        .zip(outcomes.iter())
        .enumerate()
        .filter_map(|(i, (p, o))| match o {
            PatternOutcome::Invalid(msg) => Some((i, p, msg)),
            _ => None,
        })
        .collect();

    let matched = format_matched(patterns, outcomes);

    // Single pattern, succeeded, nothing captured worth reporting: no report.
    if applied == total && total <= 1 && matched.is_empty() {
        return String::new();
    }

    // All succeeded: summary, plus whatever the regexes captured.
    if applied == total {
        let mut report = if total <= 1 {
            String::new()
        } else {
            format!("{applied}/{total} patterns applied.")
        };
        if !matched.is_empty() {
            if !report.is_empty() {
                report.push_str("\n\n");
            }
            report.push_str(&matched);
        }
        return report;
    }

    // Some or all failed: detailed report.
    let mut report = format!("{applied}/{total} patterns applied.");

    if !matched.is_empty() {
        report.push_str("\n\n");
        report.push_str(&matched);
    }

    if !not_found.is_empty() {
        report.push_str("\n\nPatterns not found:");
        for (i, (pattern, _)) in &not_found {
            let preview = pattern_preview(&pattern.old);
            report.push_str(&format!("\n  #{}: `{preview}`", i + 1));
        }
    }

    if !invalid.is_empty() {
        report.push_str("\n\nInvalid regex patterns:");
        for (i, pattern, msg) in &invalid {
            let preview = pattern_preview(&pattern.old);
            report.push_str(&format!("\n  #{}: `{preview}`\n      {msg}", i + 1));
        }
    }

    report
}

/// Maximum distinct matched strings listed per pattern.
///
/// A pattern that binds to more shapes than this has already told the reader
/// what they need to know.
const MAX_REPORTED_MATCHES: usize = 8;

/// Lists what each regex pattern actually matched.
///
/// Empty when no pattern captured anything, which is every literal-only call.
/// This is the part a regex author can't otherwise see: the pattern says what
/// was intended and the diff says what resulted, but only this says what the
/// engine bound — so a quantifier that ate past its boundary is visible here
/// without reading a single diff hunk.
fn format_matched(patterns: &[Pattern], outcomes: &[PatternOutcome]) -> String {
    let mut sections = Vec::new();

    for (i, (pattern, outcome)) in patterns.iter().zip(outcomes.iter()).enumerate() {
        let PatternOutcome::Applied { matches } = outcome else {
            continue;
        };
        if matches.is_empty() {
            continue;
        }

        let preview = pattern_preview(&pattern.old);
        let mut section = format!("Pattern #{} `{preview}` matched:", i + 1);
        for tally in matches.iter().take(MAX_REPORTED_MATCHES) {
            let text = pattern_preview(&tally.text);
            let _ = write!(section, "\n  {text:?}");
            if tally.count > 1 {
                let _ = write!(section, " ×{}", tally.count);
            }
        }
        if matches.len() > MAX_REPORTED_MATCHES {
            let _ = write!(
                section,
                "\n  …and {} more distinct match(es)",
                matches.len() - MAX_REPORTED_MATCHES
            );
        }

        sections.push(section);
    }

    sections.join("\n\n")
}

/// Returns a short preview of a pattern string (first line, max 60 chars).
fn pattern_preview(s: &str) -> String {
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.chars().count() <= 60 {
        return first_line.to_owned();
    }

    let truncated: String = first_line.chars().take(57).collect();
    format!("{truncated}...")
}

/// Prepends a report to a successful outcome.
///
/// Non-success outcomes (e.g. `NeedsInput`) are passed through unchanged.
fn append_report(outcome: Outcome, report: &str) -> Outcome {
    if report.is_empty() {
        return outcome;
    }

    match outcome {
        Outcome::Success { content } => Outcome::Success {
            content: format!("{content}\n\n{report}"),
        },
        other => other,
    }
}

/// Regex patterns that are known to be overly broad.
///
/// These patterns match every line (or every character position) in a file,
/// which is almost never intended in a search-and-replace context.
const BLOCKED_REGEX_PATTERNS: &[&str] = &[".*", ".+", "^.*$", "^.+$", r"[\s\S]*", r"[\s\S]+"];

/// Minimum number of lines in the original file before the broad-change check
/// activates.
/// Small files are not worth flagging.
const BROAD_CHANGE_MIN_LINES: usize = 10;

/// Maximum percentage of changed (deleted) lines to total lines before asking
/// for confirmation. 50 means more than 50% of the original lines were removed
/// or replaced.
const BROAD_CHANGE_MAX_PERCENT: usize = 50;

/// Returns the subset of regex-mode patterns whose `old` field is a known
/// overly-broad regex.
///
/// `default_regex` is the call-level regex mode, used for patterns that do not
/// set their own `regex` flag.
fn find_blocked_regex_patterns(patterns: &[Pattern], default_regex: bool) -> Option<Vec<&str>> {
    let matches = patterns
        .iter()
        .filter(|p| p.regex.unwrap_or(default_regex))
        .map(|p| p.old.trim())
        .filter(|old| BLOCKED_REGEX_PATTERNS.contains(old))
        .collect::<Vec<_>>();

    if matches.is_empty() {
        return None;
    }

    Some(matches)
}

/// Returns `true` if the change modifies a suspiciously large fraction of the
/// file.
///
/// Only activates for files with at least [`BROAD_CHANGE_MIN_LINES`] lines.
/// The ratio is computed as deleted lines / total original lines.
fn is_broad_change(before: &str, after: &str) -> bool {
    let total_lines = before.lines().count();
    if total_lines < BROAD_CHANGE_MIN_LINES {
        return false;
    }

    let diff = text_diff(before, after);
    let changed = diff
        .iter_all_changes()
        .filter(|c| matches!(c.tag(), ChangeTag::Delete))
        .count();

    changed * 100 > total_lines * BROAD_CHANGE_MAX_PERCENT
}

/// Checks the user's answer to the `broad_replacement` question.
///
/// Returns `None` if the user approved (continue execution).
/// Returns `Some(ToolResult)` if the user rejected or hasn't answered yet.
fn guard_broad_replacement(
    answers: &Map<String, Value>,
    reject_message: &str,
    question_text: String,
) -> Option<ToolResult> {
    let (pre_amble, text) = question_text
        .split_once('\n')
        .map(|(pre, text)| (Some(pre.to_owned()), text.to_owned()))
        .unwrap_or((None, question_text));

    match answers.get("broad_replacement").and_then(Value::as_bool) {
        Some(true) => None,
        Some(false) => Some(fail(reject_message)),
        None => {
            let mut q =
                Question::boolean("broad_replacement", text).with_default(Value::Bool(false));
            if let Some(p) = pre_amble {
                q = q.with_preamble(p);
            }
            Some(Ok(Outcome::NeedsInput { question: q }))
        }
    }
}

/// Returns the paths of changes that affect a suspiciously large fraction of
/// the file.
fn find_broad_changes(changes: &[Change]) -> Option<Vec<&str>> {
    let matches = changes
        .iter()
        .filter(|c| is_broad_change(&c.before, &c.after))
        .map(|c| c.path.as_str())
        .collect::<Vec<_>>();

    if matches.is_empty() {
        return None;
    }

    Some(matches)
}

pub struct Change {
    pub path: Utf8PathBuf,
    pub before: String,
    pub after: String,
}

pub struct Content(String);

impl Deref for Content {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Content {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Content {
    fn find_pattern_range(&self, pattern: &str) -> Option<(usize, usize)> {
        self.find_exact_substring(pattern)
            .or_else(|| self.find_trimmed_substring(pattern))
            .or_else(|| {
                // Only use fuzzy matching for single-line patterns.
                // Multi-line fuzzy matching is unreliable because the pattern
                // length may not match the actual matched text length due to
                // different line wrapping.
                if pattern.lines().count() <= 1 {
                    self.find_fuzzy_substring(pattern)
                } else {
                    None
                }
            })
    }

    fn find_exact_substring(&self, pattern: &str) -> Option<(usize, usize)> {
        let start = self.0.find(pattern)?;
        Some((start, start + pattern.len()))
    }

    fn find_trimmed_substring(&self, pattern: &str) -> Option<(usize, usize)> {
        let trimmed_pattern = pattern.trim();
        let start = self.0.find(trimmed_pattern)?;
        Some((start, start + trimmed_pattern.len()))
    }

    fn find_fuzzy_substring(&self, pattern: &str) -> Option<(usize, usize)> {
        let first_line_to_find = pattern
            .lines()
            .next()?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        // Find lines that fuzzy match
        let mut byte_offset = 0;
        for line in self.0.lines() {
            let fuzzy_line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            if fuzzy_line.contains(&first_line_to_find) {
                return Some((byte_offset, byte_offset + pattern.len()));
            }
            byte_offset += line.len() + 1; // +1 for newline
        }

        None
    }

    /// Replace occurrences of a literal string.
    ///
    /// Uses [`Content::find_pattern_range`] to locate the first occurrence
    /// (trying exact, trimmed, and fuzzy matching).
    /// When `replace_all` is true, all subsequent exact matches of the resolved
    /// text are also replaced.
    fn replace_literal(
        &self,
        find: &str,
        replace: &str,
        replace_all: bool,
        case_sensitive: bool,
    ) -> Result<String, ReplaceError> {
        if case_sensitive {
            self.replace_literal_sensitive(find, replace, replace_all)
        } else {
            self.replace_literal_insensitive(find, replace, replace_all)
        }
    }

    fn replace_literal_sensitive(
        &self,
        find: &str,
        replace: &str,
        replace_all: bool,
    ) -> Result<String, ReplaceError> {
        // Find the first occurrence to determine the effective match.
        let (first_start, first_end) = self
            .find_pattern_range(find)
            .ok_or(ReplaceError::NotFound)?;

        if !replace_all {
            let mut result = String::with_capacity(self.0.len());
            result.push_str(&self.0[..first_start]);
            result.push_str(replace);
            result.push_str(&self.0[first_end..]);
            return Ok(result);
        }

        // Derive the actual matched text (may differ from `find` due to
        // trimmed/fuzzy matching) so we can find all subsequent occurrences
        // using exact substring search.
        let matched = &self.0[first_start..first_end];

        let mut result = String::with_capacity(self.0.len());
        let mut remaining = &self.0[..];

        while let Some(pos) = remaining.find(matched) {
            result.push_str(&remaining[..pos]);
            result.push_str(replace);
            remaining = &remaining[pos + matched.len()..];
        }
        result.push_str(remaining);

        Ok(result)
    }

    fn replace_literal_insensitive(
        &self,
        find: &str,
        replace: &str,
        replace_all: bool,
    ) -> Result<String, ReplaceError> {
        // Case-insensitive literal search: use regex with escaped pattern.
        let escaped = fancy_regex::escape(find);
        let re = RegexBuilder::new(&escaped)
            .case_insensitive(true)
            .multi_line(true)
            .unicode_mode(true)
            .build()?;

        if !re.is_match(&self.0)? {
            return Err(ReplaceError::NotFound);
        }

        let replaced = if replace_all {
            re.replace_all(&self.0, replace)
        } else {
            re.replace(&self.0, replace)
        };

        Ok(replaced.to_string())
    }

    /// Replace occurrences of a regex pattern.
    ///
    /// The returned [`Replacement`] carries the text each match bound to, so a
    /// caller can report what the pattern captured rather than only what it
    /// produced.
    fn replace_regexp(
        &self,
        find: &str,
        replace: &str,
        replace_all: bool,
        case_sensitive: bool,
    ) -> Result<Replacement, ReplaceError> {
        let re = RegexBuilder::new(find)
            .case_insensitive(!case_sensitive)
            .multi_line(true)
            .dot_matches_new_line(false)
            .unicode_mode(true)
            .build()?;

        if !re.is_match(&self.0)? {
            return Err(ReplaceError::NotFound);
        }

        // Collected before replacing, and bounded the same way the replacement
        // is: with `replace_all` off only the first match is rewritten, so only
        // the first is reported.
        let mut matched = Vec::new();
        for found in re.find_iter(&self.0) {
            matched.push(found?.as_str().to_owned());
            if !replace_all {
                break;
            }
        }

        let content = if replace_all {
            re.replace_all(&self.0, replace)
        } else {
            re.replace(&self.0, replace)
        };

        Ok(Replacement {
            content: content.to_string(),
            matched,
        })
    }
}

fn format_changes(changes: Vec<Change>) -> String {
    let diff = changes
        .into_iter()
        .map(|change| {
            let path = change.path.to_string();
            let diff = text_diff(&change.before, &change.after);
            let unified = unified_diff(&diff, &path);

            colored_diff(&diff, &unified, &path)
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    if diff.is_empty() {
        return "<before and after are identical>".to_owned();
    }

    diff
}

/// Parsed auto-approve configuration from tool options.
struct AutoApproveConfig {
    enabled: bool,
    max_changed_files: usize,
    max_changed_lines: usize,
    max_ratio_percent: usize,
}

impl AutoApproveConfig {
    const DEFAULT_MAX_CHANGED_FILES: usize = 2;
    const DEFAULT_MAX_CHANGED_LINES: usize = 10;
    const DEFAULT_MAX_RATIO_PERCENT: usize = 20;
}

fn parse_auto_approve_config(options: &Map<String, Value>) -> AutoApproveConfig {
    let trigger = options
        .get("apply_changes_trigger")
        .and_then(Value::as_str)
        .unwrap_or("always");

    let enabled = trigger == "heuristics";

    let max_changed_files = options
        .get("auto_approve_max_changed_files")
        .and_then(Value::as_u64)
        .map_or(AutoApproveConfig::DEFAULT_MAX_CHANGED_FILES, |v| {
            usize::try_from(v).unwrap_or(AutoApproveConfig::DEFAULT_MAX_CHANGED_FILES)
        });

    let max_changed_lines = options
        .get("auto_approve_max_changed_lines")
        .and_then(Value::as_u64)
        .map_or(AutoApproveConfig::DEFAULT_MAX_CHANGED_LINES, |v| {
            usize::try_from(v).unwrap_or(AutoApproveConfig::DEFAULT_MAX_CHANGED_LINES)
        });

    let max_ratio_percent = options
        .get("auto_approve_max_ratio_percent")
        .and_then(Value::as_u64)
        .map_or(AutoApproveConfig::DEFAULT_MAX_RATIO_PERCENT, |v| {
            usize::try_from(v).unwrap_or(AutoApproveConfig::DEFAULT_MAX_RATIO_PERCENT)
        });

    AutoApproveConfig {
        enabled,
        max_changed_files,
        max_changed_lines,
        max_ratio_percent,
    }
}

/// Returns `true` if the changes are small enough to skip the `apply_changes`
/// inquiry.
///
/// Criteria (all must hold):
///
/// - Tlta. changed files \<= threshold
/// - Total changed lines (insertions + deletions) \<= threshold
/// - Deletion ratio per file \< threshold percent
fn should_auto_approve(changes: &[(String, String, String)], config: &AutoApproveConfig) -> bool {
    if !config.enabled || changes.len() > config.max_changed_files {
        return false;
    }

    let mut total_changed = 0;
    for (_, before, after) in changes {
        let diff = text_diff(before, after);
        let (insertions, deletions) =
            diff.iter_all_changes()
                .fold((0usize, 0usize), |(ins, del), c| match c.tag() {
                    ChangeTag::Insert => (ins + 1, del),
                    ChangeTag::Delete => (ins, del + 1),
                    ChangeTag::Equal => (ins, del),
                });

        total_changed += insertions + deletions;

        let total_lines = before.lines().count();
        if total_lines >= BROAD_CHANGE_MIN_LINES
            && deletions * 100 > total_lines * config.max_ratio_percent
        {
            return false;
        }
    }

    total_changed <= config.max_changed_lines
}

fn apply_changes<R: ProcessRunner>(
    changes: Vec<Change>,
    root: &Utf8Path,
    answers: &Map<String, Value>,
    auto_approve: &AutoApproveConfig,
    runner: &R,
) -> Result<Outcome, Error> {
    let mut queue = vec![];
    let count = changes.len();
    for Change {
        path,
        after,
        before,
    } in changes
    {
        if is_file_dirty_impl(root, &path, runner)? {
            match answers.get("modify_dirty_file").and_then(Value::as_bool) {
                Some(true) => {}
                Some(false) => {
                    return Err("File has uncommitted changes. Change discarded.".into());
                }
                None => {
                    return Ok(Outcome::NeedsInput {
                        question: Question::boolean(
                            "modify_dirty_file",
                            format!("File '{path}' has uncommitted changes. Modify anyway?"),
                        ),
                    });
                }
            }
        }

        let file_path = path.to_string();
        let file_path = file_path.trim_start_matches('/');

        queue.push((file_path.to_owned(), before, after));
    }

    let patch = queue
        .iter()
        .map(|(path, before, after)| {
            let diff = text_diff(before, after);
            let diff = unified_diff(&diff, path);
            format!("```diff\n{diff}```")
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    if should_auto_approve(&queue, auto_approve) {
        for (path, _, after) in &queue {
            fs::write(root.join(path), after)?;
        }

        return Ok(format!(
            "{} modified successfully:\n\n{}",
            if count == 1 { "File" } else { "Files" },
            patch
        )
        .into());
    }

    match answers.get("apply_changes").and_then(Value::as_bool) {
        Some(true) => {}
        Some(false) => {
            return Err(
                "The proposed file changes were reviewed and rejected. Changes were not applied. \
                 You may retry with different changes."
                    .into(),
            );
        }
        None => {
            return Ok(Outcome::NeedsInput {
                question: Question::boolean(
                    "apply_changes",
                    "Do you want to apply the patch shown above?",
                )
                .with_preamble(patch)
                .with_default(Value::Bool(true)),
            });
        }
    }

    for (path, _, after) in queue {
        fs::write(root.join(path), after)?;
    }

    Ok(format!(
        "{} modified successfully:\n\n{}",
        if count == 1 { "File" } else { "Files" },
        patch
    )
    .into())
}

#[cfg(test)]
#[path = "modify_file_tests.rs"]
mod tests;
