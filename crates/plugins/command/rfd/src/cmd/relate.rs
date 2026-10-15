//! `jp rfd extend`, `require`, and `supersede`: record a relationship between
//! two RFDs in both documents.
//!
//! Each takes the RFD that holds the relationship first: `extend 116 100`
//! records that 116 extends 100, `require 116 072` that 116 requires 072, and
//! `supersede 033 034` that 033 is superseded by 034.
//!
//! `Requires` and `Extends` are one dependency graph for the promotion gate, so
//! a link that would close a cycle in it is refused, and so is one target
//! listed under both.
//! A published RFD never links to a draft: a draft may require or extend a
//! published RFD, but the published one gets no back-link until the draft is
//! promoted.

use std::collections::BTreeSet;

use camino::Utf8Path;

use super::{is_draft, parse};
use crate::{
    Output,
    document::{Document, NoMetadata},
    id::RfdId,
    paths::{self, Paths},
    priority, references,
};

/// A dependency between two RFDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relation {
    Requires,
    Extends,
}

impl Relation {
    /// The field the dependent RFD carries.
    fn forward(self) -> &'static str {
        match self {
            Self::Requires => "Requires",
            Self::Extends => "Extends",
        }
    }

    /// The field the RFD depended on carries back.
    fn inverse(self) -> &'static str {
        match self {
            Self::Requires => "Required by",
            Self::Extends => "Extended by",
        }
    }

    /// The other relation, which must not list the same target.
    fn other(self) -> Self {
        match self {
            Self::Requires => Self::Extends,
            Self::Extends => Self::Requires,
        }
    }
}

/// Statuses an RFD can be extended from: not one that was abandoned or
/// replaced.
const EXTENDABLE: [&str; 4] = ["Draft", "Discussion", "Accepted", "Implemented"];

/// Record that `id` extends `target`.
pub(crate) fn extend(paths: &Paths, id: RfdId, target: RfdId) -> Result<Output, String> {
    let target_file = paths.resolve(target)?;
    let status = Document::parse(&paths::read(&target_file)?)
        .header()
        .status()
        .unwrap_or_default();
    if !EXTENDABLE.contains(&status.as_str()) {
        return Err(format!(
            "Cannot extend RFD {target} (status: '{status}').\nAbandoned and Superseded RFDs \
             cannot be extended."
        ));
    }

    link(paths, Relation::Extends, id, target)
}

/// Record that `id` requires `target`.
pub(crate) fn require(paths: &Paths, id: RfdId, target: RfdId) -> Result<Output, String> {
    link(paths, Relation::Requires, id, target)
}

/// Write `relation` from `source` to `target` into both documents.
///
/// Both edits are checked before either is written.
fn link(paths: &Paths, relation: Relation, source: RfdId, target: RfdId) -> Result<Output, String> {
    let source_file = paths.resolve(source)?;
    let target_file = paths.resolve(target)?;
    let (source_id, target_id) = (source.to_string(), target.to_string());

    if !is_draft(&source_id) && is_draft(&target_id) {
        return Err(format!(
            "Refused: published RFD {source} cannot link to draft RFD {target}.\nPromote {target} \
             first, or move the relationship to a draft."
        ));
    }

    let mut source_document = Document::parse(&paths::read(&source_file)?);
    let other = relation.other().forward();
    if source_document
        .header()
        .citations(other)
        .contains(&target_id)
    {
        return Err(format!(
            "Refused: RFD {source} already lists RFD {target} under '{other}'.\nExtension implies \
             dependency; don't list the same target under both. Drop one entry first."
        ));
    }

    if reaches(paths, target, source)? {
        return Err(format!(
            "Refused: cycle detected. Adding RFD {source} -> RFD {target} on '{}' would close a \
             loop in the Requires/Extends graph.",
            relation.forward()
        ));
    }

    let mut output = Output::default();
    let forward = relation.forward();
    let forward_link = format!(
        "[RFD {target}]({})",
        link_between(&source_file, &target_file)
    );
    let wrote_forward = source_document
        .cite(forward, &target_id, &forward_link)
        .map_err(|NoMetadata| no_metadata(paths, &source_file))?;

    // A draft's dependency on a published RFD is written on the draft only;
    // the back-link is added when the draft is promoted.
    let inverse = relation.inverse();
    let suppressed = is_draft(&source_id) && !is_draft(&target_id);
    let mut target_document = None;
    if !suppressed {
        let mut document = Document::parse(&paths::read(&target_file)?);
        let inverse_link = format!(
            "[RFD {source}]({})",
            link_between(&target_file, &source_file)
        );
        let wrote = document
            .cite(inverse, &source_id, &inverse_link)
            .map_err(|NoMetadata| no_metadata(paths, &target_file))?;
        target_document = Some((document, wrote));
    }

    if wrote_forward {
        paths::write(&source_file, &source_document.render())?;
        output.text.push_str(&format!(
            "{}: {forward}: RFD {target}\n",
            paths.show(&source_file)
        ));
    } else {
        output.text.push_str(&format!(
            "{}: already lists RFD {target} under {forward}\n",
            paths.show(&source_file)
        ));
    }

    match target_document {
        Some((document, true)) => {
            paths::write(&target_file, &document.render())?;
            output.text.push_str(&format!(
                "{}: {inverse}: RFD {source}\n",
                paths.show(&target_file)
            ));
        }
        Some((_, false)) => output.text.push_str(&format!(
            "{}: already lists RFD {source} under {inverse}\n",
            paths.show(&target_file)
        )),
        None => output.text.push_str(&format!(
            "{}: skipped {inverse}: RFD {source} (suppressed: draft -> non-draft)\n",
            paths.show(&target_file)
        )),
    }

    Ok(output)
}

