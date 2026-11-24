//! The text the commands print for a person at a terminal, laid out from the
//! records in `view`.

use chrono::{DateTime, SecondsFormat, Utc};
use jp_review_api::{
    AnchorView, Change, Comment as CommentView, Counts, Detail, DiffView, Pass as PassView, Placed,
    Reply as ReplyView, Summary, Version,
};

use crate::{
    anchor::{Anchor, Target},
    model::{Comment, Reply},
    pass::Outcome,
    view::PassReport,
};

/// The width the label of a line `jp review submit` prints is padded to: the
/// longest verdict, `decline`.
const LABEL_WIDTH: usize = 7;

/// A review's line in `jp review ls`.
pub(crate) fn row(summary: &Summary) -> String {
    let head = &summary.head;

    format!(
        "{}  {:<6}  {}  {}{}\n",
        head.id,
        head.status,
        head.created_at.format("%Y-%m-%d"),
        head.title,
        counts(&summary.comments)
    )
}

/// How many comments stand where, ` (1 draft, 2 open)`, or nothing when there
/// are none.
fn counts(counts: &Counts) -> String {
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
        return String::new();
    }

    format!(" ({})", parts.join(", "))
}

/// A review, the closing message of each of its passes, and its comments, as
/// `jp review show` prints them.
pub(crate) fn detail(detail: &Detail) -> String {
    let head = &detail.head;
    let mut out = format!("# {}: {}\n\n", head.id, head.title);
    out.push_str(&format!("- **Status**: {}\n", head.status));
    out.push_str(&format!("- **Author**: {}\n", head.author));
    out.push_str(&format!("- **Files**: {}\n", head.files.join(", ")));
    out.push_str(&format!("- **Versions**: {}\n", versions(&detail.versions)));
    out.push_str(&format!("- **Showing**: version {}\n", detail.version));
    if let Some(conversation) = &detail.conversation {
        out.push_str(&format!("- **Conversation**: {conversation}\n"));
    }
    if !detail.passes.is_empty() {
        out.push_str(&format!("- **Passes**: {}\n", passes(&detail.passes)));
    }

    for pass in &detail.passes {
        out.push_str(&closing(pass));
    }
    for view in &detail.comments {
        out.push('\n');
        out.push_str(&comment(view, detail.version));
    }

    out
}

