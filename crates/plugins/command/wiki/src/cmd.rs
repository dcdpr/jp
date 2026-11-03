//! The commands that list, create, rank, archive, pin, and check pages.
//!
//! `show` and `edit` live in modules of their own.
//! Each command checks what it can before it writes, so a refusal leaves the
//! wiki as it was, and reports what it changed as an [`Output`].
//! `list` and `add` also answer the `wiki_list` and `wiki_create` tools, under
//! the call's access.

use std::collections::BTreeSet;

use chrono::Local;
use doc::{epoch::EPOCH_FILE, files, metadata};
use jp_label::{Labels, Selector, Vocabulary};
use jp_tool::Capability;
use serde::Serialize;
use ticket::TicketId;

use crate::{
    Context, Output,
    check::{self, Report},
    library::{Library, Lookup},
    order::{self, Place, place_of, siblings},
    page::{self, NewPage, Page, PageId, Status, WikiKind},
    pin::{self, Pinned},
    store,
    store::Wiki,
};

/// Today, as a new page's `Date` reads.
pub(crate) fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// `title`, trimmed, when it can head a page.
pub(crate) fn check_title(title: &str) -> Result<&str, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("Give the page a title.".to_owned());
    }
    if title.contains(['\n', '\r']) {
        return Err("A title is one line.".to_owned());
    }

    Ok(title)
}

/// Create the wiki directory and the epoch file its ids count time from.
pub(crate) fn init(ctx: &Context<'_>) -> Result<Output, String> {
    let dir = &ctx.paths.wiki;
    let existed = dir.is_dir();
    files::establish_epoch::<WikiKind>(dir).map_err(|error| error.to_string())?;

    Ok(if existed {
        format!("{} already exists.\n", ctx.paths.show(dir))
    } else {
        format!("Created {}\n", ctx.paths.show(dir))
    }
    .into())
}

/// What a new page is made of.
pub(crate) struct Add<'a> {
    pub title: &'a str,
    pub status: Option<Status>,
    pub summary: Option<&'a str>,
    pub body: Option<&'a str>,
    pub parent: Option<PageId>,
    pub labels: &'a [String],

    /// Who the page is attributed to.
    pub author: &'a str,
}

/// Create a page.
///
/// A Snapshot is pinned as it is created, so the file is written once, already
/// citing source by permalink.
pub(crate) fn add(ctx: &Context<'_>, add: &Add<'_>) -> Result<Output, String> {
    let title = check_title(add.title)?;
    let status = add.status.unwrap_or(Status::Reference);
    if status == Status::Archived {
        return Err("A page starts as a Reference or a Snapshot; archive it later.".to_owned());
    }

    let wiki = ctx.wiki()?;
    if let Some(parent) = add.parent {
        active_parent(&wiki, parent)?;
    }
    let vocabulary = ctx.options.vocabulary()?;
    let labels = vocabulary
        .resolve(add.labels)
        .map_err(|error| error.to_string())?;
    unique(&vocabulary, &labels, &wiki, None)?;

    let summary = add.summary.map(str::trim).unwrap_or_default();
    if summary.contains(['\n', '\r']) {
        return Err("A summary is one sentence on one line.".to_owned());
    }
    let text = page::render(&NewPage {
        title,
        status,
        authors: add.author,
        date: &today(),
        parent: add.parent,
        labels: &labels,
        summary,
        body: add.body.unwrap_or_default(),
    });

    let (_, output) = create(ctx, title, &text, status, summary)?;
    Ok(output)
}

