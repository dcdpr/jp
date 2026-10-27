//! `jp rfd renumber`: move an RFD to another id in its id space.
//!
//! A draft moves to another draft slot and a published RFD to another permanent
//! number; moving between the two is `promote`'s job.
//! The file keeps its directory and slug, so a link's path around the file name
//! stays right and only the name changes.
//!
//! References are rewritten in the RFDs, the drafts, and the tickets: the file
//! name everywhere, and `RFD <old>`, plus a draft's bare `D05` and the labels
//! that link a ticket to the RFD ([`Links`]), unless another file still holds
//! the old id, which would make every such mention ambiguous.
//! Mentions elsewhere in the workspace are reported, not rewritten.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use super::{Leftovers, listed, stem};
use crate::{
    Output,
    document::Document,
    id::{self, RfdId},
    links::Links,
    paths::{self, Paths},
    priority,
    references::{self, Rule, markdown_files},
};

/// Move the RFD `given` names to `target`, or to the next free id in its id
/// space.
///
/// `given` is an id, or a file name, which picks one of two documents sharing
/// an id after a merge.
/// `scan_workspace` says whether the workspace may be searched for mentions the
/// rewrite didn't reach.
pub(crate) fn renumber(
    paths: &Paths,
    links: &Links,
    given: &str,
    target: Option<&str>,
    scan_workspace: bool,
) -> Result<Output, String> {
    let (old, file) = resolve_given(paths, given)?;
    let dir = old.directory(&paths.rfds);
    let old_name = file.file_name().unwrap_or_default().to_owned();
    let old_stem = stem(&old_name).to_owned();
    let slug = &old_stem[old.file_prefix().len()..];

    // Another file holding the old id makes every id-keyed reference in the
    // repository unattributable.
    let siblings: Vec<Utf8PathBuf> = markdown_files(&dir)?
        .into_iter()
        .filter(|other| {
            *other != file
                && other
                    .file_name()
                    .is_some_and(|name| name.starts_with(&old.file_prefix()))
        })
        .collect();

    let new = match target {
        Some(text) => parse_target(old, text)?,
        None if matches!(old, RfdId::Draft(_)) => id::next_draft_slot(&paths.rfds)?,
        None => id::next_number(&paths.rfds)?,
    };
    if new == old {
        return Err(format!("RFD {old} already has that id; nothing to do."));
    }
    if let Some(taken) = markdown_files(&dir)?.iter().find(|other| {
        other
            .file_name()
            .is_some_and(|name| name.starts_with(&new.file_prefix()))
    }) {
        return Err(format!(
            "Target id {new} is taken by {}.",
            taken.file_name().unwrap_or_default()
        ));
    }

    let mut board = if siblings.is_empty() {
        priority::read_for_update(&paths.rfds)?
    } else {
        None
    };

    let new_name = format!("{new}-{slug}.md");
    let new_file = dir.join(&new_name);
    let mut document = Document::parse(&paths::read(&file)?);
    document.set_heading_id(&new.to_string());
    paths::create(&new_file, &document.render())?;
    fs::remove_file(&file).map_err(|error| format!("{file}: {error}"))?;

    // A board entry holds an id, so with the id shared there is no telling
    // which document it ranked.
    if let Some(board) = board.as_mut()
        && board.rewrite(&old.to_string(), &new.to_string())
    {
        priority::write(&paths.rfds, board)?;
    }

    let mut output = Output::default();
    let updated = rewrite_references(
        paths,
        links,
        old,
        new,
        (&old_stem, stem(&new_name)),
        &new_file,
        siblings.is_empty(),
        &mut output,
    )?;

    output.text.push_str(&format!(
        "{old_name} -> {} ({old} -> {new})\n",
        paths.show(&new_file)
    ));
    if updated > 0 {
        output.text.push_str(&format!(
            "Updated {updated} file(s) with cross-references.\n"
        ));
    }

    if !siblings.is_empty() {
        output.warnings.push(format!(
            "{}\nMentions of \"RFD {old}\" outside {new_name}, and a board entry for {old}, were \
             left alone: they name either document. Check them by hand.",
            listed(
                &format!("Warning: {old} is still held by:"),
                paths,
                &siblings
            )
        ));
    }

    let leftovers = Leftovers {
        covered: &[&paths.rfds, &paths.tickets],
        scope: format!(
            "outside {}/ and {}/",
            paths.show(&paths.rfds),
            paths.show(&paths.tickets)
        ),
        finding: format!("still mention {old}"),
    };
    output
        .warnings
        .extend(leftovers.report(paths, scan_workspace, |text| {
            references::cites(text, &old.to_string()) || text.contains(&old_name)
        }));

    Ok(output)
}

