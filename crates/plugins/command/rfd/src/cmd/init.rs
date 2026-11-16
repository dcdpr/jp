//! `jp rfd init`: initialize the RFD directory.
//!
//! Creates the RFD directory, the `drafts/` subdirectory, a template for each
//! configured category, and an empty `.priority.json` board.
//! Re-running `init` writes any missing templates and overwrites modified
//! templates only when git can restore them or `--force` is passed (see
//! [`overwrite`]).
//! An existing `.priority.json` holds project state and is never overwritten.
//! `init` does not create RFD 001, as each project defines its own RFD process.

use std::fs;

use camino::Utf8PathBuf;
use overwrite::Existing;

use super::capitalized;
use crate::{
    Output,
    paths::Paths,
    priority::{PRIORITY_FILE, Priority},
};

/// Built-in templates for the default RFD categories.
const TEMPLATES: [(&str, &str); 4] = [
    ("design", include_str!("../../templates/design.md")),
    ("decision", include_str!("../../templates/decision.md")),
    ("guide", include_str!("../../templates/guide.md")),
    ("process", include_str!("../../templates/process.md")),
];

/// Fallback template for custom categories; `CATEGORY` is replaced with the
/// capitalized category name.
const OTHER: &str = include_str!("../../templates/other.md");

/// Planned template file and its current state on disk.
struct Planned {
    name: Utf8PathBuf,
    text: String,
    existing: Existing,
}

/// Initialize the RFD directory and category templates for `categories`.
///
/// Modified templates that git cannot restore are overwritten only when `force`
/// is true; otherwise no files are written.
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

/// Build the template write plan for `categories` and check each target path on
/// disk.
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

/// Return a refusal error if any differing template cannot be restored by git.
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

/// Create the RFD directory and its `drafts/` subdirectory if missing.
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

/// Write missing or differing category templates.
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

/// Return the template contents for `category`.
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