/// Write a new page holding `text` into the wiki directory, pinning a Snapshot
/// first, and report it, with a note when it has no summary yet.
fn create(
    ctx: &Context<'_>,
    title: &str,
    text: &str,
    status: Status,
    summary: &str,
) -> Result<(PageId, Output), String> {
    // A new id counts time from the epoch file.
    let dir = &ctx.paths.wiki;
    let refused = |refusal| {
        format!(
            "A page can't be created in {}: {refusal}.",
            ctx.paths.show(dir)
        )
    };
    ctx.access
        .check(Capability::Read, &dir.join(EPOCH_FILE))
        .map_err(refused)?;
    ctx.access.check(Capability::Create, dir).map_err(refused)?;

    // Pinned before the file is written, so a Snapshot never exists citing
    // source by relative links. A pin reads only the page's directory from its
    // path, and the id naming the file is drawn as the file is created.
    let pinned = if status == Status::Snapshot {
        let repository = ctx.repository()?;
        let provisional = dir.join("new.md");
        Some(pin::pin(
            text,
            &provisional,
            &ctx.paths,
            repository.as_deref(),
            &ctx.access,
        ))
    } else {
        None
    };
    let text = pinned.as_ref().map_or(text, |(text, _)| text.as_str());

    let (id, path) = store::create(dir, title, text)?;
    let mut output = Output::from(format!("Created {} ({id})\n", ctx.paths.show(&path)));
    if let Some((_, report)) = pinned {
        output.extend(pin_report(report));
    }
    if summary.trim().is_empty() {
        output.notes.push(format!(
            "{id} has no summary yet, which `jp wiki check` reports; give it one with `jp wiki \
             edit {id} --summary \"...\"`."
        ));
    }

    Ok((id, output))
}

/// What a pin did, as the lines a command reports: each link pinned, and each
/// left as it was, with why.
pub(crate) fn pin_report(report: Pinned) -> Output {
    let mut output = Output::default();
    for (old, new) in report.pinned {
        output
            .text
            .push_str(&format!("  pinned {old}\n      to {new}\n"));
    }
    for (url, why) in report.left {
        output
            .warnings
            .push(format!("{url} was left unpinned: {why}."));
    }

    output
}

/// The page `id` names, when it can be a parent: there, and not archived.
pub(crate) fn active_parent(wiki: &Wiki, id: PageId) -> Result<&Page, String> {
    let parent = wiki.find(id)?;
    if parent.is_archived() {
        return Err(format!(
            "{id} is archived; a page's parent is an active page."
        ));
    }

    Ok(parent)
}

/// Refuse `labels` when another page than `except` carries one the vocabulary
/// lets one page carry.
///
/// A page the command can't read might carry it too, so `labels` carrying such
/// a key is refused while one is hidden.
pub(crate) fn unique(
    vocabulary: &Vocabulary,
    labels: &Labels,
    wiki: &Wiki,
    except: Option<PageId>,
) -> Result<(), String> {
    let taken: Vec<String> = wiki
        .pages
        .iter()
        .filter(|page| Some(page.id) != except)
        .flat_map(|page| {
            vocabulary
                .clashes(labels, &page.label_set())
                .into_iter()
                .map(move |clash| format!("`{clash}` is already on {}", page.id))
        })
        .collect();
    if !taken.is_empty() {
        return Err(format!(
            "{}, and the vocabulary lets one page carry it.",
            taken.join("; ")
        ));
    }

    let held: Vec<&str> = vocabulary
        .facets()
        .filter(|(key, facet)| {
            facet.unique().is_some() && labels.keys().any(|label| label.eq_ignore_ascii_case(key))
        })
        .map(|(key, _)| key)
        .collect();
    let hidden: Vec<String> = wiki
        .hidden
        .iter()
        .filter(|page| Some(page.id) != except)
        .map(|page| page.id.to_string())
        .collect();
    if held.is_empty() || hidden.is_empty() {
        return Ok(());
    }

    Err(format!(
        "The vocabulary lets one page carry `{}`, and these pages can't be read to see that none \
         does: {}.",
        held.join("`, `"),
        hidden.join(", ")
    ))
}

/// What `list` narrows the pages to.
pub(crate) struct Filter<'a> {
    /// Only the pages below this one.
    pub under: Option<PageId>,
    pub status: Option<Status>,
    pub labels: &'a [String],
    pub query: Option<&'a str>,

    /// Only the archived pages.
    pub archived: bool,

    /// The archived pages too, after the rest.
    pub all: bool,

    /// Each page's summary too.
    pub full: bool,
}

/// One page in `jp -F json wiki list` output.
#[derive(Serialize)]
struct Row<'a> {
    id: PageId,
    title: &'a str,
    status: Option<&'a str>,
    parent: Option<&'a str>,
    labels: &'a [String],
    summary: Option<&'a str>,
    path: String,

    /// How far below the top the page sits, or below the page `--under` names.
    depth: usize,
}

