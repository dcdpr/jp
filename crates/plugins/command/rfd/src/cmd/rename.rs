//! `jp rfd rename`: retitle an RFD and move its file to the slug the new title
//! produces.
//!
//! The id stays, so `RFD <id>` mentions stay right; links name the file, with
//! or without `.md`, so every one under the documentation tree the RFD
//! directory sits in, and in the ticket directory, is rewritten.
//! Links elsewhere are reported, not rewritten.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use super::{Leftovers, check_title};
use crate::{
    Output,
    document::Document,
    id::{self, RfdId},
    paths::{self, Paths},
    references::{self, Rule},
};

/// Give `id` the title `title`.
///
/// `scan_workspace` says whether the workspace may be searched for links the
/// rewrite didn't reach.
pub(crate) fn rename(
    paths: &Paths,
    id: RfdId,
    title: &str,
    scan_workspace: bool,
) -> Result<Output, String> {
    let title = title.trim();
    check_title(title)?;

    let file = paths.resolve(id)?;
    let dir = id.directory(&paths.rfds);
    let old_name = file.file_name().unwrap_or_default().to_owned();
    let old_stem = file.file_stem().unwrap_or_default().to_owned();
    let new_stem = format!("{id}-{}", id::slug(title));
    let new_file = dir.join(format!("{new_stem}.md"));
    if new_file != file && new_file.exists() {
        return Err(format!("{} already exists.", paths.show(&new_file)));
    }

    // The heading is written before the move, so an interrupted rename leaves
    // a readable RFD with a stale file name, and running it again finishes it.
    let mut document = Document::parse(&paths::read(&file)?);
    if !document.set_heading(&id.to_string(), title) {
        return Err(format!(
            "No '# RFD {id}: ...' heading in {}.",
            paths.show(&file)
        ));
    }
    paths::write(&file, &document.render())?;

    let mut output = Output::default();
    if new_file == file {
        output.text = format!("{old_name} -> {}\n", paths.show(&new_file));
        return Ok(output);
    }

    fs::rename(&file, &new_file).map_err(|error| format!("{file}: {error}"))?;

    // The stem, so a link written without `.md` moves too; a longer slug that
    // starts with this one is a different file and stays.
    let linking = paths.linking();
    let rules = [Rule::Stem {
        stem: old_stem.clone(),
        to: new_stem,
    }];
    let mut updated = 0;
    for other in linking.iter().flat_map(|dir| references::walk(dir)) {
        if references::rewrite_file(&other, &rules)? {
            output.text.push_str(&format!(
                "  updated link targets in {}\n",
                paths.show(&other)
            ));
            updated += 1;
        }
    }

    output
        .text
        .push_str(&format!("{old_name} -> {}\n", paths.show(&new_file)));
    if updated > 0 {
        output
            .text
            .push_str(&format!("Updated {updated} file(s) with link targets.\n"));
    }

    let covered: Vec<&Utf8Path> = linking.iter().map(Utf8PathBuf::as_path).collect();
    let scope: Vec<String> = linking
        .iter()
        .map(|dir| format!("{}/", paths.show(dir)))
        .collect();
    let leftovers = Leftovers {
        covered: &covered,
        scope: format!("outside {}", scope.join(" and ")),
        finding: format!("still name {old_stem}"),
    };
    output
        .warnings
        .extend(leftovers.report(paths, scan_workspace, |text| {
            references::names_stem(text, &old_stem)
        }));

    Ok(output)
}

#[cfg(test)]
#[path = "rename_tests.rs"]
mod tests;
