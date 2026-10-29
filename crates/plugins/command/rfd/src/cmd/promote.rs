//! `jp rfd promote`: advance an RFD one status along its lifecycle.
//!
//! - Draft to Discussion assigns the next permanent number, moves the file out
//!   of the drafts directory, and rewrites every reference to the draft's id in
//!   the RFDs, the drafts, and the tickets.
//! - Discussion to Accepted, and Accepted to Implemented, are gated on every
//!   RFD this one requires or extends having got that far first.
//! - Implemented drops the `Requires` links in both directions: a dependency
//!   that is built has nothing left to gate.
//!
//! The promoted file is reflowed with `comfort` the way CI's markdown check
//! lays it out.

use std::{collections::BTreeSet, fs, sync::LazyLock};

use camino::Utf8Path;
use comfort::{
    DEFAULT_MAX_WIDTH,
    format::{FormatOptions, format_markdown_with},
};
use regex::Regex;

use super::{is_draft, parse, stem};
use crate::{
    Output,
    document::{Document, NoMetadata},
    id::{self, ResolveError, RfdId},
    paths::{self, Paths},
    priority,
    references::{self, Rule, markdown_files},
};

/// An inline link target one directory up: the `](../` of `](../001-x.md)`.
static PARENT_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\]\(\.\./").expect("valid regex"));

/// A reference definition one directory up: `[RFD 001]: ../001-x.md`.
static PARENT_DEFINITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(\[[^\]]*\]:[ ]*)\.\./").expect("valid regex"));

/// A draft id, which counts as a mention only standing alone.
static DRAFT_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"D[0-9]{2}").expect("valid regex"));

/// The relationship fields a promotion reads, each with the field the other RFD
/// carries back.
const LINKS: [(&str, &str); 2] = [("Requires", "Required by"), ("Extends", "Extended by")];

/// Advance the RFD `id` one status.
pub(crate) fn promote(paths: &Paths, id: RfdId) -> Result<Output, String> {
    let file = paths.resolve(id)?;
    let document = Document::parse(&paths::read(&file)?);
    let status = document.header().status().unwrap_or_default();

    let output = match status.as_str() {
        "Draft" => to_discussion(paths, id, &file, document)?,
        "Discussion" => to_accepted(paths, &file, document)?,
        "Accepted" => to_implemented(paths, id, &file, document)?,
        other => {
            return Err(format!(
                "Cannot promote from '{other}'.\nPromotable statuses: Draft, Discussion, Accepted."
            ));
        }
    };

    Ok(output)
}

/// Publish a draft under the next permanent number.
fn to_discussion(
    paths: &Paths,
    draft: RfdId,
    file: &Utf8Path,
    mut document: Document,
) -> Result<Output, String> {
    let name = file.file_name().unwrap_or_default();
    check_publishable(&document, draft, name)?;

    let mut board = priority::read_for_update(&paths.rfds)?;
    let number = id::next_number(&paths.rfds)?;
    let old_stem = stem(name);
    let slug = &old_stem[draft.file_prefix().len()..];
    let new_name = format!("{number}-{slug}.md");
    let new_file = paths.rfds.join(&new_name);

    document.set_heading_id(&number.to_string());
    set_status(&mut document, "Discussion", file)?;
    paths::create(&new_file, &lift_links(&document.render()))?;
    fs::remove_file(file).map_err(|error| format!("{file}: {error}"))?;

    if let Some(board) = board.as_mut()
        && board.rewrite(&draft.to_string(), &number.to_string())
    {
        priority::write(&paths.rfds, board)?;
    }

    let mut output = Output::default();
    let updated = rewrite_references(paths, draft, number, old_stem, &new_name, &mut output)?;

    // Back-links from drafts were bookkeeping for the draft-to-draft policy; a
    // published RFD can't carry them.
    let mut promoted = Document::parse(&paths::read(&new_file)?);
    let required = promoted.uncite("Required by", is_draft);
    let extended = promoted.uncite("Extended by", is_draft);
    if required || extended {
        paths::write(&new_file, &promoted.render())?;
    }

    backfill(paths, number, &new_name, &promoted, &mut output)?;

    output.text.push_str(&format!(
        "{}: Draft -> Discussion (assigned {number})\n",
        paths.show(&new_file)
    ));
    if updated > 0 {
        output.text.push_str(&format!(
            "Updated {updated} cross-reference(s) in RFDs and tickets.\n"
        ));
    }

    reflow(&new_file)?;
    Ok(output)
}