/// List pages in reading order: each top page, then down its children.
pub(crate) fn list(ctx: &Context<'_>, filter: &Filter<'_>) -> Result<Output, String> {
    let wiki = ctx.wiki()?;
    let selectors = Selector::parse_all(filter.labels).map_err(|error| error.to_string())?;
    if let Some(under) = filter.under {
        wiki.find(under)?;
    }
    let query = filter
        .query
        .map(str::trim)
        .filter(|query| !query.is_empty());

    let mut archived: Vec<&Page> = wiki
        .pages
        .iter()
        .filter(|page| page.is_archived())
        .collect();
    archived.sort_by_key(|page| page.id);
    let rows: Vec<(usize, &Page)> = if filter.archived {
        archived.into_iter().map(|page| (0, page)).collect()
    } else {
        let mut rows = order::layout(&wiki.pages, &wiki.order);
        if filter.all {
            rows.extend(archived.into_iter().map(|page| (0, page)));
        }
        rows
    };
    let rows = match filter.under {
        Some(under) => below(rows, under),
        None => rows,
    };

    let rows: Vec<(usize, &Page)> = rows
        .into_iter()
        .filter(|(_, page)| {
            filter
                .status
                .is_none_or(|status| page.status() == Some(status))
                && page.label_set().matches(&selectors)
                && query.is_none_or(|query| page.mentions(query))
        })
        .collect();

    if ctx.format.is_json() {
        let json: Vec<Row<'_>> = rows
            .iter()
            .map(|(depth, page)| Row {
                id: page.id,
                title: page.name(),
                status: page.status.as_deref(),
                parent: page.parent.as_deref(),
                labels: &page.labels,
                summary: page.summary.as_deref(),
                path: ctx.paths.show(&page.path),
                depth: *depth,
            })
            .collect();
        let text = serde_json::to_string_pretty(&json)
            .map_err(|error| format!("failed to serialize the pages: {error}"))?;

        return Ok(Output {
            text: format!("{text}\n"),
            json: true,
            ..Output::default()
        });
    }

    let mut output = Output::default();
    if !wiki.hidden.is_empty() {
        let hidden: Vec<String> = wiki.hidden.iter().map(|page| page.id.to_string()).collect();
        output.warnings.push(format!(
            "These pages can't be read, so they aren't listed: {}.",
            hidden.join(", ")
        ));
    }

    for (depth, page) in rows {
        let labels = if page.labels.is_empty() {
            String::new()
        } else {
            format!(" [{}]", page.labels.join(", "))
        };
        output.text.push_str(&format!(
            "{}{}  {:<9}  {}{labels}\n",
            "  ".repeat(depth),
            page.id,
            page.status.as_deref().unwrap_or("-"),
            page.name(),
        ));
        if filter.full
            && let Some(summary) = &page.summary
        {
            output
                .text
                .push_str(&format!("{}    {summary}\n", "  ".repeat(depth)));
        }
    }

    Ok(output)
}

/// The rows laid out below the page `under`, which `rows` holds in reading
/// order, with their depth counted from it.
///
/// A page's descendants follow it in reading order until a row at its depth or
/// above; `under` itself, archived or missing from `rows`, has none.
fn below(rows: Vec<(usize, &Page)>, under: PageId) -> Vec<(usize, &Page)> {
    let mut rows = rows.into_iter();
    let Some((depth, _)) = rows.by_ref().find(|(_, page)| page.id == under) else {
        return vec![];
    };

    rows.take_while(|(row, _)| *row > depth)
        .map(|(row, page)| (row - depth - 1, page))
        .collect()
}

/// Move `id` directly above or below `other`, among the pages sharing its
/// parent, or among the top pages when it has none.
pub(crate) fn rank(
    ctx: &Context<'_>,
    id: PageId,
    before: bool,
    other: PageId,
) -> Result<Output, String> {
    let mut wiki = ctx.wiki()?;
    let place = rank_place(&wiki, id, None, other)?;

    let order_file = ctx.paths.wiki.join(order::ORDER_FILE);
    ctx.access
        .check(Capability::Update, &order_file)
        .map_err(|refusal| format!("The order is unchanged: {refusal}."))?;

    let siblings = siblings(place, &wiki.pages);
    wiki.order.rank(place, &siblings, id, before, other);
    wiki.order.write(&ctx.paths.wiki)?;

    let relation = if before { "above" } else { "below" };
    Ok(format!("{id} now sits {relation} {other}.\n").into())
}

