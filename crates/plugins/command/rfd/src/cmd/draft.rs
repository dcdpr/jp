//! `jp rfd draft`: a new draft from its category's template, or from a ticket.
//!
//! A draft takes the first free slot in the drafts directory, `D05`, and its
//! file is named after its title: `D05-my-feature.md`.
//! The template is the project's own, `000-<category>-template.md` in the RFD
//! directory, which `jp rfd init` writes; its `RFD NNN: TITLE` heading,
//! `AUTHOR`, and `DATE` placeholders are filled in, and its `Category` set.

use std::{fs, io};

use camino::Utf8PathBuf;
use ticket::{TicketId, store};

use super::{alternatives, capitalized};
use crate::{
    Output,
    document::Document,
    id::{self, RfdId},
    paths::{self, Paths},
    references::markdown_files,
};

/// What a new draft is made of.
pub(crate) struct Draft<'a> {
    pub category: &'a str,
    pub title: &'a str,
    pub author: &'a str,

    /// The day the draft is filed, `2026-10-15`.
    pub date: &'a str,
}

/// A ticket a draft is seeded from.
struct Seed<'a> {
    ticket: TicketId,
    description: &'a str,
}

/// The heading placeholder a template carries.
const HEADING: &str = "RFD NNN: TITLE";

/// Write a new draft.
pub(crate) fn draft(
    paths: &Paths,
    categories: &[String],
    draft: &Draft<'_>,
) -> Result<Output, String> {
    let (_, file) = create(paths, categories, draft, None)?;

    Ok(format!("Created {}\n", paths.show(&file)).into())
}

/// Write a new draft seeded from `ticket`'s title and description, then record
/// on the ticket that it moved there and close it.
///
/// A title in `draft` replaces the ticket's.
/// Safe to repeat: a ticket that already names an existing RFD is reported, and
/// a draft an interrupted run seeded is found by the line naming the ticket and
/// linked rather than seeded twice.
pub(crate) fn from_ticket(
    paths: &Paths,
    categories: &[String],
    ticket: TicketId,
    draft: &Draft<'_>,
) -> Result<Output, String> {
    if !paths.tickets.is_dir() {
        return Err(format!(
            "Drafting from a ticket needs tickets, and there is no ticket directory at {}.",
            paths.show(&paths.tickets)
        ));
    }

    let ticket_file =
        store::locate_ticket(&paths.tickets, ticket).map_err(|error| error.to_string())?;
    let parsed = ticket::parse::document(&paths::read(&ticket_file)?)
        .map_err(|error| format!("{ticket} is not a well-formed ticket: {error}"))?;

    let mut output = Output::default();
    if let Some(promoted) = parsed.metadata.promoted_to.as_deref() {
        let existing = promoted
            .parse::<RfdId>()
            .ok()
            .and_then(|id| paths.resolve(id).ok());

        if let Some(existing) = existing {
            output.text = format!(
                "{ticket} is already promoted to {promoted} ({}).\n",
                paths.show(&existing)
            );
            return Ok(output);
        }

        output.warnings.push(format!(
            "{ticket} names {promoted}, but no such RFD exists; seeding a new draft."
        ));
    }

    if let Some((id, seeded)) = find_seeded(paths, ticket)? {
        let promoted = store::promote(&paths.tickets, ticket, &id.to_string())
            .map_err(|error| error.to_string())?;
        output.text = format!(
            "{ticket} already seeded {}; linking it.\n{}: promoted to {id}, closed as Done\n",
            paths.show(&seeded),
            paths.show(&promoted)
        );
        return Ok(output);
    }

    let title = if draft.title.trim().is_empty() {
        parsed.title.as_str()
    } else {
        draft.title
    };
    let seeded = Draft { title, ..*draft };
    let seed = Seed {
        ticket,
        description: &parsed.description,
    };
    let (id, file) = create(paths, categories, &seeded, Some(&seed))?;

    let promoted = store::promote(&paths.tickets, ticket, &id.to_string())
        .map_err(|error| error.to_string())?;
    output.text = format!(
        "Created {}\n{}: promoted to {id}, closed as Done\n",
        paths.show(&file),
        paths.show(&promoted)
    );

    Ok(output)
}

/// Check the request and write the draft, returning its id and path.
fn create(
    paths: &Paths,
    categories: &[String],
    draft: &Draft<'_>,
    seed: Option<&Seed<'_>>,
) -> Result<(RfdId, Utf8PathBuf), String> {
    let category = draft.category.trim().to_lowercase();
    if !categories.contains(&category) {
        return Err(format!(
            "Unknown category '{}'. Use {}.",
            draft.category.trim(),
            alternatives(categories)
        ));
    }

    let title = draft.title.trim();
    super::check_title(title)?;

    let template_path = paths.rfds.join(format!("000-{category}-template.md"));
    let template = match fs::read_to_string(&template_path) {
        Ok(template) => template,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(format!(
                "There is no template for '{category}' at {}. `jp rfd init` writes one for each \
                 configured category.",
                paths.show(&template_path)
            ));
        }
        Err(error) => return Err(format!("{template_path}: {error}")),
    };

    let id = id::next_draft_slot(&paths.rfds)?;
    let mut text = fill(&template, id, title, &category, draft.author, draft.date)
        .map_err(|problem| format!("{}: {problem}", paths.show(&template_path)))?;
    if let Some(seed) = seed {
        text = seed_summary(&text, seed);
    }

    let drafts = paths.drafts();
    fs::create_dir_all(&drafts).map_err(|error| format!("{drafts}: {error}"))?;

    let file = drafts.join(format!("{id}-{}.md", id::slug(title)));
    paths::create(&file, &text)?;

    Ok((id, file))
}