/// Refuse a draft that would publish a reference to another draft: the docs
/// build rejects draft ids anywhere in a published RFD.
///
/// The draft's own id doesn't count, since the promotion rewrites it, and
/// neither do its `Required by` and `Extended by` back-links, which it drops.
fn check_publishable(document: &Document, draft: RfdId, name: &str) -> Result<(), String> {
    let header = document.header();
    for (field, _) in LINKS {
        if header.citations(field).iter().any(|cited| is_draft(cited)) {
            return Err(format!(
                "Cannot promote: '{field}' on {name} contains draft references.\n  {}\nPromote \
                 those drafts first, or remove the entries.",
                header.wrapped(field).unwrap_or_default()
            ));
        }
    }

    let own = draft.to_string();
    let strays: BTreeSet<String> = document
        .lines()
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            !header.is_field_line("Required by", *index)
                && !header.is_field_line("Extended by", *index)
        })
        .flat_map(|(_, line)| draft_tokens(line))
        .filter(|token| *token != own)
        .collect();

    if strays.is_empty() {
        return Ok(());
    }

    let listed: Vec<String> = strays.iter().map(|id| format!("  RFD {id}")).collect();
    Err(format!(
        "Cannot promote: {name} references other drafts in its body:\n{}\nPublished RFDs must not \
         reference drafts (the docs build rejects DNN tokens).\nPromote those drafts first, or \
         reword the references.",
        listed.join("\n")
    ))
}

/// The draft ids `line` mentions standing alone.
fn draft_tokens(line: &str) -> Vec<String> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';

    DRAFT_TOKEN
        .find_iter(line)
        .filter(|found| {
            let before = line[..found.start()].chars().next_back();
            let after = line[found.end()..].chars().next();
            !before.is_some_and(word) && !after.is_some_and(word)
        })
        .map(|found| found.as_str().to_owned())
        .collect()
}

/// Strip one `../` from the document's link targets, inline and by reference:
/// it moves from the drafts directory up into the RFD directory.
fn lift_links(text: &str) -> String {
    let text = PARENT_LINK.replace_all(text, "](");
    PARENT_DEFINITION.replace_all(&text, "$1").into_owned()
}

/// Rewrite the references to `draft` in every RFD, draft, and ticket, the
/// promoted file included, returning how many files changed.
fn rewrite_references(
    paths: &Paths,
    draft: RfdId,
    number: RfdId,
    old_stem: &str,
    new_name: &str,
    output: &mut Output,
) -> Result<usize, String> {
    let mut updated = 0;
    for dir in [paths.rfds.clone(), paths.drafts(), paths.tickets.clone()] {
        let rules = draft_rules(paths, &dir, draft, number, old_stem, new_name);
        for file in markdown_files(&dir)? {
            if references::rewrite_file(&file, &rules)? {
                output.text.push_str(&format!(
                    "  updated {draft} -> {number} references in {}\n",
                    paths.show(&file)
                ));
                updated += 1;
            }
        }
    }

    Ok(updated)
}

/// The rules that move the references in documents in `dir` from `draft` to
/// `number`.
///
/// A link target follows the file out of the drafts directory, so it is
/// rewritten relative to `dir`; a path from the workspace root keeps its form.
fn draft_rules(
    paths: &Paths,
    dir: &Utf8Path,
    draft: RfdId,
    number: RfdId,
    old_stem: &str,
    new_name: &str,
) -> Vec<Rule> {
    let new_stem = stem(new_name);
    let link = references::link(dir, &paths.rfds, new_name);
    let link_stem = stem(&link).to_owned();

    let mut rules = vec![Rule::Citation {
        old: draft.to_string(),
        new: number.to_string(),
    }];

    let rfds = paths.show(&paths.rfds);
    if !rfds.is_empty() {
        rules.push(Rule::Stem {
            stem: format!("{rfds}/drafts/{old_stem}"),
            to: format!("{rfds}/{new_stem}"),
        });
    }

    let to_drafts = references::relative_path(dir, &paths.drafts());
    if !to_drafts.is_empty() {
        rules.push(Rule::Stem {
            stem: format!("{to_drafts}/{old_stem}"),
            to: link_stem.clone(),
        });
    }

    rules.push(Rule::Stem {
        stem: old_stem.to_owned(),
        to: link_stem,
    });
    rules.push(Rule::Token {
        old: draft.to_string(),
        new: number.to_string(),
    });

    rules
}

/// Give every RFD the promoted one requires or extends its back-link, which a
/// draft never received from a published RFD.
fn backfill(
    paths: &Paths,
    number: RfdId,
    new_name: &str,
    promoted: &Document,
    output: &mut Output,
) -> Result<(), String> {
    for (forward, inverse) in LINKS {
        for cited in promoted.header().citations(forward) {
            let Some(Ok(target)) = parse(&cited).map(|id| paths.resolve(id)) else {
                continue;
            };
            let dir = target.parent().unwrap_or(&paths.rfds);
            let link = format!(
                "[RFD {number}]({})",
                references::link(dir, &paths.rfds, new_name)
            );

            let mut document = Document::parse(&paths::read(&target)?);
            match document.cite(inverse, &number.to_string(), &link) {
                Ok(true) => {
                    paths::write(&target, &document.render())?;
                    output.text.push_str(&format!(
                        "  backfilled {inverse}: RFD {number} into {}\n",
                        paths.show(&target)
                    ));
                }
                Ok(false) => {}
                Err(NoMetadata) => output.warnings.push(format!(
                    "Skipped backfilling {inverse}: RFD {number} into {}: it has no metadata \
                     block.",
                    paths.show(&target)
                )),
            }
        }
    }

    Ok(())
}

