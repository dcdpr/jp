//! `jp wiki show`: one page's metadata, what it links to and what links to it,
//! and its table of contents; or its body, or one section, with embeds
//! expanded, or as written.
//!
//! What the command may not read is left out: an embed of such a file is a
//! marker naming it, and the pages and documents it couldn't search for
//! references are counted in a warning.

use std::collections::BTreeMap;

use camino::Utf8Path;
use doc::{heading::headings, references::names_stem};
use serde::Serialize;

use crate::{
    Context, Output,
    embed::{Expander, Problem},
    library::Library,
    links::{self, Named, has_scheme, resolve, split_fragment},
    page::{Page, PageId, body_start},
    section,
    store::Wiki,
};

/// A document a page links to, or that links to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Reference {
    /// `W-0a1b2c3`, `RFD 042`, or `T-02wt0kx`.
    pub id: String,
    pub title: String,
}

/// One heading of a page's table of contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Entry {
    level: u8,
    title: String,
    slug: String,
}

/// A page in `jp -F json wiki show` output.
#[derive(Serialize)]
struct Detail<'a> {
    id: PageId,
    title: &'a str,
    path: String,
    status: Option<&'a str>,
    authors: Option<&'a str>,
    date: Option<&'a str>,
    parent: Option<&'a str>,
    superseded_by: Option<&'a str>,
    labels: &'a [String],
    summary: Option<&'a str>,
    references: Vec<Reference>,
    referenced_by: Vec<Reference>,
    contents: Vec<Entry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

/// One section in `jp -F json wiki show --section` output.
#[derive(Serialize)]
struct SectionDetail<'a> {
    id: PageId,
    slug: &'a str,
    title: &'a str,
    text: &'a str,
}

/// Show page `id`: with `body`, its body; with `section`, only that section.
///
/// With `raw`, the body or section is the text as written, its embeds left as
/// the images that name them.
pub(crate) fn show(
    ctx: &Context<'_>,
    id: PageId,
    body: bool,
    wanted: Option<&str>,
    raw: bool,
) -> Result<Output, String> {
    let wiki = ctx.wiki()?;
    let page = wiki.find(id)?;
    let library = ctx.library();
    let mut expander = Expander::new(&ctx.paths.root, &ctx.paths.wiki, &library, &ctx.access);
    let mut output = Output::default();

    if let Some(wanted) = wanted {
        let found = section::find(&page.source, wanted)?;
        let text = section::text(&page.source, &found);
        let text = if raw {
            text
        } else {
            expander.expand(&text, &page.path, Some(&found.heading.slug))
        };
        output.warnings.extend(problems(expander.problems));

        if ctx.format.is_json() {
            output.json = true;
            output.text = serde_json::to_string_pretty(&SectionDetail {
                id,
                slug: &found.heading.slug,
                title: &found.heading.text,
                text: &text,
            })
            .map_err(|error| format!("failed to serialize {id}: {error}"))?;
        } else {
            output.text = format!("{text}\n");
        }
        return Ok(output);
    }

    let references = references(
        page,
        &wiki,
        &library,
        &ctx.paths.wiki,
        &ctx.options.relevant_key,
    );
    let referenced_by = referenced_by(page, &wiki, &library);
    let contents = contents(&page.source);
    let body = body.then(|| {
        if raw {
            page.body()
        } else {
            expander.expand(&page.body(), &page.path, None)
        }
    });
    output.warnings.extend(problems(expander.problems));
    output.warnings.extend(unsearched(ctx, &wiki, &library, id));

    if ctx.format.is_json() {
        output.json = true;
        output.text = serde_json::to_string_pretty(&Detail {
            id,
            title: page.name(),
            path: ctx.paths.show(&page.path),
            status: page.status.as_deref(),
            authors: page.authors.as_deref(),
            date: page.date.as_deref(),
            parent: page.parent.as_deref(),
            superseded_by: page.superseded_by.as_deref(),
            labels: &page.labels,
            summary: page.summary.as_deref(),
            references,
            referenced_by,
            contents,
            body,
        })
        .map_err(|error| format!("failed to serialize {id}: {error}"))?;
        return Ok(output);
    }

    output.text = text(
        ctx,
        page,
        &references,
        &referenced_by,
        &contents,
        body.as_deref(),
    );
    Ok(output)
}

/// What an expansion couldn't do, as warnings.
///
/// An embed the command may not read is left out: the marker standing in its
/// place says so.
fn problems(problems: Vec<Problem>) -> Vec<String> {
    problems
        .into_iter()
        .filter_map(|problem| match problem {
            Problem::Loop(chain) => Some(format!("Embeds loop: {}.", chain.join(" embeds "))),
            Problem::Unresolved(message) => Some(format!("An embed wasn't expanded: {message}")),
            Problem::Denied { .. } => None,
        })
        .collect()
}

/// What couldn't be searched for references to and from page `id`, as warnings:
/// the pages and documents the command may not read, and the directories it may
/// not list.
fn unsearched(ctx: &Context<'_>, wiki: &Wiki, library: &Library, id: PageId) -> Vec<String> {
    let mut warnings = vec![];
    match wiki.hidden.len() + library.unread().count() {
        0 => {}
        1 => warnings.push(format!(
            "1 document can't be read, so a reference it makes to {id} isn't shown."
        )),
        count => warnings.push(format!(
            "{count} documents can't be read, so references they make to {id} aren't shown."
        )),
    }

    if !library.unlisted.is_empty() {
        let dirs: Vec<String> = library
            .unlisted
            .iter()
            .map(|dir| format!("{}/", ctx.paths.show(dir)))
            .collect();
        warnings.push(format!(
            "{} can't be listed, so references to and from the documents there aren't shown.",
            dirs.join(" and ")
        ));
    }

    warnings
}

