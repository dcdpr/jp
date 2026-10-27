//! `jp rfd track`, `start`, and `stop`: an RFD's tracking ticket.
//!
//! The tracking ticket stands for an RFD's implementation as a whole: one
//! ticket carrying `Implements: <id>` and the label `type=tracking`, holding
//! the RFD's Implementation Plan as its description.
//! A phase that needs a card of its own is filed separately and names the RFD
//! without the label.
//! An RFD is in development while its tracking ticket is In Progress.
//!
//! `track` files the ticket, or finds the one already filed.
//! The plan is read out of the document by a model, since the phases differ per
//! RFD; when it isn't read, the marker [`PENDING`] stands in for it and the
//! next `track` reads it again.
//! Only the marker is ever replaced: the rest of the description is the
//! project's to edit.

use camino::{Utf8Path, Utf8PathBuf};
use jp_plugin::message::QueryRequest;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use ticket::{Labels, NewTicket, Status, Ticket, TicketId, Vocabulary, parse, store};

use crate::{
    Output,
    board::{self, TRACKING_LABEL},
    document::{self, Document},
    id::RfdId,
    options::TICKET_LABELS_KEY,
    paths::{self, Paths},
    references,
};

/// The marker standing in for a plan that hasn't been read yet.
pub(crate) const PENDING: &str = "<!-- rfd-plan: pending -->";

/// How long the conversation a plan is read in is kept.
const PLAN_LIFETIME: &str = "5m";

/// What the plan is read under: a conversation kept out of the workspace tree,
/// no reasoning, and no tool calls, which a turn run for a plugin would make
/// without asking.
const PLAN_CFG: [&str; 3] = [
    "conversation.start_local=true",
    "assistant.model.parameters.reasoning=off",
    "assistant.tool_choice=none",
];

/// What the model is asked, ahead of the document.
const PLAN_PROMPT: &str = "Read the RFD below and list the phases of its Implementation Plan, in \
                           order. Give each a short imperative title and a summary of what it \
                           delivers. Return an empty array if the RFD has no Implementation Plan.";

/// One phase of an RFD's Implementation Plan, as the tracking ticket lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Phase {
    pub title: String,
    pub summary: String,
}

/// What reading an RFD's Implementation Plan came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    /// The phases, in order; empty when the RFD has no Implementation Plan.
    Phases(Vec<Phase>),

    /// The plan couldn't be read, for the reason given.
    Unavailable(String),

    /// The plan wasn't read, on purpose: a later `track` reads it.
    Deferred,
}

/// The RFD a plan is read from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlanRequest<'a> {
    pub id: RfdId,

    /// Its file, relative to the workspace root.
    pub file: &'a str,

    pub document: &'a str,
}

/// Reads a plan, or fails when the command can't go on, such as when the user
/// interrupts it.
pub(crate) type ReadPlan<'a> = dyn FnMut(&PlanRequest<'_>) -> Result<Plan, String> + 'a;

/// Who files a tracking ticket, and when.
#[derive(Debug, Clone)]
pub(crate) struct Filer {
    pub author: String,

    /// The day the ticket is filed, `2026-10-15`.
    pub date: String,
}

/// An RFD's tracking ticket, and what [`track`] did about it.
#[derive(Debug)]
pub(crate) struct Tracked {
    pub ticket: TicketId,

    /// What was done, as notes, and what is left to check, as warnings.
    pub report: Output,
}

