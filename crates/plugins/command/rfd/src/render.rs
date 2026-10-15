//! What `list` and `show` print.
//!
//! The listing groups the board into three sections, each opt-in:
//!
//! - Planned: the ranked RFDs, in priority order, with a line under the last
//!   RFD of each milestone group.
//! - Backlog: RFDs on the board below the planned ones, unranked.
//! - Other: every RFD the board doesn't place, terminal ones included.
//!
//! Text output reads as a table, one RFD per row; JSON output carries every
//! RFD, tagged with its section, for a script to filter itself.

use serde::Serialize;

use crate::board::{Board, Entry};

/// The sections a listing shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sections {
    pub planned: bool,
    pub backlog: bool,
    pub other: bool,
}

impl Sections {
    /// The sections the `--planned`, `--backlog`, and `--all` flags name.
    ///
    /// Naming none shows the planned RFDs; `all` shows every section.
    #[must_use]
    pub(crate) fn from_flags(planned: bool, backlog: bool, all: bool) -> Self {
        let any = planned || backlog || all;

        Self {
            planned: all || planned || !any,
            backlog: all || backlog,
            other: all,
        }
    }
}

/// The board's entries in their sections, each in display order.
struct Grouped<'a> {
    planned: Vec<&'a Entry>,
    backlog: Vec<&'a Entry>,
    other: Vec<&'a Entry>,
}

impl<'a> Grouped<'a> {
    /// Sort the entries of `category` into sections; every entry when
    /// `category` is `None`.
    ///
    /// The category matches without regard to case.
    fn of(board: &'a Board, category: Option<&str>) -> Self {
        let cutoff = board.cutoff();
        let category = category.map(str::to_lowercase);
        let shown = board.entries.iter().filter(|entry| {
            category.as_ref().is_none_or(|category| {
                entry
                    .category
                    .as_ref()
                    .is_some_and(|own| own.to_lowercase() == *category)
            })
        });

        let mut planned = vec![];
        let mut backlog = vec![];
        let mut other = vec![];
        for entry in shown {
            match entry.priority {
                Some(position) if position < cutoff => planned.push(entry),
                Some(_) => backlog.push(entry),
                None => other.push(entry),
            }
        }

        planned.sort_by_key(|entry| entry.priority);
        backlog.sort_by_key(|entry| entry.priority);
        other.sort_by(|a, b| a.num.cmp(&b.num));

        Self {
            planned,
            backlog,
            other,
        }
    }

    fn count(&self) -> usize {
        self.planned.len() + self.backlog.len() + self.other.len()
    }
}

/// Where an entry sits in a listing of every RFD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Section {
    Planned,
    Backlog,
    Other,
}

/// One entry of a JSON listing: the RFD, its section, and its rank among the
/// planned RFDs shown.
#[derive(Serialize)]
struct Tagged<'a> {
    #[serde(flatten)]
    entry: &'a Entry,
    group: Section,
    rank: Option<usize>,
}

/// List the board as text.
///
/// The first line names the sections and how many RFDs each holds, so the
/// sections not shown can still be found.
#[must_use]
pub(crate) fn list_text(
    board: &Board,
    category: Option<&str>,
    sections: Sections,
    full: bool,
) -> String {
    let grouped = Grouped::of(board, category);

    let shown: Vec<(&str, &[&Entry], bool)> = [
        (
            sections.planned,
            "Planned (priority order, * in development)",
            grouped.planned.as_slice(),
            true,
        ),
        (
            sections.backlog,
            "Backlog (not yet ranked)",
            grouped.backlog.as_slice(),
            false,
        ),
        (
            sections.other,
            "Other (implemented, terminal, or unplaced)",
            grouped.other.as_slice(),
            false,
        ),
    ]
    .into_iter()
    .filter(|(wanted, _, entries, _)| *wanted && !entries.is_empty())
    .map(|(_, title, entries, ranked)| (title, entries, ranked))
    .collect();

    if shown.is_empty() {
        return "No matching RFDs.\n".to_owned();
    }

    // Measured over the rows shown, so the common case stays narrow and only a
    // listing with `Superseded (NNN)` in it widens.
    let status_width = shown
        .iter()
        .flat_map(|(_, entries, _)| entries.iter())
        .map(|entry| status_text(entry).chars().count())
        .max()
        .unwrap_or(0)
        .max(10);

    let mut tips = vec![
        format!("--planned ({})", grouped.planned.len()),
        format!("--backlog ({})", grouped.backlog.len()),
        format!("--all ({})", grouped.count()),
    ];
    if !full {
        tips.push("--full".to_owned());
    }

    let mut blocks = vec![tips.join(", ")];
    for (title, entries, ranked) in shown {
        let mut lines = vec![];
        for (index, entry) in entries.iter().enumerate() {
            let rank = ranked.then_some(index + 1);
            lines.push(row(entry, rank, status_width, full));

            // A milestone's marker closes the group of rows above it, as on
            // the site's board.
            let next = entries.get(index + 1).map(|next| next.milestone.as_deref());
            if ranked
                && let Some(milestone) = entry.milestone.as_deref()
                && next != Some(Some(milestone))
            {
                lines.push(format!("        ─── {milestone} milestone ───"));
            }
        }
        blocks.push(format!("{title}\n\n{}", lines.join("\n")));
    }

    format!("{}\n", blocks.join("\n\n"))
}