/// The page as text: its metadata, its references, and its contents or body.
fn text(
    ctx: &Context<'_>,
    page: &Page,
    references: &[Reference],
    referenced_by: &[Reference],
    contents: &[Entry],
    body: Option<&str>,
) -> String {
    let mut out = format!("# {}: {}\n\n", page.id, page.name());
    out.push_str(&format!("- **Path**: {}\n", ctx.paths.show(&page.path)));
    for (key, value) in [
        ("Status", &page.status),
        ("Authors", &page.authors),
        ("Date", &page.date),
        ("Parent", &page.parent),
        ("Superseded by", &page.superseded_by),
    ] {
        if let Some(value) = value {
            out.push_str(&format!("- **{key}**: {value}\n"));
        }
    }
    for label in &page.labels {
        out.push_str(&format!("- **Label**: {label}\n"));
    }
    if let Some(summary) = &page.summary {
        out.push_str(&format!("- **Summary**: {summary}\n"));
    }

    for (heading, list) in [("References", references), ("Referenced by", referenced_by)] {
        if list.is_empty() {
            continue;
        }
        out.push_str(&format!("\n## {heading}\n\n"));
        for reference in list {
            out.push_str(&format!("- {}: {}\n", reference.id, reference.title));
        }
    }

    if let Some(body) = body {
        if !body.is_empty() {
            out.push_str(&format!("\n{body}\n"));
        }
        return out;
    }

    if !contents.is_empty() {
        out.push_str("\n## Contents\n\n");
        let top = contents.iter().map(|entry| entry.level).min().unwrap_or(1);
        for entry in contents {
            let indent = "  ".repeat(usize::from(entry.level - top));
            out.push_str(&format!("{indent}- {} `#{}`\n", entry.title, entry.slug));
        }
    }

    out
}

/// The headings of a page's body, its title left out.
fn contents(source: &str) -> Vec<Entry> {
    let lines: Vec<&str> = source.lines().collect();
    let start = body_start(&lines);

    headings(source)
        .into_iter()
        .filter(|heading| heading.line >= start)
        .map(|heading| Entry {
            level: heading.level,
            title: heading.text,
            slug: heading.slug,
        })
        .collect()
}

/// The documents `page` links to or names: by id, by a link to their file, and
/// RFDs under the relevant label key.
fn references(
    page: &Page,
    wiki: &Wiki,
    library: &Library,
    dir: &Utf8Path,
    relevant_key: &str,
) -> Vec<Reference> {
    let mut found: BTreeMap<(u8, String), String> = BTreeMap::new();
    let mut add_page = |id: PageId| {
        if id != page.id
            && let Some(name) = wiki.name_of(id)
        {
            found.insert((0, id.to_string()), name);
        }
    };

    let mut rfds = vec![];
    let mut tickets = vec![];
    for mention in links::mentions(&page.source) {
        match mention.named {
            Named::Wiki => {
                if let Ok(id) = mention.id.parse() {
                    add_page(id);
                }
            }
            Named::Rfd => rfds.push(mention.id),
            Named::Ticket => tickets.push(mention.id),
        }
    }

    let from = page.path.parent().unwrap_or(dir);
    for link in links::links(&page.source) {
        let (path, _) = split_fragment(&link.url);
        if path.is_empty() || has_scheme(&link.url) || link.url.starts_with('/') {
            continue;
        }
        let target = resolve(from, path);
        if let Some(name) = library.name_of(&target, dir) {
            match name.strip_prefix("RFD ") {
                Some(rfd) => rfds.push(rfd.to_owned()),
                None if name.starts_with("W-") => {
                    if let Ok(id) = name.parse() {
                        add_page(id);
                    }
                }
                None => tickets.push(name),
            }
        }
    }
    for (key, values) in &page.label_set() {
        if key.eq_ignore_ascii_case(relevant_key) {
            rfds.extend(values.iter().map(|value| format!("{value:0>3}")));
        }
    }

    for id in rfds {
        if let Some(rfd) = library.rfds.get(&id) {
            found.insert((1, format!("RFD {id}")), rfd.title.clone());
        }
    }
    for id in tickets {
        if let Some(ticket) = library.tickets.get(&id) {
            found.insert((2, id), ticket.title.clone());
        }
    }

    found
        .into_iter()
        .map(|((_, id), title)| Reference { id, title })
        .collect()
}

/// The pages, RFDs, and tickets that name `page` by id or link to its file.
fn referenced_by(page: &Page, wiki: &Wiki, library: &Library) -> Vec<Reference> {
    let id = page.id.to_string();
    let stem = page.path.file_stem().unwrap_or_default();
    let names = |source: &str| links::names(source, &id) || names_stem(source, stem);

    let mut found = vec![];
    for other in &wiki.pages {
        if other.id != page.id && names(&other.source) {
            found.push(Reference {
                id: other.id.to_string(),
                title: other.name().to_owned(),
            });
        }
    }
    for (rfd, document) in &library.rfds {
        if document.source.as_deref().is_some_and(names) {
            found.push(Reference {
                id: format!("RFD {rfd}"),
                title: document.title.clone(),
            });
        }
    }
    for (ticket, document) in &library.tickets {
        if document.source.as_deref().is_some_and(names) {
            found.push(Reference {
                id: ticket.clone(),
                title: document.title.clone(),
            });
        }
    }

    found
}

#[cfg(test)]
#[path = "show_tests.rs"]
mod tests;
