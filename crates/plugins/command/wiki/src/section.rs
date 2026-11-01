//! A document's sections: each heading, and what follows it up to the next
//! heading of the same or a higher level.
//!
//! The reference definitions `comfort` gathers at the end of a document belong
//! to no section, so reading or replacing the last section leaves them where
//! they are.
//! A section is named by its heading's anchor, or by its title when no anchor
//! matches and only one heading carries it.

use std::{ops::Range, sync::LazyLock};

use doc::heading::{Heading, headings};
use regex::Regex;

/// A link reference definition: `[label]: destination`.
static DEFINITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}\[[^\]]+\]:\s*\S").expect("valid regex"));

/// One section of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Section {
    pub heading: Heading,

    /// The 0-based lines it spans: its heading, and everything up to the next
    /// heading of its level or higher, or the definitions at the end.
    pub lines: Range<usize>,
}

/// Every section of `source`, in document order.
#[must_use]
pub(crate) fn sections(source: &str) -> Vec<Section> {
    let lines: Vec<&str> = source.lines().collect();
    let end = definitions_start(&lines);
    let all = headings(source);

    all.iter()
        .enumerate()
        .map(|(index, heading)| {
            let next = all[index + 1..]
                .iter()
                .find(|later| later.level <= heading.level)
                .map_or(end, |later| later.line.min(end));

            Section {
                heading: heading.clone(),
                lines: heading.line..next.max(heading.line + 1),
            }
        })
        .collect()
}

/// The section `wanted` names in `source`: the one whose anchor it is, or the
/// one heading whose title it is, ignoring case.
///
/// # Errors
///
/// Names the anchors to choose from when nothing matches, or when several
/// headings share the title.
pub(crate) fn find(source: &str, wanted: &str) -> Result<Section, String> {
    let wanted = wanted.trim().trim_start_matches('#');
    let all = sections(source);

    if let Some(section) = all.iter().find(|section| section.heading.slug == wanted) {
        return Ok(section.clone());
    }

    let titled: Vec<&Section> = all
        .iter()
        .filter(|section| section.heading.text.trim().eq_ignore_ascii_case(wanted))
        .collect();
    match titled.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!(
            "No section `{wanted}`; the sections are {}.",
            anchors(all.iter())
        )),
        several => Err(format!(
            "{} sections are titled `{wanted}`; name one by its anchor: {}.",
            several.len(),
            anchors(several.iter().copied())
        )),
    }
}

/// `#a`, `#b`, and `#c`, for a message.
fn anchors<'a>(sections: impl Iterator<Item = &'a Section>) -> String {
    let anchors: Vec<String> = sections
        .map(|section| format!("`#{}`", section.heading.slug))
        .collect();

    if anchors.is_empty() {
        "none".to_owned()
    } else {
        anchors.join(", ")
    }
}

/// The text of `section`, without the blank lines around it.
#[must_use]
pub(crate) fn text(source: &str, section: &Section) -> String {
    let lines: Vec<&str> = source.lines().collect();

    lines[section.lines.clone()].join("\n").trim().to_owned()
}

/// `source` with `section` replaced by `body`.
///
/// A body that opens with a heading replaces the section's own; any other keeps
/// the heading and replaces what follows it.
/// The section keeps one blank line between it and what comes after.
#[must_use]
pub(crate) fn replace(source: &str, section: &Section, body: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let body = body.trim();

    let start = if opens_with_heading(body) {
        section.lines.start
    } else {
        // The heading's own lines: one for `## Title`, two for an underlined
        // one.
        (section.heading.line + 1..section.lines.end)
            .find(|&index| lines[index].trim().is_empty() || !is_underline(lines[index]))
            .unwrap_or(section.lines.end)
    };

    let mut out: Vec<String> = lines[..start]
        .iter()
        .map(|line| (*line).to_owned())
        .collect();
    if start > section.lines.start {
        out.push(String::new());
    }
    out.extend(body.lines().map(ToOwned::to_owned));

    let rest: Vec<&str> = lines[section.lines.end..]
        .iter()
        .copied()
        .skip_while(|line| line.trim().is_empty())
        .collect();
    if !rest.is_empty() {
        out.push(String::new());
    }
    out.extend(rest.iter().map(|line| (*line).to_owned()));

    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Whether `text` opens with an ATX heading, `## Title`.
fn opens_with_heading(text: &str) -> bool {
    let hashes = text.chars().take_while(|c| *c == '#').count();

    (1..=6).contains(&hashes) && text[hashes..].starts_with([' ', '\t'])
}

/// Whether `line` underlines the heading above it, `-----` or `=====`.
fn is_underline(line: &str) -> bool {
    let line = line.trim();

    !line.is_empty() && (line.chars().all(|c| c == '-') || line.chars().all(|c| c == '='))
}

/// The index of the first line of the reference definitions a document ends
/// with, the blank lines before them included; the line count when it ends with
/// none.
#[must_use]
pub(crate) fn definitions_start(lines: &[&str]) -> usize {
    let mut start = lines.len();
    while start > 0 {
        let line = lines[start - 1];
        if !line.trim().is_empty() && !DEFINITION.is_match(line) {
            break;
        }
        start -= 1;
    }

    start
}

#[cfg(test)]
#[path = "section_tests.rs"]
mod tests;