/// File RFD `id`'s tracking ticket, or find the one already filed and read its
/// plan if it is still pending.
///
/// Only an Accepted or Implemented RFD carries one: implementation begins at
/// Accepted.
/// `vocabulary` is the ticket board's, which has to define the tracking label.
pub(crate) fn track(
    paths: &Paths,
    vocabulary: &Vocabulary,
    id: RfdId,
    filer: &Filer,
    read_plan: &mut ReadPlan<'_>,
) -> Result<Tracked, String> {
    needs_tickets(paths)?;

    let file = paths.resolve(id)?;
    let text = paths::read(&file)?;
    let status = Document::parse(&text).header().status().unwrap_or_default();
    if !matches!(status.as_str(), "Accepted" | "Implemented") {
        return Err(format!(
            "Cannot track: {} is '{status}'.\nOnly Accepted or Implemented RFDs carry a tracking \
             ticket.",
            file.file_name().unwrap_or_default()
        ));
    }

    let labels = tracking_labels(vocabulary)?;
    let entries = store::list(&paths.tickets).map_err(|error| error.to_string())?;

    let mut report = Output::default();
    let shown = paths.show(&file);
    let request = PlanRequest {
        id,
        file: &shown,
        document: &text,
    };
    let link = references::link(
        &paths.tickets,
        file.parent().unwrap_or(&paths.rfds),
        file.file_name().unwrap_or_default(),
    );

    if let Some((ticket, path, existing)) = the_one(paths, id, &entries, &mut report) {
        if !existing.description.contains(PENDING) {
            report.notes.push(format!(
                "{} ({ticket}) already tracks RFD {id}.",
                paths.show(path)
            ));
        } else if let Some(phases) = phases(id, read_plan(&request)?, &mut report) {
            fill(paths, id, ticket, &phases, &mut report)?;
        }

        return Ok(Tracked { ticket, report });
    }

    let title = document::title(&text)
        .ok_or_else(|| format!("No heading found in {shown}; cannot title the ticket."))?;
    let plan = phases(id, read_plan(&request)?, &mut report);

    let (ticket, path) = store::create(&paths.tickets, vocabulary, &NewTicket {
        title: &title,
        authors: &filer.author,
        date: &filer.date,
        implements: Some(&id.to_string()),
        labels: &labels,
        description: &description(id, &link, plan.as_deref()),
    })
    .map_err(|error| error.to_string())?;
    report.notes.push(format!(
        "Filed {} ({ticket}), tracking RFD {id}.",
        paths.show(&path)
    ));

    // A tracking ticket that couldn't be read wasn't found either, so the
    // one filed now may duplicate it.
    let unreadable: Vec<Utf8PathBuf> = entries
        .iter()
        .filter(|entry| entry.ticket.is_err())
        .map(|entry| entry.path.clone())
        .collect();
    if !unreadable.is_empty() {
        report.warnings.push(super::listed(
            &format!(
                "Warning: these tickets could not be read; if one of them tracks RFD {id}, \
                 {ticket} duplicates it:"
            ),
            paths,
            &unreadable,
        ));
    }

    Ok(Tracked { ticket, report })
}

/// Mark RFD `id` as in development: file its tracking ticket if it has none,
/// then move that ticket to In Progress.
pub(crate) fn start(
    paths: &Paths,
    vocabulary: &Vocabulary,
    id: RfdId,
    filer: &Filer,
    read_plan: &mut ReadPlan<'_>,
) -> Result<Output, String> {
    let tracked = track(paths, vocabulary, id, filer, read_plan)?;
    let (path, previous) = store::set_status(&paths.tickets, tracked.ticket, Status::InProgress)
        .map_err(|error| error.to_string())?;

    let mut output = tracked.report;
    output.text = moved(paths, &path, previous, Status::InProgress);

    Ok(output)
}

/// Mark RFD `id` as no longer in development: move its tracking ticket back to
/// Todo.
///
/// Allowed at any status, unlike [`start`]: a mark left behind by an RFD moving
/// back to Discussion still has to be clearable.
/// An RFD without a tracking ticket is already not in development, so nothing
/// is filed.
pub(crate) fn stop(paths: &Paths, id: RfdId) -> Result<Output, String> {
    needs_tickets(paths)?;
    paths.resolve(id)?;

    let entries = store::list(&paths.tickets).map_err(|error| error.to_string())?;
    let mut output = Output::default();
    let Some((ticket, ..)) = the_one(paths, id, &entries, &mut output) else {
        output.text = format!("RFD {id} has no tracking ticket; nothing to stop.\n");
        return Ok(output);
    };

    let (path, previous) = store::set_status(&paths.tickets, ticket, Status::Todo)
        .map_err(|error| error.to_string())?;
    output.text = moved(paths, &path, previous, Status::Todo);

    Ok(output)
}