/// The sibling list `id` and `other` share, where `id` can be ranked against
/// `other`: both active, and in the same list.
///
/// `parent`, when given, is where `id` is about to move, as an edit's
/// `Changes::parent` says it: under a page, or with `None` to the top.
pub(crate) fn rank_place(
    wiki: &Wiki,
    id: PageId,
    parent: Option<Option<PageId>>,
    other: PageId,
) -> Result<Place, String> {
    if id == other {
        return Err(format!("{id} can't be ranked against itself."));
    }

    let page = wiki.find(id)?;
    let next_to = wiki.find(other)?;
    for page in [page, next_to] {
        if page.is_archived() {
            return Err(format!(
                "{} is archived, and only active pages are ranked.",
                page.id
            ));
        }
    }

    let place = match parent {
        Some(Some(parent)) => Place::Children(parent),
        Some(None) => Place::Top,
        None => place_of(page, &wiki.pages),
    };
    if place != place_of(next_to, &wiki.pages) {
        return Err(format!(
            "{id} and {other} aren't siblings; a page is ranked among the pages sharing its \
             parent, or among the top pages when it has none."
        ));
    }

    Ok(place)
}

/// Retire a page, keeping it so links to it still resolve.
///
/// A page with active children is refused, so no page is left under a parent
/// the sidebar hides; its entries leave the order file.
pub(crate) fn archive(
    ctx: &Context<'_>,
    id: PageId,
    superseded_by: Option<&str>,
) -> Result<Output, String> {
    let mut wiki = ctx.wiki()?;
    let page = wiki.find(id)?.clone();
    refuse_active_children(&wiki, id)?;

    let target = superseded_by
        .map(|given| superseded_target(&wiki, &ctx.library(), id, given))
        .transpose()?;

    let mut document = metadata::Document::parse(&page.source);
    let no_block = |_| format!("{} has no metadata block.", ctx.paths.show(&page.path));
    document
        .set("Status", &Status::Archived.to_string())
        .map_err(no_block)?;
    if let Some(target) = &target {
        document.set("Superseded by", target).map_err(no_block)?;
    }
    store::write(&page.path, &document.render())?;

    if wiki.order.remove(id) {
        wiki.order.write(&ctx.paths.wiki)?;
    }

    let before = page.status.as_deref().unwrap_or("no status");
    Ok(format!("{}: {before} -> Archived\n", ctx.paths.show(&page.path)).into())
}

/// Refuse to archive `id` while it has active children, naming them.
pub(crate) fn refuse_active_children(wiki: &Wiki, id: PageId) -> Result<(), String> {
    let children: Vec<String> = wiki
        .active_children(id)
        .iter()
        .map(|child| child.id.to_string())
        .collect();

    if children.is_empty() {
        return Ok(());
    }

    Err(format!(
        "{id} has active children: {}. Archive them, or give them another parent, first.",
        children.join(", ")
    ))
}

/// What `given` names as the page or RFD that replaced `id`, written the way
/// `Superseded by` writes it: `W-0a1b2c3` or `RFD 042`.
pub(crate) fn superseded_target(
    wiki: &Wiki,
    library: &Library,
    id: PageId,
    given: &str,
) -> Result<String, String> {
    if let Ok(page) = given.parse::<PageId>() {
        if page == id {
            return Err(format!("{id} can't be superseded by itself."));
        }
        wiki.find(page)?;
        return Ok(page.to_string());
    }

    let bare = given.trim();
    let bare = bare
        .get(..3)
        .filter(|prefix| prefix.eq_ignore_ascii_case("rfd"))
        .map_or(bare, |_| bare[3..].trim_start());
    let rfd = if bare.bytes().all(|b| b.is_ascii_digit()) && !bare.is_empty() && bare.len() <= 3 {
        format!("{bare:0>3}")
    } else {
        bare.to_ascii_uppercase()
    };
    match library.rfd(&rfd) {
        Lookup::Found(_) => Ok(format!("RFD {rfd}")),
        Lookup::Unknown => Err(format!(
            "RFD {rfd} can't be looked up: the access policy doesn't allow listing its directory."
        )),
        Lookup::Missing => Err(format!(
            "`{given}` names neither a wiki page nor an RFD; give `W-0a1b2c3` or `RFD 042`."
        )),
    }
}

