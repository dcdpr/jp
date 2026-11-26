//! Prompt formatting for review passes.
//!
//! A pass prompt opens with `# Review R-1h4k2m9, pass 1` and the reviewer's
//! optional pass message.
//! The first pass in a conversation includes the full numbered text of each
//! tracked file under `## <path> (version <N>)`.
//! Subsequent passes in the same conversation include a unified diff under `##
//! Changes since pass <M> was sent` against the version the latest pass whose
//! turn completed or stopped was sent with, so each file's full text is sent
//! only once.
//! The diff includes the assistant's own edits from that pass, because edits
//! made outside the turn while it ran can't be told apart from them.
//!
//! Under `## Read so far` and `## Approved`, every pass says how far the
//! reviewer has read each file and which passages they approved, as the pass's
//! starting version holds them.
//! An approved passage that changed since is left out: the approval was of the
//! text as it stood.
//!
//! Under `## Comments`, each submitted comment is rendered with its anchor in
//! the pass's starting version (e.g. `### R-1h4k2m9#1 at docs/a.md:3:1-3:6`),
//! the quoted excerpt or suggestion diff from its anchored version, and any
//! comment body or replies added by the reviewer since the assistant last
//! replied.

use std::collections::BTreeSet;

use jp_review_api::{Comment as CommentView, DiffView};

use crate::{
    id::ReviewId,
    model::{Comment, Review, Role},
    pass::{Carry, Pass},
    render,
    text::Text,
    view::{self, Texts},
};

/// What a pass message is made of.
pub(crate) struct Prompt<'a> {
    pub id: ReviewId,
    pub review: &'a Review,
    pub pass: &'a Pass,

    /// What the message carries of the files.
    pub carry: Carry,

    /// Every blob [`blobs`] names.
    pub texts: &'a Texts,
}

/// The blobs [`message`] reads for `pass` of `review`, carrying `carry`.
pub(crate) fn blobs<'a>(review: &'a Review, pass: &Pass, carry: Carry) -> BTreeSet<&'a str> {
    let mut versions = vec![pass.version];
    if let Carry::Changes { version, .. } = carry {
        versions.push(version);
    }

    let files = versions.into_iter().flat_map(|version| {
        review
            .paths
            .iter()
            .filter_map(move |path| review.blob(version, path))
    });
    let comments = pass
        .comments
        .iter()
        .filter_map(|number| review.comment(*number).ok())
        .flat_map(|comment| view::blobs_of(review, comment, pass.version));

    files
        .chain(comments)
        .chain(view::progress_blobs(review, pass.version))
        .collect()
}

/// The message `prompt` describes.
pub(crate) fn message(prompt: &Prompt<'_>) -> String {
    let mut out = format!("# Review {}, pass {}\n", prompt.id, prompt.pass.number);
    if let Some(message) = &prompt.pass.message {
        out.push_str(&format!("\n{message}\n"));
    }

    match prompt.carry {
        Carry::Files => out.push_str(&files(prompt)),
        Carry::Changes { pass, version } => out.push_str(&changes(prompt, pass, version)),
    }
    out.push_str(&read_so_far(prompt));
    out.push_str(&approved(prompt));

    let comments: Vec<&Comment> = prompt
        .pass
        .comments
        .iter()
        .filter_map(|number| prompt.review.comment(*number).ok())
        .collect();
    if !comments.is_empty() {
        out.push_str("\n## Comments\n");
    }
    for found in comments {
        out.push_str(&comment(prompt, found));
    }

    out
}

/// Each of the review's files whole, as the pass's version holds it.
fn files(prompt: &Prompt<'_>) -> String {
    let version = prompt.pass.version;
    let mut out = String::new();
    for path in &prompt.review.paths {
        match view::text_at(prompt.review, version, path, prompt.texts) {
            Some(text) => out.push_str(&format!(
                "\n## {path} (version {version})\n\n```\n{}```\n",
                numbered(text)
            )),
            None => out.push_str(&format!("\n## {path} (absent from version {version})\n")),
        }
    }

    out
}