/// Each version's number, and its label if it has one: `1 (base), 2, 3`.
fn versions(versions: &[Version]) -> String {
    versions
        .iter()
        .map(|version| match &version.label {
            Some(label) => format!("{} ({label})", version.number),
            None => version.number.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Each pass's number and how it ended: `1 completed, 2 running`.
fn passes(passes: &[PassView]) -> String {
    passes
        .iter()
        .map(|pass| match &pass.ended {
            Some(ended) => format!("{} {}", pass.number, ended.outcome),
            None => format!("{} running", pass.number),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the assistant said last in a pass, under a heading naming the pass;
/// nothing for a pass whose turn left no closing message.
fn closing(pass: &PassView) -> String {
    let Some(ended) = &pass.ended else {
        return String::new();
    };
    let Some(answer) = ended
        .answer
        .as_deref()
        .map(str::trim)
        .filter(|answer| !answer.is_empty())
    else {
        return String::new();
    };

    format!(
        "\n## Pass {} ({})\n\n{answer}\n",
        pass.number, ended.outcome
    )
}

/// A comment, its anchor placed in version `shown`.
fn comment(view: &CommentView, shown: u32) -> String {
    let kind = if view.suggestion.is_some() {
        " suggestion"
    } else {
        ""
    };
    let mut out = format!("## {} ({}{kind})\n\n", view.id, view.status);
    out.push_str(&format!(
        "On {}, by {}, {}\n",
        place(&view.anchor, shown),
        view.author,
        time(view.created_at)
    ));

    out.push_str(&excerpt(view));
    if !view.body.is_empty() {
        out.push_str(&format!("\n{}\n", view.body));
    }
    for sent in &view.replies {
        out.push_str(&reply(sent, ""));
    }
    if let Some(draft) = &view.draft_reply {
        out.push_str(&reply(draft, "Draft reply from "));
    }
    if let Some(at) = view.resolved_at {
        out.push_str(&format!("\nResolved {}.\n", time(at)));
    }

    out
}

/// The text a comment's range covers, quoted, or as the diff its suggestion
/// makes; nothing for a comment on no range.
pub(crate) fn excerpt(view: &CommentView) -> String {
    match (view.excerpt.as_deref(), view.suggestion.as_deref()) {
        (Some(excerpt), Some(replacement)) => {
            format!("\n```diff\n{}```\n", changed(excerpt, replacement))
        }
        (Some(excerpt), None) if !excerpt.is_empty() => format!("\n{}", quote(excerpt)),
        _ => String::new(),
    }
}

/// What a comment is on, as it reads in version `shown`.
///
/// An outdated one names the version it was written in, and the first since
/// then that no longer holds its text.
pub(crate) fn place(view: &AnchorView, shown: u32) -> String {
    let written = view.version;
    let since = view.resolved.as_ref().map_or_else(
        || format!("in version {shown}"),
        |last| format!("since version {}", last.version + 1),
    );

    match (&view.path, &view.range, &view.projected) {
        (None, ..) => "the review".to_owned(),
        (Some(path), None, Some(_)) => path.clone(),
        (Some(path), None, None) => format!("{path} in version {written}, absent {since}"),
        (
            Some(path),
            Some(_),
            Some(Placed {
                range: Some(range), ..
            }),
        ) => format!("{path}:{range}"),
        (Some(path), Some(range), _) => {
            format!("{path}:{range} in version {written}, outdated {since}")
        }
    }
}

/// `text` as a Markdown quote, one `> ` line per line.
fn quote(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                ">\n".to_owned()
            } else {
                format!("> {line}\n")
            }
        })
        .collect()
}

/// A suggestion as diff lines: what the range holds, then what it would hold.
fn changed(excerpt: &str, replacement: &str) -> String {
    let removed = excerpt.lines().map(|line| format!("-{line}\n"));
    let added = replacement.lines().map(|line| format!("+{line}\n"));

    removed.chain(added).collect()
}

/// A reply under a comment, its heading opening with `lead`.
fn reply(reply: &ReplyView, lead: &str) -> String {
    let verdict = reply
        .verdict
        .map_or_else(String::new, |verdict| format!(", {verdict}"));

    format!(
        "\n### {lead}{}{verdict}, {}\n\n{}\n",
        reply.author,
        time(reply.created_at),
        reply.body
    )
}

fn time(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// `count` with the noun that fits it: `1 file`, `2 files`.
pub(crate) fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The line `jp review submit` prints for the assistant's reply to comment
/// `number`: `#2  decline  The longer wording names the failure case.`
pub(crate) fn reply_line(number: u32, reply: &Reply) -> String {
    let label = reply
        .verdict
        .map_or_else(|| "reply".to_owned(), |verdict| verdict.to_string());

    format!(
        "#{number}  {label:<LABEL_WIDTH$}  {}\n",
        first_line(&reply.body)
    )
}

/// The line `jp review submit` prints for a comment the assistant opened: `#4
/// opened   docs/a.md:12: The heading skips a level.`
pub(crate) fn opened_line(comment: &Comment) -> String {
    format!(
        "#{}  {:<LABEL_WIDTH$}  {}: {}\n",
        comment.number,
        "opened",
        written(&comment.content.anchor),
        first_line(&comment.content.body)
    )
}

/// What an anchor is on, as it was written: `the review`, `docs/a.md`, or
/// `docs/a.md:3:1-3:6`.
pub(crate) fn written(anchor: &Anchor) -> String {
    match &anchor.target {
        Target::Review => "the review".to_owned(),
        Target::File(path) => path.clone(),
        Target::Range(path, range) => format!("{path}:{range}"),
    }
}

/// The first line of `text` that holds something, trimmed.
fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// What `jp review submit` prints once the pass has ended: that it was stopped,
/// the version holding the assistant's edits, and the comments the assistant
/// left unanswered.
///
/// A failure is reported as the command's error instead.
pub(crate) fn pass_end(report: &PassReport<'_>) -> String {
    let pass = report.pass;
    let mut out = String::new();
    if pass
        .ended
        .as_ref()
        .is_some_and(|ended| ended.outcome == Outcome::Stopped)
    {
        out.push_str(&format!("Pass {} stopped.\n", pass.number));
    }

    let end = pass.ended.as_ref().and_then(|ended| ended.version);
    match end {
        Some(version) if !report.changed.is_empty() => out.push_str(&format!(
            "Version {version}: {} changed.\n",
            counted(report.changed.len(), "file", "files")
        )),
        _ => out.push_str("No file changed.\n"),
    }

    if !report.unanswered.is_empty() {
        let numbers: Vec<String> = report
            .unanswered
            .iter()
            .map(|comment| format!("#{}", comment.number))
            .collect();
        let them = if numbers.len() == 1 { "it" } else { "them" };
        out.push_str(&format!(
            "No reply to {}; the next submit sends {them} again.\n",
            numbers.join(", ")
        ));
    }

    out
}

/// What changed from one version to another, as a unified diff per file: each
/// path under `vN/`, the version it is read in, and `/dev/null` for a file
/// absent from a version.
pub(crate) fn diff(view: &DiffView) -> String {
    let mut out = String::new();
    for file in &view.files {
        let old = match file.change {
            Change::Added => "/dev/null".to_owned(),
            Change::Deleted | Change::Modified => format!("v{}/{}", view.from, file.path),
        };
        let new = match file.change {
            Change::Deleted => "/dev/null".to_owned(),
            Change::Added | Change::Modified => format!("v{}/{}", view.to, file.path),
        };
        out.push_str(&format!("--- {old}\n+++ {new}\n{}", file.diff));
    }

    out
}
