//! The words the UI shows for what the API answers.

use chrono::{DateTime, Utc};
use jp_review_api::{Counts, Ended, Pass, Running, Version};

use crate::route::Showing;

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
    /// The classes a diff line of this kind is styled with.
    #[must_use]
    pub fn class(self) -> &'static str {
        match self {
            Self::Hunk | Self::Note => "px-3 text-muted",
            Self::Added => "bg-added/15 px-3 text-added",
            Self::Removed => "px-3 text-removed",
            Self::Context => "px-3",
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

/// A version as a picker lists it: `Version 2`, `Version 1 (base)`, or `Version
/// 3 (latest)` for the `latest` one.
#[must_use]
pub fn version(version: &Version, latest: bool) -> String {
    let notes: Vec<&str> = [version.label.as_deref(), latest.then_some("latest")]
        .into_iter()
        .flatten()
        .collect();
    if notes.is_empty() {
        return format!("Version {}", version.number);
    }

    format!("Version {} ({})", version.number, notes.join(", "))
}

/// What the page shows of a file, as the header says it, in full and for a
/// narrow screen: `Version 3` and `v3`, where `latest` is the latest version's
/// number.
#[must_use]
pub fn showing(showing: Showing, latest: u32) -> (String, String) {
    match showing {
        Showing::Latest => (format!("Version {latest}"), format!("v{latest}")),
        Showing::Version(version) => (format!("Version {version}"), format!("v{version}")),
        Showing::Pass(pass) => (format!("Pass {pass} changes"), format!("Pass {pass}")),
        Showing::Compare => ("Compare versions".to_owned(), "Compare".to_owned()),
    }
}

/// A count of comments: `1 comment`, `3 comments`.
#[must_use]
pub fn comments(count: usize) -> String {
    if count == 1 {
        return "1 comment".to_owned();
    }

    format!("{count} comments")
}

/// The button that sends the `pending` comments to the assistant, in full and
/// for a narrow screen: `Send 2 comments` and `Send 2`, or `Send` when none
/// wait, which sends a message alone.
#[must_use]
pub fn send(pending: usize) -> (String, String) {
    if pending == 0 {
        return ("Send".to_owned(), "Send".to_owned());
    }

    (
        format!("Send {}", comments(pending)),
        format!("Send {pending}"),
    )
}

/// A file of the review where the file shown is picked: `docs/a.md`, or
/// `docs/a.md (2 unresolved)`.
#[must_use]
pub fn file(path: &str, unresolved: usize) -> String {
    if unresolved == 0 {
        return path.to_owned();
    }

    format!("{path} ({unresolved} unresolved)")
}

/// The switch between listing resolved comments and not: `Show 2 resolved`, or
/// `Hide resolved` while they are `shown`.
#[must_use]
pub fn resolved(count: usize, shown: bool) -> String {
    if shown {
        return "Hide resolved".to_owned();
    }

    format!("Show {count} resolved")
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

/// That `count` passages the reviewer approved changed since: `1 passage
/// changed since you approved it`.
#[must_use]
pub fn changed_since(count: usize) -> String {
    if count == 1 {
        return "1 passage changed since you approved it".to_owned();
    }

    format!("{count} passages changed since you approved them")
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
