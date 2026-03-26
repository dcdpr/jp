// TODO:
//
// Look into using (parts of) <https://github.com/jbr/semantic-edit-mcp> for
// semantic edits with (in-memory) staged changes.

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs::{self},
    ops::{Deref, DerefMut},
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};
use crossterm::style::{ContentStyle, Stylize as _};
use fancy_regex::RegexBuilder;
use jp_tool::{AnswerType, Outcome, Question};
use serde::Deserialize;
use serde_json::{Map, Value};
use similar::{ChangeTag, TextDiff, udiff::UnifiedDiff};

use super::utils::is_file_dirty_impl;
use crate::{
    Context, Error,
    util::{
        OneOrMany, ToolResult, error, fail,
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
    if replace_using_regex && let Some(blocked) = find_blocked_regex_patterns(patterns) {
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

        let mut applied_any = false;

        for target in &targets {
            let clean = target.trim_start_matches('/');
            let relative = Utf8PathBuf::from(clean);
            let absolute = ctx.root.join(clean);

            // Load file on first access.
            if !files.contains_key(&relative) {
                if !absolute.exists() {
                    return error(format!("File does not exist: {clean}"));
                }
                if !absolute.is_file() {
                    return error(format!("Path is not a regular file: {clean}"));
                }
                let Ok(stripped) = absolute.strip_prefix(&ctx.root) else {
                    return fail("Path is not within workspace root.");
                };
                let content = fs::read_to_string(&absolute)?;
                files.insert(stripped.to_owned(), (content.clone(), content));
            }

            let (_, current) = files.get_mut(&relative).unwrap();
            let contents = Content(current.clone());
            let result = if replace_using_regex {
                contents.replace_regexp(&pattern.old, &pattern.new, replace_all, case_sensitive)
            } else {
                contents.replace_literal(&pattern.old, &pattern.new, replace_all, case_sensitive)
            };

            if let Ok(after) = result {
                *current = after;
                applied_any = true;
            }
        }

        outcomes.push(if applied_any {
            PatternOutcome::Applied
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

    /// Optional per-pattern file paths. Overrides the root-level `path`.
    #[serde(default)]
    pub paths: Option<OneOrMany<String>>,
}

/// Result of applying a single pattern.
#[derive(Debug, PartialEq)]
enum PatternOutcome {
    /// The pattern was found and replaced.
    Applied,

    /// The pattern was not found in the content.
    NotFound,
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

/// Validates that every pattern has at least one target path, and all paths are
/// relative.
fn validate_paths(default_path: Option<&str>, patterns: &[Pattern]) -> Result<(), String> {
    // Check the default path if provided.
    if let Some(p) = default_path {
        validate_single_path(p)?;
    }

    // Every pattern must have a target: either from the default or its own paths.
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

    // Validate per-pattern paths.
    for (i, pattern) in patterns.iter().enumerate() {
        if let Some(paths) = &pattern.paths {
            if paths.is_empty() {
                return Err(format!("Pattern #{} has an empty `paths` array.", i + 1));
            }
            for p in paths.iter() {
                validate_single_path(p)?;
            }
        }
    }

    Ok(())
}

/// Validates a single file path.
fn validate_single_path(path: &str) -> Result<(), String> {
    let p = Utf8PathBuf::from(path);
    if p.is_absolute() {
        return Err(format!("Path must be relative: {path}"));
    }

    Ok(())
}

/// Formats a report of pattern outcomes.
///
/// Returns empty string when there is a single pattern that succeeded. Shows a
/// summary when there are multiple patterns, and details which patterns were
/// not found.
fn format_pattern_report(patterns: &[Pattern], outcomes: &[PatternOutcome]) -> String {
    let total = outcomes.len();
    let applied = outcomes
        .iter()
        .filter(|o| matches!(o, PatternOutcome::Applied))
        .count();
    let failed: Vec<_> = patterns
        .iter()
        .zip(outcomes.iter())
        .enumerate()
        .filter(|(_, (_, o))| matches!(o, PatternOutcome::NotFound))
        .collect();

    // Single pattern, succeeded: no report.
    if failed.is_empty() && total <= 1 {
        return String::new();
    }

    // All succeeded, multiple patterns: brief summary.
    if failed.is_empty() {
        return format!("{applied}/{total} patterns applied.");
    }

    // Some or all failed: detailed report.
    let mut report = format!("{applied}/{total} patterns applied.\n\nPatterns not found:");
    for (i, (pattern, _)) in &failed {
        let preview = pattern_preview(&pattern.old);
        report.push_str(&format!("\n  #{}: `{preview}`", i + 1));
    }

    report
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
/// activates. Small files are not worth flagging.
const BROAD_CHANGE_MIN_LINES: usize = 10;

/// Maximum percentage of changed (deleted) lines to total lines before asking
/// for confirmation. 50 means more than 50% of the original lines were removed
/// or replaced.
const BROAD_CHANGE_MAX_PERCENT: usize = 50;

/// Returns the subset of patterns whose `old` field is a known overly-broad
/// regex.
fn find_blocked_regex_patterns(patterns: &[Pattern]) -> Option<Vec<&str>> {
    let matches = patterns
        .iter()
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
/// Only activates for files with at least [`BROAD_CHANGE_MIN_LINES`] lines. The
/// ratio is computed as deleted lines / total original lines.
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
/// Returns `None` if the user approved (continue execution). Returns
/// `Some(ToolResult)` if the user rejected or hasn't answered yet.
fn guard_broad_replacement(
    answers: &Map<String, Value>,
    reject_message: &str,
    question_text: String,
) -> Option<ToolResult> {
    match answers.get("broad_replacement").and_then(Value::as_bool) {
        Some(true) => None,
        Some(false) => Some(fail(reject_message)),
        None => Some(Ok(Outcome::NeedsInput {
            question: Question {
                id: "broad_replacement".to_string(),
                text: question_text,
                answer_type: AnswerType::Boolean,
                default: Some(Value::Bool(false)),
            },
        })),
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
    /// (trying exact, trimmed, and fuzzy matching). When `replace_all` is true,
    /// all subsequent exact matches of the resolved text are also replaced.
    fn replace_literal(
        &self,
        find: &str,
        replace: &str,
        replace_all: bool,
        case_sensitive: bool,
    ) -> std::result::Result<String, Error> {
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
    ) -> std::result::Result<String, Error> {
        // Find the first occurrence to determine the effective match.
        let (first_start, first_end) = self
            .find_pattern_range(find)
            .ok_or("Cannot find pattern to replace")?;

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
    ) -> std::result::Result<String, Error> {
        // Case-insensitive literal search: use regex with escaped pattern.
        let escaped = fancy_regex::escape(find);
        let re = RegexBuilder::new(&escaped)
            .case_insensitive(true)
            .multi_line(true)
            .unicode_mode(true)
            .build()?;

        if !re.is_match(&self.0)? {
            return Err("Cannot find pattern to replace".into());
        }

        let replaced = if replace_all {
            re.replace_all(&self.0, replace)
        } else {
            re.replace(&self.0, replace)
        };

        Ok(replaced.to_string())
    }

    /// Replace occurrences of a regex pattern.
    fn replace_regexp(
        &self,
        find: &str,
        replace: &str,
        replace_all: bool,
        case_sensitive: bool,
    ) -> std::result::Result<String, Error> {
        let re = RegexBuilder::new(find)
            .case_insensitive(!case_sensitive)
            .multi_line(true)
            .dot_matches_new_line(false)
            .unicode_mode(true)
            .build()?;

        let result = if replace_all {
            re.replace_all(&self.0, replace)
        } else {
            re.replace(&self.0, replace)
        };

        Ok(result.to_string())
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
/// - Tlta. changed files <= threshold
/// - Total changed lines (insertions + deletions) <= threshold
/// - Deletion ratio per file < threshold percent
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
                        question: Question {
                            id: "modify_dirty_file".to_string(),
                            text: format!("File '{path}' has uncommitted changes. Modify anyway?"),
                            answer_type: AnswerType::Boolean,
                            default: None,
                        },
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
                "`apply_changes` inquiry was answered with `false`. Changes discarded.".into(),
            );
        }
        None => {
            return Ok(Outcome::NeedsInput {
                question: Question {
                    id: "apply_changes".to_string(),
                    text: format!("Do you want to apply the following patch?\n\n{patch}"),
                    answer_type: AnswerType::Boolean,
                    default: Some(Value::Bool(true)),
                },
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

/// Formats a line number as a right-aligned string of the given width,
/// or blank spaces if the index is `None`.
fn fmt_line_num(index: Option<usize>, width: usize) -> String {
    match index {
        Some(idx) => format!("{:>width$}", idx + 1),
        None => " ".repeat(width),
    }
}

fn text_diff<'old, 'new, 'bufs>(
    old: &'old str,
    new: &'new str,
) -> TextDiff<'old, 'new, 'bufs, str> {
    similar::TextDiff::configure()
        .algorithm(similar::Algorithm::Patience)
        .timeout(Duration::from_secs(2))
        .diff_lines(old, new)
}

fn unified_diff<'diff, 'old, 'new, 'bufs>(
    diff: &'diff TextDiff<'old, 'new, 'bufs, str>,
    file: &str,
) -> UnifiedDiff<'diff, 'old, 'new, 'bufs, str> {
    let mut unified = diff.unified_diff();
    unified.context_radius(3).header(file, file);
    unified
}

fn colored_diff<'old, 'new, 'diff: 'old + 'new, 'bufs>(
    diff: &'diff TextDiff<'old, 'new, 'bufs, str>,
    unified: &UnifiedDiff<'diff, 'old, 'new, 'bufs, str>,
    path: &str,
) -> String {
    let mut buf = String::new();

    let (additions, deletions) =
        diff.iter_all_changes()
            .fold((0, 0), |(mut add, mut del), change| {
                if matches!(change.tag(), ChangeTag::Delete) {
                    del += 1;
                } else if matches!(change.tag(), ChangeTag::Insert) {
                    add += 1;
                }
                (add, del)
            });

    // Dynamic number column width based on the largest line number.
    let max_line = diff.old_slices().len().max(diff.new_slices().len()).max(1);
    let nw = max_line.to_string().len();

    // Build stats: deletions first (left column = red), additions second (right = green).
    let mut stats_plain = String::new();
    let mut stats_colored = String::new();
    if deletions > 0 {
        stats_plain.push_str(&format!("-{deletions}"));
        stats_colored.push_str(format!("-{deletions}").red().to_string().as_str());
    }
    if additions > 0 {
        if !stats_plain.is_empty() {
            stats_plain.push(',');
            stats_colored.push(',');
        }
        stats_plain.push_str(&format!("+{additions}"));
        stats_colored.push_str(format!("+{additions}").green().to_string().as_str());
    }
    let stats_width = stats_plain.len();

    // Unified column where │ sits. Enough room for two right-aligned number
    // columns plus a separator space, or the stats text plus a leading space.
    let line_nums_width = 2 * nw + 1;
    let pipe_col = (line_nums_width + 1).max(stats_width + 1);

    // Header: stats line + separator.
    let stats_pad = " ".repeat(pipe_col - stats_width - 1);
    let header_line = format!("{stats_pad}{stats_colored} │ {}\n", path.bold());
    let separator = format!("{}┼{}\n", "─".repeat(pipe_col), "─".repeat(path.len() + 2));
    buf.push_str(&header_line);
    buf.push_str(&separator);

    // Hunks, with an ellipsis separator between non-contiguous regions.
    let num_pad = " ".repeat(pipe_col - line_nums_width);
    let mut first_hunk = true;
    for hunk in unified.iter_hunks() {
        if !first_hunk {
            let _ = writeln!(&mut buf, "{}│ …", " ".repeat(pipe_col));
        }
        first_hunk = false;

        for op in hunk.ops() {
            for change in diff.iter_inline_changes(op) {
                let (sign, s) = match change.tag() {
                    ChangeTag::Delete => ("-", ContentStyle::new().red()),
                    ChangeTag::Insert => ("+", ContentStyle::new().green()),
                    ChangeTag::Equal => (" ", ContentStyle::new().dim()),
                };

                let old = fmt_line_num(change.old_index(), nw);
                let new = fmt_line_num(change.new_index(), nw);

                let _ = write!(
                    &mut buf,
                    "{} {}{}│{}",
                    s.apply(old),
                    s.apply(new),
                    num_pad,
                    s.apply(sign).bold(),
                );
                for (emphasized, value) in change.iter_strings_lossy() {
                    if emphasized {
                        let _ = write!(&mut buf, "{}", s.apply(value).underlined().on_black());
                    } else {
                        let _ = write!(&mut buf, "{}", s.apply(value));
                    }
                }
                if change.missing_newline() {
                    buf.push('\n');
                }
            }
        }
    }

    // Footer: separator + stats (mirrored header for long diffs).
    buf.push_str(&separator);
    buf.push_str(&header_line);

    buf
}

#[cfg(test)]
#[path = "modify_file_tests.rs"]
mod tests;
