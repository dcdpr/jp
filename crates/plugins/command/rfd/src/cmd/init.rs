//! `jp rfd init`: set up the RFD directory.
//!
//! Creates the directory and its drafts directory, a template for each
//! configured category, and an empty priority board.
//! What is already there is left as it is, so running it again only adds what
//! is missing, such as the template for a category added to the configuration.
//! It writes no RFD 001: the process document is each project's own.

use std::fs;

use super::capitalized;
use crate::{
    Output,
    paths::{self, Paths},
    priority::{PRIORITY_FILE, Priority},
};

/// The templates for the default categories, by category.
const TEMPLATES: [(&str, &str); 4] = [
    ("design", include_str!("../../templates/design.md")),
    ("decision", include_str!("../../templates/decision.md")),
    ("guide", include_str!("../../templates/guide.md")),
    ("process", include_str!("../../templates/process.md")),
];

/// The template for a category without one of its own; `CATEGORY` is replaced
/// by the category's name.
const OTHER: &str = include_str!("../../templates/other.md");

/// Set up the RFD directory for `categories`.
pub(crate) fn init(paths: &Paths, categories: &[String]) -> Result<Output, String> {
    let mut output = Output::default();

    for dir in [paths.rfds.clone(), paths.drafts()] {
        if dir.is_dir() {
            continue;
        }
        fs::create_dir_all(&dir).map_err(|error| format!("{dir}: {error}"))?;
        output
            .text
            .push_str(&format!("Created {}/\n", paths.show(&dir)));
    }

    for category in categories {
        let path = paths.rfds.join(format!("000-{category}-template.md"));
        if path.exists() {
            continue;
        }
        paths::create(&path, &template(category))?;
        output
            .text
            .push_str(&format!("Wrote {}\n", paths.show(&path)));
    }

    let board = paths.rfds.join(PRIORITY_FILE);
    if !board.exists() {
        paths::create(&board, &Priority::default().render())?;
        output
            .text
            .push_str(&format!("Wrote {}\n", paths.show(&board)));
    }

    if output.text.is_empty() {
        output.text = format!(
            "{}/ already has its drafts directory, its templates, and its priority board.\n",
            paths.show(&paths.rfds)
        );
    }

    Ok(output)
}

/// The template a new project starts `category` with.
fn template(category: &str) -> String {
    TEMPLATES
        .iter()
        .find(|(name, _)| *name == category)
        .map_or_else(
            || OTHER.replacen("CATEGORY", &capitalized(category), 1),
            |(_, template)| (*template).to_owned(),
        )
}

#[cfg(test)]
#[path = "init_tests.rs"]
mod tests;