/// Pin the relative source links of page `id` to permalinks.
pub(crate) fn pin_page(ctx: &Context<'_>, id: PageId) -> Result<Output, String> {
    let wiki = ctx.wiki()?;
    let page = wiki.find(id)?;
    let repository = ctx.repository()?;

    let (text, report) = pin::pin(
        &page.source,
        &page.path,
        &ctx.paths,
        repository.as_deref(),
        &ctx.access,
    );
    if text != page.source {
        ctx.access.check(Capability::Update, &page.path)?;
        store::write(&page.path, &text)?;
    }

    let shown = ctx.paths.show(&page.path);
    let count = report.pinned.len();
    let mut output = pin_report(report);
    output.text.push_str(&match count {
        0 => format!("{shown}: nothing to pin.\n"),
        1 => format!("{shown}: pinned 1 link.\n"),
        count => format!("{shown}: pinned {count} links.\n"),
    });

    Ok(output)
}

/// What a page moved from a ticket is made of.
pub(crate) struct FromTicket<'a> {
    pub ticket: TicketId,
    pub parent: Option<PageId>,
    pub status: Option<Status>,
    pub summary: Option<&'a str>,

    /// Labels to add to the ones carried over.
    pub labels: &'a [String],
}

/// The line a page moved from `ticket` ends with, which a repeated run finds it
/// by.
fn moved_marker(ticket: TicketId) -> String {
    format!("Moved from ticket {ticket}.")
}

/// Move a note ticket into the wiki: a page from its title and description,
/// then the ticket closed as Done with `Promoted to` naming the page.
///
/// Safe to repeat: a ticket already promoted to a page is reported, and a page
/// an interrupted run created is found by the line naming the ticket and linked
/// rather than created twice.
pub(crate) fn from_ticket(ctx: &Context<'_>, from: &FromTicket<'_>) -> Result<Output, String> {
    let tickets = &ctx.paths.tickets;
    let ticket = from.ticket;
    let path = ticket::store::locate_ticket(tickets, ticket).map_err(|error| error.to_string())?;
    let parsed = ticket::parse::document(&store::read(&path)?)
        .map_err(|error| format!("{ticket} is not a well-formed ticket: {error}"))?;

    let wiki = ctx.wiki()?;
    if let Some(page) = parsed
        .metadata
        .promoted_to
        .as_deref()
        .and_then(|promoted| promoted.parse::<PageId>().ok())
        .and_then(|id| wiki.find(id).ok())
    {
        return Ok(format!(
            "{ticket} is already moved to {} ({}).\n",
            page.id,
            ctx.paths.show(&page.path)
        )
        .into());
    }

    let marker = moved_marker(ticket);
    if let Some(page) = wiki.pages.iter().find(|page| page.source.contains(&marker)) {
        let closed = ticket::store::promote(tickets, ticket, &page.id.to_string())
            .map_err(|error| error.to_string())?;
        return Ok(format!(
            "{ticket} already moved to {}; linking it.\n{}: promoted to {}, closed as Done\n",
            ctx.paths.show(&page.path),
            ctx.paths.show(&closed),
            page.id
        )
        .into());
    }

    let status = from.status.unwrap_or(Status::Reference);
    if status == Status::Archived {
        return Err("A page starts as a Reference or a Snapshot; archive it later.".to_owned());
    }
    if let Some(parent) = from.parent {
        active_parent(&wiki, parent)?;
    }

    let vocabulary = ctx.options.vocabulary()?;
    let (labels, dropped) = carry(
        &vocabulary,
        &parsed.metadata.labels.to_tokens(),
        from.labels,
    )?;
    unique(&vocabulary, &labels, &wiki, None)?;

    let summary = from.summary.unwrap_or_default();
    let description = parsed.description.trim();
    let body = if description.is_empty() {
        marker
    } else {
        format!("{description}\n\n{marker}")
    };
    let text = page::render(&NewPage {
        title: &parsed.title,
        status,
        authors: &parsed.metadata.authors,
        date: &parsed.metadata.date,
        parent: from.parent,
        labels: &labels,
        summary,
        body: &body,
    });

    let (id, mut output) = create(ctx, &parsed.title, &text, status, summary)?;
    if !dropped.is_empty() {
        output.notes.push(format!(
            "Left out the labels the wiki doesn't take: {}.",
            dropped.join(", ")
        ));
    }

    let closed = ticket::store::promote(tickets, ticket, &id.to_string())
        .map_err(|error| error.to_string())?;
    output.text.push_str(&format!(
        "{}: promoted to {id}, closed as Done\n",
        ctx.paths.show(&closed)
    ));
    if !parsed.comments.is_empty() {
        output.notes.push(format!(
            "{ticket}'s comments stay with it; fold what matters into {id}."
        ));
    }

    Ok(output)
}

