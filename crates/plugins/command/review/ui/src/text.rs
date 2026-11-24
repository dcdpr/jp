//! The words the UI shows for what the API answers.

use chrono::{DateTime, Utc};
use jp_review_api::{Counts, Ended, Pass, Running, Version};

/// What a line of a diff is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// A hunk header: `@@ -40,3 +40,3 @@`.
    Hunk,
    Added,
    Removed,
    Context,

    /// `\ No newline at end of file`.
    Note,
}

impl Line {
    /// The class a diff line of this kind is styled with.
    #[must_use]
    pub fn class(self) -> &'static str {
        match self {
            Self::Hunk => "jp-diff-hunk",
            Self::Added => "jp-diff-added",
            Self::Removed => "jp-diff-removed",
            Self::Context => "jp-diff-context",
            Self::Note => "jp-diff-note",
        }
    }
}

/// Each line of the unified diff hunks `diff`, and what it is.
#[must_use]
pub fn diff_lines(diff: &str) -> Vec<(Line, &str)> {
    diff.lines()
        .map(|line| {
            let kind = match line.as_bytes().first() {
                Some(b'@') if line.starts_with("@@") => Line::Hunk,
                Some(b'+') => Line::Added,
                Some(b'-') => Line::Removed,
                Some(b'\\') => Line::Note,
                _ => Line::Context,
            };
            (kind, line)
        })
        .collect()
}

/// A suggestion as diff lines: what its range holds, then what it would hold.
#[must_use]
pub fn suggestion(excerpt: &str, replacement: &str) -> Vec<(Line, String)> {
    let removed = excerpt
        .lines()
        .map(|line| (Line::Removed, format!("-{line}")));
    let added = replacement
        .lines()
        .map(|line| (Line::Added, format!("+{line}")));

    removed.chain(added).collect()
}

/// How many comments stand where: `1 draft, 2 open`; `no comments` when there
/// are none.
#[must_use]
pub fn counts(counts: &Counts) -> String {
    let parts: Vec<String> = [
        (counts.draft, "draft"),
        (counts.open, "open"),
        (counts.resolved, "resolved"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, status)| format!("{count} {status}"))
    .collect();

    if parts.is_empty() {
        return "no comments".to_owned();
    }

    parts.join(", ")
}

/// A version as a version picker lists it: `3`, or `1 (base)`.
#[must_use]
pub fn version(version: &Version) -> String {
    match &version.label {
        Some(label) => format!("{} ({label})", version.number),
        None => version.number.to_string(),
    }
}

/// The pass running on a review, as its header says it.
#[must_use]
pub fn running(running: &Running) -> String {
    if running.stoppable {
        return format!("Pass {} running", running.number);
    }

    format!("Pass {} running in another process", running.number)
}

/// How a pass ended: `Pass 2 completed`; `Pass 2 running` for one that hasn't.
#[must_use]
pub fn pass(pass: &Pass) -> String {
    match &pass.ended {
        Some(Ended { outcome, .. }) => format!("Pass {} {outcome}", pass.number),
        None => format!("Pass {} running", pass.number),
    }
}

/// When something happened, to the minute: `2026-11-17 10:04 UTC`.
#[must_use]
pub fn time(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// The note that lines changed without rendered text: `2 changed lines not
/// shown`.
#[must_use]
pub fn hidden(count: usize) -> String {
    if count == 1 {
        return "1 changed line not shown".to_owned();
    }

    format!("{count} changed lines not shown")
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
