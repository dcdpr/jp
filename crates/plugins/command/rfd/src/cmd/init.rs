//! `jp rfd init`: set up the RFD directory.
//!
//! Creates the directory and its drafts directory, a template for each
//! configured category, and an empty priority board.
//! Running it again writes the templates again, overwriting one that holds
//! something else only as the `overwrite` crate allows: when git can restore
//! it, or with `--force`.
//! The priority board is the project's ranking, not a template, so one that
//! exists is never touched.
//! It writes no RFD 001: the process document is each project's own.

use std::fs;

use camino::Utf8PathBuf;
use overwrite::Existing;

use super::capitalized;
use crate::{
    Output,
    paths::Paths,
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

/// A category's template, as `init` plans to write it.
struct Planned {
    /// The file's name in the RFD directory.
    name: Utf8PathBuf,
    text: String,
    existing: Existing,
}

/// Set up the RFD directory for `categories`.
///
/// A template git couldn't restore is overwritten only with `force`; without
/// it, nothing is written.
pub(crate) fn init(paths: &Paths, categories: &[String], force: bool) -> Result<Output, String> {
    let planned = plan(paths, categories)?;
    if !force {
        refuse_unrecorded(paths, &planned)?;
    }

    let mut output = Output::default();
    create_dirs(paths, &mut output)?;
    write_templates(paths, planned, &mut output)?;

    let board = paths.rfds.join(PRIORITY_FILE);
    if !board.exists() {
        paths.create(&board, &Priority::default().render())?;
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

/// Each category's template, with what is in its file's place.
fn plan(paths: &Paths, categories: &[String]) -> Result<Vec<Planned>, String> {
    categories
        .iter()
        .map(|category| {
            let name = Utf8PathBuf::from(format!("000-{category}-template.md"));
            let path = paths.rfds.join(&name);
            let text = template(category);
            let existing =
                overwrite::existing(&path, paths.formatting.apply(&path, &text).as_bytes())?;

            Ok(Planned {
                name,
                text,
                existing,
            })
        })
        .collect()
}

/// Refuse when a template that differs holds something git couldn't restore.
fn refuse_unrecorded(paths: &Paths, planned: &[Planned]) -> Result<(), String> {
    let differing: Vec<Utf8PathBuf> = planned
        .iter()
        .filter(|planned| planned.existing == Existing::Differs)
        .map(|planned| planned.name.clone())
        .collect();
    let unrecorded: Vec<_> = overwrite::unrecorded(&paths.rfds, &differing)?
        .into_iter()
        .map(|(name, why)| (Utf8PathBuf::from(paths.show(&paths.rfds.join(name))), why))
        .collect();
    if unrecorded.is_empty() {
        return Ok(());
    }

    Err(overwrite::refusal("jp rfd init", &unrecorded))
}

/// Create the RFD directory and its drafts directory where they are missing.
fn create_dirs(paths: &Paths, output: &mut Output) -> Result<(), String> {
    for dir in [paths.rfds.clone(), paths.drafts()] {
        if dir.is_dir() {
            continue;
        }
        fs::create_dir_all(&dir).map_err(|error| format!("{dir}: {error}"))?;
        output
            .text
            .push_str(&format!("Created {}/\n", paths.show(&dir)));
    }

    Ok(())
}

/// Write the templates that are missing or differ.
fn write_templates(
    paths: &Paths,
    planned: Vec<Planned>,
    output: &mut Output,
) -> Result<(), String> {
    for planned in planned {
        let path = paths.rfds.join(planned.name);
        let verb = match planned.existing {
            Existing::Same => continue,
            Existing::Missing => {
                paths.create(&path, &planned.text)?;
                "Wrote"
            }
            Existing::Differs => {
                paths.write(&path, &planned.text)?;
                "Overwrote"
            }
        };
        output
            .text
            .push_str(&format!("{verb} {}\n", paths.show(&path)));
    }

    Ok(())
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