/// Whether `to` is reachable from `from` through `Requires` and `Extends`,
/// `from` itself included.
///
/// An RFD that can't be found or read is a dead end, not an error: the walk
/// only has to find a path that exists.
fn reaches(paths: &Paths, from: RfdId, to: RfdId) -> Result<bool, String> {
    let mut seen = BTreeSet::new();
    let mut frontier = vec![from];

    while let Some(current) = frontier.pop() {
        if current == to {
            return Ok(true);
        }
        if !seen.insert(current.to_string()) {
            continue;
        }

        let Ok(file) = paths.resolve(current) else {
            continue;
        };
        let document = Document::parse(&paths::read(&file)?);
        for relation in [Relation::Requires, Relation::Extends] {
            frontier.extend(
                document
                    .header()
                    .citations(relation.forward())
                    .iter()
                    .filter_map(|cited| parse(cited)),
            );
        }
    }

    Ok(false)
}

/// Record that `old` is superseded by `by`.
///
/// Only an Accepted or Implemented RFD can be superseded, and only by a
/// published one: a draft replaced before it is published is deleted or
/// abandoned instead (RFD 001).
pub(crate) fn supersede(paths: &Paths, old: RfdId, by: RfdId) -> Result<Output, String> {
    if old == by {
        return Err(format!("RFD {old} cannot supersede itself."));
    }
    if is_draft(&old.to_string()) {
        return Err(format!(
            "Cannot supersede RFD {old}: a draft is deleted or abandoned, not superseded."
        ));
    }
    if is_draft(&by.to_string()) {
        return Err(format!(
            "Cannot supersede RFD {old} with RFD {by}: promote the draft first."
        ));
    }

    let old_file = paths.resolve(old)?;
    let new_file = paths.resolve(by)?;
    let mut old_document = Document::parse(&paths::read(&old_file)?);
    let mut new_document = Document::parse(&paths::read(&new_file)?);

    let status = old_document.header().status().unwrap_or_default();
    if !matches!(status.as_str(), "Accepted" | "Implemented") {
        return Err(format!(
            "Cannot supersede from '{status}'.\nOnly Accepted or Implemented RFDs can be \
             superseded."
        ));
    }

    let mut board = priority::read_for_update(&paths.rfds)?;

    old_document
        .set("Status", "Superseded")
        .map_err(|NoMetadata| no_metadata(paths, &old_file))?;
    old_document
        .cite(
            "Superseded by",
            &by.to_string(),
            &format!("[RFD {by}]({})", link_between(&old_file, &new_file)),
        )
        .map_err(|NoMetadata| no_metadata(paths, &old_file))?;
    new_document
        .cite(
            "Supersedes",
            &old.to_string(),
            &format!("[RFD {old}]({})", link_between(&new_file, &old_file)),
        )
        .map_err(|NoMetadata| no_metadata(paths, &new_file))?;

    paths::write(&old_file, &old_document.render())?;
    paths::write(&new_file, &new_document.render())?;

    // The replacement carries the work now; whether it belongs on the board is
    // the call of whoever ranks it.
    if let Some(board) = board.as_mut()
        && board.remove(&[&old.to_string()])
    {
        priority::write(&paths.rfds, board)?;
    }

    let mut output = Output::from(format!(
        "{}: Superseded by RFD {by}\n{}: Supersedes RFD {old}\n",
        paths.show(&old_file),
        paths.show(&new_file)
    ));
    if let Some(issue) = old_document.header().tracking_issue() {
        output.text.push_str(&format!(
            "Remember to close the superseded tracking issue: {issue}\n"
        ));
    }

    Ok(output)
}

/// A link from the document `from` to the document `to`.
fn link_between(from: &Utf8Path, to: &Utf8Path) -> String {
    let (Some(from_dir), Some(to_dir), Some(name)) = (from.parent(), to.parent(), to.file_name())
    else {
        return to.to_string();
    };

    references::link(from_dir, to_dir, name)
}

fn no_metadata(paths: &Paths, file: &Utf8Path) -> String {
    format!(
        "{} has no metadata block to record the relationship in.",
        paths.show(file)
    )
}

#[cfg(test)]
#[path = "relate_tests.rs"]
mod tests;