/// The labels a page moved from a ticket carries: the ticket's that the
/// vocabulary takes, and `requested`, with the ticket's it doesn't take.
///
/// A requested label the vocabulary refuses is an error, as is a set breaking a
/// key's `min` or `max`.
fn carry(
    vocabulary: &Vocabulary,
    ticket: &[String],
    requested: &[String],
) -> Result<(Labels, Vec<String>), String> {
    let takes = |token: &String| match vocabulary.resolve(std::slice::from_ref(token)) {
        Ok(_) => true,
        // Only a count can be off with one label alone, and the whole set is
        // what a count is checked against.
        Err(rejected) => {
            rejected.malformed.is_empty()
                && rejected.unknown_keys.is_empty()
                && rejected.unknown_values.is_empty()
                && rejected.retired.is_empty()
        }
    };

    let (kept, dropped): (Vec<String>, Vec<String>) =
        ticket.iter().cloned().partition(|token| takes(token));
    let mut tokens: BTreeSet<String> = kept.into_iter().collect();
    tokens.extend(requested.iter().cloned());
    let tokens: Vec<String> = tokens.into_iter().collect();

    let labels = vocabulary
        .resolve(&tokens)
        .map_err(|error| error.to_string())?;

    Ok((labels, dropped))
}

/// Check every page, exiting 1 when one breaks a rule, or, with `strict`, when
/// anything is reported.
pub(crate) fn check(ctx: &Context<'_>, strict: bool) -> Result<Output, String> {
    let wiki = ctx.wiki()?;
    let library = ctx.library();
    let repository = ctx.repository()?;
    let report = check::check(&check::Context {
        paths: &ctx.paths,
        options: ctx.options,
        wiki: &wiki,
        library: &library,
        repository: repository.as_deref(),
        access: &ctx.access,
    });
    let failing = !report.errors.is_empty() || (strict && !report.warnings.is_empty());
    let code = u8::from(failing);

    if ctx.format.is_json() {
        let text = serde_json::to_string(&report)
            .map_err(|error| format!("failed to serialize the check: {error}"))?;
        return Ok(Output {
            text,
            json: true,
            code,
            ..Output::default()
        });
    }

    Ok(Output {
        text: describe(&report, strict),
        code,
        ..Output::default()
    })
}

/// A check report as text: each finding on a line, then the counts.
fn describe(report: &Report, strict: bool) -> String {
    let line = |finding: &check::Finding| match finding.line {
        Some(line) => format!("{}:{line}: {}", finding.path, finding.message),
        None => format!("{}: {}", finding.path, finding.message),
    };

    let mut text = String::new();
    for finding in &report.errors {
        text.push_str(&format!("error: {}\n", line(finding)));
    }
    let kind = if strict { "error" } else { "warning" };
    for finding in &report.warnings {
        text.push_str(&format!("{kind}: {}\n", line(finding)));
    }

    let count = |count: usize, one: &str, many: &str| {
        format!("{count} {}", if count == 1 { one } else { many })
    };
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&format!(
        "Checked {}: {}, {}.\n",
        count(report.pages, "page", "pages"),
        count(report.errors.len(), "error", "errors"),
        count(report.warnings.len(), "warning", "warnings")
    ));

    text
}
