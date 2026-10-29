//! `jp rfd abandon`: set an idea aside, keeping the document and the reason.
//!
//! The RFD opens with RFD 001's notice block, between its metadata and its
//! first section:
//!
//! ```markdown
//! > [!IMPORTANT]
//! > This RFD is **Abandoned**.
//! >
//! > {reason}
//! >
//! > The original text below is preserved for historical context.
//! ```

use super::parse;
use crate::{
    Output,
    document::{Document, NoMetadata},
    id::RfdId,
    paths::{self, Paths},
    priority,
};

/// Statuses past which an RFD can no longer be abandoned.
const FINAL: [&str; 3] = ["Implemented", "Superseded", "Abandoned"];

/// Abandon `id` for `reason`.
pub(crate) fn abandon(paths: &Paths, id: RfdId, reason: &str) -> Result<Output, String> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(format!("Give the reason RFD {id} is abandoned."));
    }

    let file = paths.resolve(id)?;
    let mut document = Document::parse(&paths::read(&file)?);
    let status = document.header().status().unwrap_or_default();
    if FINAL.contains(&status.as_str()) {
        return Err(format!("Cannot abandon from '{status}'."));
    }

    let mut board = priority::read_for_update(&paths.rfds)?;

    document
        .set("Status", "Abandoned")
        .map_err(|NoMetadata| format!("{} has no metadata block.", paths.show(&file)))?;
    document.insert_before_sections(&notice(reason));
    paths::write(&file, &document.render())?;

    if let Some(board) = board.as_mut()
        && board.remove(&[&id.to_string()])
    {
        priority::write(&paths.rfds, board)?;
    }

    let header = document.header();
    let mut output = Output::from(format!(
        "{}: Abandoned ({status} -> Abandoned)\n",
        paths.show(&file)
    ));
    if let Some(issue) = header.tracking_issue() {
        output
            .text
            .push_str(&format!("Remember to close the tracking issue: {issue}\n"));
    }

    // Nothing cascades: what depended on this RFD needs someone to decide
    // what it depends on now.
    let dependents: Vec<String> = header
        .citations("Required by")
        .iter()
        .filter(|cited| parse(cited).is_some())
        .map(|cited| format!("  RFD {cited}"))
        .collect();
    if !dependents.is_empty() {
        output.warnings.push(format!(
            "Warning: the following RFDs depend on this one (Required by):\n{}\nTheir dependency \
             on RFD {id} is now broken; review and update.",
            dependents.join("\n")
        ));
    }

    Ok(output)
}

/// RFD 001's notice block for an abandoned RFD.
fn notice(reason: &str) -> Vec<String> {
    let mut lines = vec![
        "> [!IMPORTANT]".to_owned(),
        "> This RFD is **Abandoned**.".to_owned(),
        ">".to_owned(),
    ];
    lines.extend(reason.lines().map(|line| {
        if line.trim().is_empty() {
            ">".to_owned()
        } else {
            format!("> {line}")
        }
    }));
    lines.push(">".to_owned());
    lines.push("> The original text below is preserved for historical context.".to_owned());

    lines
}

#[cfg(test)]
#[path = "abandon_tests.rs"]
mod tests;