/// Refuse a project without a ticket directory, which has nowhere to track an
/// RFD.
fn needs_tickets(paths: &Paths) -> Result<(), String> {
    if paths.tickets.is_dir() {
        return Ok(());
    }

    Err(format!(
        "Tracking needs tickets, and there is no ticket directory at {}.",
        paths.show(&paths.tickets)
    ))
}

/// The labels a tracking ticket is filed with, as the ticket vocabulary spells
/// them, or a refusal naming what to change.
///
/// A board that defines no labels takes any, the tracking label among them.
fn tracking_labels(vocabulary: &Vocabulary) -> Result<Labels, String> {
    let (key, value) = TRACKING_LABEL;
    let entry = format!("{TICKET_LABELS_KEY}.{key}");

    vocabulary
        .resolve(&[format!("{key}={value}")])
        .map_err(|rejected| {
            if !rejected.retired.is_empty() {
                format!(
                    "Tracking label retired. Move `\"{value}\"` from `{entry}.retired` to \
                     `{entry}.values`."
                )
            } else if !rejected.unknown_keys.is_empty() || !rejected.unknown_values.is_empty() {
                format!("Tracking label missing. Add `\"{value}\"` to `{entry}.values`.")
            } else {
                // The label resolved, and the board's bounds ask for more than
                // the one label a tracking ticket is filed with.
                format!(
                    "A tracking ticket is filed with only `{key}={value}`, and this board needs \
                     more. {rejected}"
                )
            }
        })
}

/// RFD `id`'s tracking ticket among `entries`, the first by id when there are
/// several, which is reported.
fn the_one<'a>(
    paths: &Paths,
    id: RfdId,
    entries: &'a [store::Entry],
    report: &mut Output,
) -> Option<(TicketId, &'a Utf8PathBuf, &'a Ticket)> {
    let found: Vec<(TicketId, &Utf8PathBuf, &Ticket)> = entries
        .iter()
        .filter_map(|entry| {
            let ticket = entry.ticket.as_ref().ok()?;
            (board::tracked(ticket) == Some(id)).then_some((entry.id, &entry.path, ticket))
        })
        .collect();

    if let [(first, ..), _, ..] = found.as_slice() {
        let all: Vec<Utf8PathBuf> = found.iter().map(|(_, path, _)| (*path).clone()).collect();
        report.warnings.push(super::listed(
            &format!("Warning: RFD {id} has more than one tracking ticket; using {first}:"),
            paths,
            &all,
        ));
    }

    found.into_iter().next()
}

/// Put `phases` in place of the marker on `ticket`.
///
/// The ticket is read again first: reading the plan can take a minute, and in
/// that time the description may have been edited, or filled by another run.
/// One that no longer carries the marker is left alone.
fn fill(
    paths: &Paths,
    id: RfdId,
    ticket: TicketId,
    phases: &[Phase],
    report: &mut Output,
) -> Result<(), String> {
    let path = store::locate_ticket(&paths.tickets, ticket).map_err(|error| error.to_string())?;
    let current = parse::document(&paths::read(&path)?)
        .map_err(|error| format!("{}: {error}", paths.show(&path)))?;

    if !current.description.contains(PENDING) {
        report.notes.push(format!(
            "{} ({ticket}) already holds the Implementation Plan of RFD {id}.",
            paths.show(&path)
        ));
        return Ok(());
    }

    let description = current
        .description
        .replacen(PENDING, &plan_section(phases), 1);
    let edited = store::edit(&paths.tickets, ticket, None, Some(&description))
        .map_err(|error| error.to_string())?;
    report.notes.push(format!(
        "Read the Implementation Plan of RFD {id} into {} ({ticket}).",
        paths.show(&edited.to)
    ));

    Ok(())
}