fn to_accepted(paths: &Paths, file: &Utf8Path, mut document: Document) -> Result<Output, String> {
    gate(paths, &document, "Accepted", &[
        "Accepted",
        "Implemented",
        "Superseded",
    ])?;

    set_status(&mut document, "Accepted", file)?;
    paths::write(file, &document.render())?;
    reflow(file)?;

    Ok(format!("{}: Discussion -> Accepted\n", paths.show(file)).into())
}

fn to_implemented(
    paths: &Paths,
    id: RfdId,
    file: &Utf8Path,
    mut document: Document,
) -> Result<Output, String> {
    gate(paths, &document, "Implemented", &[
        "Implemented",
        "Superseded",
    ])?;
    let mut board = priority::read_for_update(&paths.rfds)?;

    let this = id.to_string();
    let requires = document.header().citations("Requires");
    set_status(&mut document, "Implemented", file)?;
    document.remove("Requires");
    document.remove("Required by");
    paths::write(file, &document.render())?;

    let mut output = Output::default();
    for cited in requires {
        let Some(Ok(target)) = parse(&cited).map(|id| paths.resolve(id)) else {
            continue;
        };
        let mut target_document = Document::parse(&paths::read(&target)?);
        if target_document.uncite("Required by", |id| id == this) {
            paths::write(&target, &target_document.render())?;
            output.text.push_str(&format!(
                "  stripped Required by: RFD {this} from {}\n",
                paths.show(&target)
            ));
        }
    }

    // Every dependent, found by its own `Requires` rather than through this
    // RFD's `Required by`, which never lists a draft.
    for other in markdown_files(&paths.rfds)?
        .into_iter()
        .chain(markdown_files(&paths.drafts())?)
    {
        if other == file || other.file_name().and_then(RfdId::of_file).is_none() {
            continue;
        }
        let mut dependent = Document::parse(&paths::read(&other)?);
        if dependent.uncite("Requires", |id| id == this) {
            paths::write(&other, &dependent.render())?;
            output.text.push_str(&format!(
                "  stripped Requires: RFD {this} from {}\n",
                paths.show(&other)
            ));
        }
    }

    if let Some(board) = board.as_mut()
        && board.remove(&[&this])
    {
        priority::write(&paths.rfds, board)?;
    }

    output
        .text
        .push_str(&format!("{}: Accepted -> Implemented\n", paths.show(file)));

    reflow(file)?;
    Ok(output)
}

/// Refuse a promotion to `next` while an RFD this one requires or extends is at
/// a status outside `allowed`.
fn gate(paths: &Paths, document: &Document, next: &str, allowed: &[&str]) -> Result<(), String> {
    let header = document.header();
    let dependencies: BTreeSet<String> = LINKS
        .iter()
        .flat_map(|(field, _)| header.citations(field))
        .collect();

    let mut unmet = vec![];
    for dependency in &dependencies {
        let resolved = parse(dependency).map(|id| id::resolve(&paths.rfds, id));
        let status = match resolved {
            Some(Ok(file)) => Document::parse(&paths::read(&file)?).header().status(),
            Some(Err(ResolveError::Ambiguous { .. })) => {
                unmet.push(format!("  RFD {dependency} (shared by more than one file)"));
                continue;
            }
            _ => {
                unmet.push(format!("  RFD {dependency} (not found)"));
                continue;
            }
        };

        match status {
            Some(status) if allowed.contains(&status.as_str()) => {}
            status => unmet.push(format!(
                "  RFD {dependency} ({})",
                status.unwrap_or_default()
            )),
        }
    }

    if unmet.is_empty() {
        return Ok(());
    }

    Err(format!(
        "Cannot promote to {next}: dependencies not satisfied:\n{}\nRequired: status is one of: {}",
        unmet.join("\n"),
        allowed.join(", ")
    ))
}

/// Set the status of the document read from `file`.
fn set_status(document: &mut Document, status: &str, file: &Utf8Path) -> Result<(), String> {
    document
        .set("Status", status)
        .map_err(|NoMetadata| format!("{file} has no metadata block to set the status in."))
}

/// Lay the promoted file out the way CI's markdown check does: semantic line
/// breaks, canonical markdown, and reference-style links collected at the end.
fn reflow(file: &Utf8Path) -> Result<(), String> {
    let source = paths::read(file)?;
    let formatted = format_markdown_with(&source, &FormatOptions {
        max_width: DEFAULT_MAX_WIDTH,
        canonical: true,
        reference_links: true,
        prune_reference_links: false,
    });

    if formatted != source {
        paths::write(file, &formatted)?;
    }

    Ok(())
}

#[cfg(test)]
#[path = "promote_tests.rs"]
mod tests;