/// Rewrite the references to `old` in every RFD, draft, and ticket, returning
/// how many files changed.
///
/// `stems` is the old and new file stem.
/// Id-keyed references, a ticket's link labels among them, are rewritten only
/// when `unique` says no other file holds the old id, or inside the renumbered
/// file itself.
fn rewrite_references(
    paths: &Paths,
    links: &Links,
    old: RfdId,
    new: RfdId,
    stems: (&str, &str),
    new_file: &Utf8Path,
    unique: bool,
    output: &mut Output,
) -> Result<usize, String> {
    let file_name = Rule::Stem {
        stem: stems.0.to_owned(),
        to: stems.1.to_owned(),
    };
    let mut by_id = vec![Rule::Citation {
        old: old.to_string(),
        new: new.to_string(),
    }];
    // `D24` is a distinctive token; a bare `095` is not.
    if matches!(old, RfdId::Draft(_)) {
        by_id.push(Rule::Token {
            old: old.to_string(),
            new: new.to_string(),
        });
    }

    let mut updated = 0;
    for dir in [paths.rfds.clone(), paths.drafts(), paths.tickets.clone()] {
        for file in markdown_files(&dir)? {
            let mut rules = vec![file_name.clone()];
            if unique || file == new_file {
                rules.extend(by_id.iter().cloned());
            }

            let mut changed = references::rewrite_file(&file, &rules)?;
            if unique && dir == paths.tickets {
                changed |= links.relink(&file, old, new)?;
            }

            if changed {
                output.text.push_str(&format!(
                    "  updated {old} -> {new} references in {}\n",
                    paths.show(&file)
                ));
                updated += 1;
            }
        }
    }

    Ok(updated)
}

/// The RFD and file `given` names: an id, or a file name with or without a
/// directory and `.md`.
fn resolve_given(paths: &Paths, given: &str) -> Result<(RfdId, Utf8PathBuf), String> {
    let given = given.trim();
    if looks_like_id(given) {
        let id: RfdId = given
            .parse()
            .map_err(|error: id::InvalidId| error.to_string())?;
        return Ok((id, paths.resolve(id)?));
    }

    let path = Utf8Path::new(given);
    let mut name = path.file_name().unwrap_or(given).to_owned();
    if Utf8Path::new(&name).extension() != Some("md") {
        name.push_str(".md");
    }
    let as_given = if path.is_absolute() {
        path.to_path_buf()
    } else {
        paths.root.join(path)
    };

    let file = [as_given, paths.rfds.join(&name), paths.drafts().join(&name)]
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| format!("No RFD file matching '{given}'."))?;

    let name = file.file_name().unwrap_or_default();
    if name.starts_with("000-") {
        return Err("Refusing to renumber a template.".to_owned());
    }
    let id = RfdId::of_file(name).ok_or_else(|| {
        format!(
            "'{}' does not start with an RFD id (DNN or NNN).",
            paths.show(&file)
        )
    })?;

    let expected = id.directory(&paths.rfds);
    if file.parent() != Some(expected.as_path()) {
        return Err(format!(
            "{} is not in {}, where RFD {id} belongs.",
            paths.show(&file),
            paths.show(&expected)
        ));
    }

    Ok((id, file))
}

/// Whether `text` is written as an id rather than a file name: `42`, `042`,
/// `D24`, `RFD 042`.
fn looks_like_id(text: &str) -> bool {
    let bare = text
        .strip_prefix("RFD ")
        .or_else(|| text.strip_prefix("rfd "))
        .unwrap_or(text)
        .trim_start();
    let digits = bare.strip_prefix(['D', 'd']).unwrap_or(bare);

    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The id `text` names as the target for `old`, which has to stay in its id
/// space.
fn parse_target(old: RfdId, text: &str) -> Result<RfdId, String> {
    let text = text.trim();

    match old {
        RfdId::Draft(_) => {
            let digits = text
                .strip_prefix(['D', 'd'])
                .filter(|digits| {
                    (1..=2).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
                })
                .ok_or_else(|| {
                    format!("Target for a draft must be a draft slot (D01-D99), got '{text}'.")
                })?;
            match digits.parse::<u8>() {
                Ok(0) | Err(_) => Err("Target id must be greater than zero.".to_owned()),
                Ok(slot) => Ok(RfdId::Draft(slot)),
            }
        }
        RfdId::Published(_) => {
            if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!(
                    "Target for a published RFD must be a number, got '{text}'."
                ));
            }
            let significant = text.trim_start_matches('0');
            match significant.parse::<u16>() {
                _ if significant.is_empty() => {
                    Err("Target id must be greater than zero.".to_owned())
                }
                Ok(number) if number <= 999 => Ok(RfdId::Published(number)),
                _ => Err(format!(
                    "Target for a published RFD must be at most 999, got '{text}'."
                )),
            }
        }
    }
}

#[cfg(test)]
#[path = "renumber_tests.rs"]
mod tests;