/// The phases a tracking ticket lists, or `None` when the plan stays pending,
/// which is reported.
fn phases(id: RfdId, plan: Plan, report: &mut Output) -> Option<Vec<Phase>> {
    let reason = match plan {
        Plan::Phases(phases) if !phases.is_empty() => return Some(phases),
        Plan::Phases(_) => format!("No Implementation Plan was found in RFD {id}."),
        Plan::Unavailable(reason) => {
            format!("The Implementation Plan of RFD {id} could not be read: {reason}.")
        }
        Plan::Deferred => {
            report.notes.push(format!(
                "The tracking ticket holds `{PENDING}` in place of the Implementation Plan of RFD \
                 {id} until `jp rfd track {id}` reads it."
            ));
            return None;
        }
    };

    report.warnings.push(format!(
        "{reason} The tracking ticket holds `{PENDING}` in its place until `jp rfd track {id}` \
         reads it."
    ));

    None
}

/// A tracking ticket's description: a link to its RFD, then the plan, or the
/// marker standing in for it.
fn description(id: RfdId, link: &str, phases: Option<&[Phase]>) -> String {
    let mut text = format!("Tracking ticket for [RFD {id}]({link}).\n\n");

    match phases {
        Some(phases) => text.push_str(&plan_section(phases)),
        None => text.push_str(PENDING),
    }

    text.trim_end().to_owned()
}

/// The plan as a tracking ticket lists it: a heading, then one item per phase.
fn plan_section(phases: &[Phase]) -> String {
    let mut text = String::from("## Implementation plan\n\n");
    for phase in phases {
        text.push_str(&format!(
            "- **{}**: {}\n",
            one_line(&phase.title),
            one_line(&phase.summary)
        ));
    }

    text.trim_end().to_owned()
}

/// `text` on one line, so a phase stays one item of the list.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `path: <previous> -> <status>`, or that it already was.
fn moved(paths: &Paths, path: &Utf8Path, previous: Status, status: Status) -> String {
    if previous == status {
        format!("{}: already {status}\n", paths.show(path))
    } else {
        format!("{}: {previous} -> {status}\n", paths.show(path))
    }
}

/// The query that reads the plan of the RFD `request` names.
///
/// The document goes in the request rather than as an attachment, so the plan
/// reads the text as it is on disk now, without an attachment handler.
pub(crate) fn plan_query(request: &PlanRequest<'_>) -> QueryRequest {
    QueryRequest {
        id: Some("plan".to_owned()),
        conversation: String::new(),
        content: format!(
            "{PLAN_PROMPT}\n\n<document path=\"{}\">\n{}\n</document>\n",
            request.file,
            request.document.trim_end()
        ),
        new: true,
        title: Some(format!("Implementation Plan of RFD {}", request.id)),
        cfg: PLAN_CFG.map(ToOwned::to_owned).to_vec(),
        schema: Some(plan_schema()),
        expires_in: Some(PLAN_LIFETIME.to_owned()),
    }
}

/// The shape the plan comes back in.
fn plan_schema() -> Map<String, Value> {
    let schema = json!({
        "type": "object",
        "properties": {
            "phases": {
                "type": "array",
                "description": "One entry per phase of the Implementation Plan, in order",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Imperative title, at most 60 characters"
                        },
                        "summary": {
                            "type": "string",
                            "description": "What the phase delivers, one to three sentences of markdown"
                        }
                    },
                    "required": ["title", "summary"]
                }
            }
        },
        "required": ["phases"]
    });

    schema.as_object().cloned().unwrap_or_default()
}

/// The plan in the structured response a plan query finished with.
pub(crate) fn plan_from(data: Option<Value>) -> Plan {
    #[derive(Deserialize)]
    struct Response {
        phases: Vec<Phase>,
    }

    let Some(data) = data else {
        return Plan::Unavailable("the response carried no structured data".to_owned());
    };

    match serde_json::from_value::<Response>(data) {
        Ok(response) => Plan::Phases(response.phases),
        Err(error) => Plan::Unavailable(format!("the response didn't match the schema: {error}")),
    }
}

#[cfg(test)]
#[path = "track_tests.rs"]
mod tests;