/// One row of a section: rank, in-development mark, id, status, title, and with
/// `full` the summary and dependencies below it.
fn row(entry: &Entry, rank: Option<usize>, status_width: usize, full: bool) -> String {
    let rank = rank.map_or_else(|| "   ".to_owned(), |rank| format!("{rank:>3}"));
    let development = if entry.in_development { '*' } else { ' ' };
    let status = status_text(entry);

    let mut lines = vec![format!(
        "{rank}  {development}  {:<3}  {status:<status_width$}  {}",
        entry.num, entry.title
    )];

    if full {
        if let Some(summary) = &entry.summary {
            lines.push(format!("         {summary}"));
        }
        if !entry.depends_on.is_empty() {
            lines.push(format!("         needs: {}", entry.depends_on.join(", ")));
        }
    }

    lines.join("\n")
}

/// The status column: `Superseded (NNN)` for an RFD a later one replaced, the
/// status otherwise, and `?` for a document without one.
fn status_text(entry: &Entry) -> String {
    match (entry.status.as_deref(), entry.superseded_by.as_deref()) {
        (Some("Superseded"), Some(by)) => format!("Superseded ({by})"),
        (Some(status), _) => status.to_owned(),
        (None, _) => "?".to_owned(),
    }
}

/// List the board as JSON: every RFD of `category`, in section order, tagged
/// with its section and its rank among the planned ones.
///
/// The sections flags don't apply: a script filters on the tags itself.
pub(crate) fn list_json(board: &Board, category: Option<&str>) -> Result<String, String> {
    let grouped = Grouped::of(board, category);

    let tagged: Vec<Tagged<'_>> = grouped
        .planned
        .iter()
        .enumerate()
        .map(|(index, entry)| Tagged {
            entry,
            group: Section::Planned,
            rank: Some(index + 1),
        })
        .chain(grouped.backlog.iter().map(|entry| Tagged {
            entry,
            group: Section::Backlog,
            rank: None,
        }))
        .chain(grouped.other.iter().map(|entry| Tagged {
            entry,
            group: Section::Other,
            rank: None,
        }))
        .collect();

    json(&tagged)
}

/// Where `entry` sits in a listing of every RFD: its section, and its rank if
/// it is planned.
fn placement(board: &Board, entry: &Entry) -> (Section, Option<usize>) {
    let grouped = Grouped::of(board, None);
    // A file holds one RFD, so its path tells two entries apart even when they
    // share an id.
    let same = |candidate: &&Entry| candidate.file == entry.file;

    if let Some(index) = grouped.planned.iter().position(same) {
        return (Section::Planned, Some(index + 1));
    }
    if grouped.backlog.iter().any(same) {
        return (Section::Backlog, None);
    }

    (Section::Other, None)
}

/// Show one RFD as markdown: its heading, its metadata, where it stands on the
/// board, and its summary.
///
/// `entry` has to be one of `board`'s entries.
#[must_use]
pub(crate) fn show_text(board: &Board, entry: &Entry) -> String {
    let mut out = format!("# RFD {}: {}\n\n", entry.num, entry.title);

    let mut field = |key: &str, value: &str| out.push_str(&format!("- **{key}**: {value}\n"));
    let cite = |ids: &[String]| {
        ids.iter()
            .map(|id| format!("RFD {id}"))
            .collect::<Vec<_>>()
            .join(", ")
    };

    field("File", &entry.file);
    for (key, value) in [
        ("Status", &entry.status),
        ("Category", &entry.category),
        ("Authors", &entry.authors),
        ("Date", &entry.date),
    ] {
        if let Some(value) = value {
            field(key, value);
        }
    }

    let relations = &entry.relations;
    for (key, ids) in [
        ("Requires", &relations.requires),
        ("Required by", &relations.required_by),
        ("Extends", &relations.extends),
        ("Extended by", &relations.extended_by),
        ("Supersedes", &relations.supersedes),
        ("Superseded by", &relations.superseded_by),
    ] {
        if !ids.is_empty() {
            field(key, &cite(ids));
        }
    }

    let board_text = match placement(board, entry) {
        (Section::Planned, Some(rank)) => match &entry.milestone {
            Some(milestone) => format!("planned, rank {rank}, in the {milestone} milestone"),
            None => format!("planned, rank {rank}"),
        },
        (Section::Backlog, _) => "backlog".to_owned(),
        _ => "not on the board".to_owned(),
    };
    field("Board", &board_text);
    field(
        "In development",
        if entry.in_development { "yes" } else { "no" },
    );

    if !entry.referenced_by.is_empty() {
        field("Referenced by", &cite(&entry.referenced_by));
    }

    if let Some(summary) = &entry.summary {
        out.push_str(&format!("\n{summary}\n"));
    }

    out
}

/// Show one RFD as JSON: the entry `list --json` prints for it, tagged the same
/// way.
pub(crate) fn show_json(board: &Board, entry: &Entry) -> Result<String, String> {
    let (group, rank) = placement(board, entry);

    json(&Tagged { entry, group, rank })
}

/// Pretty-printed JSON with a trailing newline.
fn json(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map(|json| format!("{json}\n"))
        .map_err(|error| format!("failed to serialize the board: {error}"))
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