/// The text of a new draft: `template` with its placeholders filled in and its
/// category set.
///
/// Each line's first `AUTHOR` and `DATE` are filled in, but not on the heading
/// line, where the title could carry either word.
fn fill(
    template: &str,
    id: RfdId,
    title: &str,
    category: &str,
    author: &str,
    date: &str,
) -> Result<String, String> {
    let heading = format!("RFD {id}: {title}");
    let mut headed = false;

    let mut text = String::with_capacity(template.len() + title.len());
    for line in template.split_inclusive('\n') {
        let (body, ending) = split_ending(line);

        if let Some(at) = body.find(HEADING) {
            headed = true;
            text.push_str(&body[..at]);
            text.push_str(&heading);
            text.push_str(&body[at + HEADING.len()..]);
        } else {
            text.push_str(&fill_placeholders(body, author, date));
        }
        text.push_str(ending);
    }

    if !headed {
        return Err(format!(
            "the template has no `# {HEADING}` heading to fill in."
        ));
    }

    let mut document = Document::parse(&text);
    document
        .set("Category", &capitalized(category))
        .map_err(|_| "the template has no metadata block.".to_owned())?;

    Ok(document.render())
}

/// `line` with its first `AUTHOR` and first `DATE` replaced.
///
/// Both are found in the template's own text, so a filled-in author is never
/// read as a date placeholder, or the other way round.
fn fill_placeholders(line: &str, author: &str, date: &str) -> String {
    let mut spans: Vec<(usize, usize, &str)> = [("AUTHOR", author), ("DATE", date)]
        .into_iter()
        .filter_map(|(placeholder, value)| {
            line.find(placeholder)
                .map(|at| (at, placeholder.len(), value))
        })
        .collect();
    spans.sort_unstable();

    let mut out = String::with_capacity(line.len());
    let mut from = 0;
    for (at, len, value) in spans {
        // The two placeholders share no letters in a row, so they can't
        // overlap; a span starting inside the last one would be a bug.
        if at < from {
            continue;
        }
        out.push_str(&line[from..at]);
        out.push_str(value);
        from = at + len;
    }
    out.push_str(&line[from..]);

    out
}

/// `line` and the line ending it carries, if any.
fn split_ending(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else {
        (line, "")
    }
}

/// The line a draft seeded from `ticket` carries, which a repeated run finds it
/// by.
fn seeded_marker(ticket: TicketId) -> String {
    format!("Promoted from ticket {ticket}.")
}

/// Replace the prose under the draft's `## Summary` with the ticket's
/// description and the line naming the ticket.
///
/// A template without that section gets one, between its metadata and its first
/// section.
fn seed_summary(text: &str, seed: &Seed<'_>) -> String {
    let mut seeded = vec![String::new()];
    let description = seed.description.trim();
    if !description.is_empty() {
        seeded.extend(description.lines().map(ToOwned::to_owned));
        seeded.push(String::new());
    }
    seeded.push(seeded_marker(seed.ticket));

    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|line| *line == "## Summary") else {
        let mut document = Document::parse(text);
        let mut section = vec!["## Summary".to_owned()];
        section.extend(seeded);
        document.insert_before_sections(&section);
        return document.render();
    };

    let end = lines[start + 1..]
        .iter()
        .position(|line| line.starts_with("## "))
        .map_or(lines.len(), |offset| start + 1 + offset);

    let mut out: Vec<String> = lines[..=start]
        .iter()
        .map(|line| (*line).to_owned())
        .collect();
    out.extend(seeded);
    if end < lines.len() {
        out.push(String::new());
        out.extend(lines[end..].iter().map(|line| (*line).to_owned()));
    }

    let mut rendered = out.join("\n");
    rendered.push('\n');

    rendered
}

/// The draft or RFD an earlier run seeded from `ticket`, by the line naming it.
fn find_seeded(paths: &Paths, ticket: TicketId) -> Result<Option<(RfdId, Utf8PathBuf)>, String> {
    let marker = seeded_marker(ticket);

    for file in markdown_files(&paths.rfds)?
        .into_iter()
        .chain(markdown_files(&paths.drafts())?)
    {
        let Some(id) = file.file_name().and_then(RfdId::of_file) else {
            continue;
        };
        if fs::read_to_string(&file).is_ok_and(|text| text.lines().any(|line| line == marker)) {
            return Ok(Some((id, file)));
        }
    }

    Ok(None)
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