/// `text`, each line after its number: ` 9 | line`.
///
/// Every line opens with its number, so no line of the text can close the code
/// block holding it.
fn numbered(text: &str) -> String {
    let lines = Text::new(text).shown();
    let width = lines.len().to_string().len();

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let number = index + 1;
            match line.trim_end_matches(['\n', '\r']) {
                "" => format!("{number:>width$} |\n"),
                line => format!("{number:>width$} | {line}\n"),
            }
        })
        .collect()
}

/// What changed in the files from `from`, the version pass `since` was sent
/// with, to the pass's version; nothing when nothing did.
fn changes(prompt: &Prompt<'_>, since: u32, from: u32) -> String {
    let (review, texts, to) = (prompt.review, prompt.texts, prompt.pass.version);
    let files: Vec<_> = review
        .paths
        .iter()
        .filter_map(|path| {
            let old = view::text_at(review, from, path, texts);
            view::file_diff(path, old, view::text_at(review, to, path, texts))
        })
        .collect();
    if files.is_empty() {
        return String::new();
    }

    let diff = render::diff(&DiffView {
        review: prompt.id.to_string(),
        from,
        to,
        files,
    });

    format!(
        "\n## Changes since pass {since} was sent\n\nThis includes your own edits from that \
         pass.\n\n```diff\n{diff}```\n"
    )
}

/// How far the reviewer has read each file they said so of, as the pass's
/// version holds it; nothing when they said so of none.
fn read_so_far(prompt: &Prompt<'_>) -> String {
    let (review, shown) = (prompt.review, prompt.pass.version);
    let files: String = review
        .progress
        .read_until
        .iter()
        .filter_map(|(path, found)| {
            let line = view::read_until(review, path, found, shown, prompt.texts)
                .projected?
                .range?;

            Some(format!("- {path}: through line {line}\n"))
        })
        .collect();
    if files.is_empty() {
        return String::new();
    }

    format!(
        "\n## Read so far\n\nThe reviewer has read these files this far, and no further. The \
         lines after are unread: that they carry no comment says nothing about them, and comments \
         on them may follow in a later pass.\n\n{files}"
    )
}

/// The passages the reviewer approved that stand unchanged in the pass's
/// version; nothing when there are none.
fn approved(prompt: &Prompt<'_>) -> String {
    let (review, shown) = (prompt.review, prompt.pass.version);
    let passages: String = review
        .progress
        .approvals
        .iter()
        .filter_map(|found| {
            let view = view::approval(review, found, shown, prompt.texts);
            view.anchor.projected.as_ref()?;
            let opening = render::opening(view.excerpt.as_deref().unwrap_or_default());

            Some(format!(
                "- {}, from \"{opening}\"\n",
                render::place(&view.anchor, shown)
            ))
        })
        .collect();
    if passages.is_empty() {
        return String::new();
    }

    format!(
        "\n## Approved\n\nThe reviewer read these passages and approved them as they stand. Leave \
         them as they are, unless acting on a comment needs one changed; say so in your reply \
         when it does.\n\n{passages}"
    )
}

/// One comment the pass sends: where it is, the text it is on, and what the
/// reviewer wrote in it since the assistant last did.
fn comment(prompt: &Prompt<'_>, found: &Comment) -> String {
    let shown = prompt.pass.version;
    let view = view::comment(prompt.id, prompt.review, found, shown, prompt.texts);

    let mut out = format!("\n### {}\n", heading(&view, shown));
    out.push_str(&render::excerpt(&view));

    let (body, replies) = found.unanswered();
    if body && !view.body.is_empty() {
        out.push_str(&format!("\n{}\n", view.body));
    }
    for reply in replies {
        out.push_str(&format!("\n#### Reply\n\n{}\n", reply.body));
    }

    out
}

/// A comment's heading: `R-1h4k2m9#2 at docs/a.md:60-64, suggestion`.
fn heading(view: &CommentView, shown: u32) -> String {
    let on = if view.anchor.range.is_some() {
        "at"
    } else {
        "on"
    };
    let mut heading = format!("{} {on} {}", view.id, render::place(&view.anchor, shown));
    if view.suggestion.is_some() {
        heading.push_str(", suggestion");
    }
    if view.role == Role::Assistant {
        heading.push_str(", opened by the assistant");
    }

    heading
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
