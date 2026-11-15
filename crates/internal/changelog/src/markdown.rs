//! Markdown rendering for release notes ([`notes`]) and upgrade guides
//! ([`upgrade_guide`]).
//!
//! - [`notes`] renders the release overview, each highlight (`## <title>`,
//!   body, and `By <authors> in <credits>.`), and the `## Changelog` sections
//!   grouped by [`Type`].
//!   Breaking entries include a link to their upgrade guide section.
//! - [`upgrade_guide`] renders `## <title>` sections for each breaking entry,
//!   with its credits and upgrade instructions, or an empty string if the
//!   release has no breaking entries.
//!
//! Neither function emits a top-level `#` document title or release date;
//! callers prepend those for their target surface.

use std::collections::HashMap;

use comrak::{Arena, nodes::NodeValue, parse_document};
use doc::heading::headings;

use crate::{
    document,
    forge::{Credit, CreditKind},
    fragment::{Fragment, Type},
    release::{Credited, Headings, Release, series},
};

/// Default relative path from the release notes to the upgrade guide.
pub const UPGRADE_GUIDE: &str = "upgrade";

/// Render the Markdown release notes for `release`, using `headings` for
/// changelog section titles and `upgrade_guide` as the base URL/path for
/// breaking-change upgrade links.
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

/// Render the Markdown upgrade guide for `release`, or an empty string if no
/// entry has an upgrade section.
#[must_use]
pub fn upgrade_guide(release: &Release) -> String {
    guide(release).0
}

/// Render the upgrade guide for `release` and map each breaking entry's slug to
/// its generated heading anchor.
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

/// Render `entry` as a bullet list item with an optional upgrade-guide link and
/// credit list.
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
        // Append on the next line when `text` ends with a paragraph, or start a
        // new paragraph after a code block, list, or blockquote.
        text.push_str(if ends_in_paragraph(&text) {
            "\n"
        } else {
            "\n\n"
        });
        text.push_str(&tail.join("\n"));
    }

    list_item(&text)
}

/// Format `markdown` as a `- ` bullet item, indenting continuation lines by two
/// spaces.
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

fn ends_in_paragraph(markdown: &str) -> bool {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &document::markdown_options());

    root.last_child()
        .is_some_and(|node| matches!(node.data().value, NodeValue::Paragraph))
}

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
