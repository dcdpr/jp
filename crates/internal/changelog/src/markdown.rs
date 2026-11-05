//! A release's two documents, as Markdown.
//!
//! [`notes`] renders the release notes: the overview, each highlight under its
//! title and credited to its authors and change requests, and every entry in a
//! list under its type's heading, with its credits.
//! A breaking entry is marked, and links to its section of the upgrade guide.
//!
//! [`upgrade_guide`] renders the upgrade guide: one section per entry with an
//! upgrade section, headed by its title, followed by the entry's credits and
//! the section's body.
//! A release without upgrade sections has no upgrade guide, which renders as
//! nothing.
//!
//! Neither document has a title or a date; whoever places them adds both.

use std::collections::HashMap;

use comrak::{Arena, nodes::NodeValue, parse_document};
use doc::heading::headings;

use crate::{
    document,
    forge::{Credit, CreditKind},
    fragment::{Fragment, Type},
    release::{Credited, Headings, Release, series},
};

/// Where the release notes link to the upgrade guide when nothing says
/// otherwise: a page beside them.
pub const UPGRADE_GUIDE: &str = "upgrade";

/// The release notes of `release`, each type's entries headed by `headings`.
///
/// A breaking entry links to its section of the upgrade guide at
/// `upgrade_guide`, such as `upgrade` or
/// `https://jp.computer/releases/0.3.0/upgrade`, with the anchor of the
/// section's heading after a `#`.
#[must_use]
pub fn notes(release: &Release, headings: &Headings, upgrade_guide: &str) -> String {
    let (_, anchors) = guide(release);
    let mut blocks = vec![release.overview.clone()];

    for highlight in &release.highlights {
        blocks.push(format!("## {}", highlight.file.title));
        blocks.push(highlight.file.body.clone());

        let authors = series(&highlight.file.authors);
        blocks.push(if highlight.credits.is_empty() {
            format!("By {authors}.")
        } else {
            format!("By {authors} in {}.", series(&links(&highlight.credits)))
        });
    }

    if !release.entries.is_empty() {
        blocks.push("## Changelog".to_owned());
    }
    for type_ in Type::ALL {
        let items: Vec<String> = release
            .entries
            .iter()
            .filter(|entry| entry.file.type_ == type_)
            .map(|entry| {
                let anchor = anchors.get(entry.slug.as_str()).map(String::as_str);
                item(entry, anchor, upgrade_guide)
            })
            .collect();
        if items.is_empty() {
            continue;
        }

        blocks.push(format!("### {}", headings.of(type_)));
        blocks.push(items.join("\n"));
    }

    blocks.join("\n\n") + "\n"
}

/// The upgrade guide of `release`, or nothing when no entry has an upgrade
/// section.
#[must_use]
pub fn upgrade_guide(release: &Release) -> String {
    guide(release).0
}

/// The upgrade guide of `release`, and the anchor of each section's heading, by
/// the slug of its entry.
///
/// Anchors are read from the rendered guide as the site gives them, so a
/// heading in a section's body that takes a section's slug first moves the
/// section's anchor along with it.
fn guide(release: &Release) -> (String, HashMap<&str, String>) {
    let mut guide = String::new();
    let mut sections = vec![];

    for entry in &release.entries {
        let Some(upgrade) = &entry.file.upgrade else {
            continue;
        };
        if !guide.is_empty() {
            guide.push('\n');
        }

        sections.push((entry.slug.as_str(), guide.matches('\n').count()));
        guide.push_str(&format!("## {}\n\n", upgrade.title));
        if !entry.credits.is_empty() {
            guide.push_str(&format!("{}\n\n", links(&entry.credits).join(", ")));
        }
        guide.push_str(&upgrade.body);
        guide.push('\n');
    }

    let slugs: HashMap<usize, String> = headings(&guide)
        .into_iter()
        .map(|heading| (heading.line, heading.slug))
        .collect();
    let anchors = sections
        .into_iter()
        .filter_map(|(slug, line)| Some((slug, slugs.get(&line)?.clone())))
        .collect();

    (guide, anchors)
}

/// `entry` as an item of its type's list: the entry, then for a breaking one a
/// link to its section of the upgrade guide at `upgrade_guide`, whose heading
/// has `anchor`, then its credits.
fn item(entry: &Credited<Fragment>, anchor: Option<&str>, upgrade_guide: &str) -> String {
    let mut tail = vec![];
    if entry.file.upgrade.is_some() {
        let link = match anchor {
            Some(anchor) => format!("{upgrade_guide}#{anchor}"),
            None => upgrade_guide.to_owned(),
        };
        tail.push(format!("**Breaking**: see [the upgrade guide]({link})."));
    }
    if !entry.credits.is_empty() {
        tail.push(format!("({})", links(&entry.credits).join(", ")));
    }

    let mut text = entry.file.entry.clone();
    if !tail.is_empty() {
        // The last paragraph goes on; after a code block, a list, or any other
        // block, a line would be read as part of it.
        text.push_str(if ends_in_paragraph(&text) {
            "\n"
        } else {
            "\n\n"
        });
        text.push_str(&tail.join("\n"));
    }

    list_item(&text)
}

/// `markdown` as one item of a bullet list: every line after the first indented
/// to the item's content, so each block stays inside it.
fn list_item(markdown: &str) -> String {
    let mut item = String::from("- ");
    for (index, line) in markdown.lines().enumerate() {
        if index > 0 {
            item.push('\n');
            if !line.is_empty() {
                item.push_str("  ");
            }
        }
        item.push_str(line);
    }

    item
}

/// Whether the last block of `markdown` is a paragraph.
fn ends_in_paragraph(markdown: &str) -> bool {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &document::markdown_options());

    root.last_child()
        .is_some_and(|node| matches!(node.data().value, NodeValue::Paragraph))
}

/// Each of `credits` as a link to its page, or as its label when it has none.
///
/// A commit's label is a hash, so it is shown as code.
fn links(credits: &[Credit]) -> Vec<String> {
    credits
        .iter()
        .map(|credit| {
            let label = match credit.kind {
                CreditKind::ChangeRequest => credit.label.clone(),
                CreditKind::Commit => format!("`{}`", credit.label),
            };
            match &credit.url {
                Some(url) => format!("[{label}]({url})"),
                None => label,
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
