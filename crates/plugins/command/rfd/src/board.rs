//! The priority board: every RFD, and where it stands.
//!
//! The board joins three sources:
//!
//! - The RFD files: published ones in the RFD directory, drafts in its
//!   `drafts/` directory.
//!
//! - The board file ([`priority`]), which ranks open work into milestone groups
//!   above an unranked backlog.
//!
//! - The tickets that name an RFD ([`Links`]): an RFD is in development while
//!   its tracking ticket is In Progress.
//!
//! [`Board::assemble`] joins sources already read, and [`load`] reads them.
//! Nothing here validates the board: an id it names that no RFD carries is
//! skipped, not reported.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::MAIN_SEPARATOR,
};

use camino::Utf8Path;
use serde::Serialize;
use ticket::{Status, store};

use crate::{
    document::{Header, referenced_labels, title},
    id::{DRAFTS_DIR, RfdId},
    links::{LinkedTicket, Links},
    priority::{self, Priority},
};

/// Statuses that take an RFD off the board, which ranks open work only.
pub(crate) const TERMINAL_STATUSES: [&str; 3] = ["Implemented", "Superseded", "Abandoned"];

/// One RFD file, read.
#[derive(Debug, Clone)]
pub(crate) struct Source {
    /// The file name, `042-slug.md`.
    pub name: String,

    /// Where the file is, relative to the workspace root, with `/` separators.
    pub file: String,

    pub content: String,
}

/// One RFD and where it stands.
///
/// Serialized as one entry of `jp -F json rfd list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    /// The id, `042` or `D05`.
    pub num: String,

    /// The heading's title, or the file name for a document without one.
    pub title: String,

    pub status: Option<String>,
    pub category: Option<String>,

    /// The RFD that replaced this one.
    pub superseded_by: Option<String>,

    pub authors: Option<String>,
    pub date: Option<String>,
    pub summary: Option<String>,

    /// The file name without `.md`.
    pub slug: String,

    /// Where the file is, relative to the workspace root.
    pub file: String,

    /// The RFDs the document links to, by id, sorted.
    pub references: Vec<String>,

    /// Every document the text links to, tickets included, in the order they
    /// first appear.
    pub links: Vec<String>,

    /// The RFDs that link to this one: published ones for a published RFD,
    /// drafts for a draft.
    pub referenced_by: Vec<String>,

    /// The position on the board, planned and backlog together, top first.
    /// `None` for an RFD the board doesn't place, which includes every terminal
    /// one.
    pub priority: Option<usize>,

    /// The milestone group a planned RFD sits in, if any.
    pub milestone: Option<String>,

    /// Whether the RFD's tracking ticket is In Progress.
    pub in_development: bool,

    /// The tickets naming the RFD, the ones tracking it first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tickets: Vec<LinkedTicket>,

    /// The RFDs this one requires or extends, which the board keeps above it.
    pub depends_on: Vec<String>,

    #[serde(skip)]
    pub relations: Relations,
}

impl Entry {
    /// Whether the RFD's status takes it off the board.
    #[must_use]
    pub(crate) fn is_terminal(&self) -> bool {
        self.status
            .as_deref()
            .is_some_and(|status| TERMINAL_STATUSES.contains(&status))
    }
}

/// The relationships an RFD's header records, each a list of RFD ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Relations {
    pub requires: Vec<String>,
    pub required_by: Vec<String>,
    pub extends: Vec<String>,
    pub extended_by: Vec<String>,
    pub supersedes: Vec<String>,
    pub superseded_by: Vec<String>,
}

/// Every RFD on the board, and the board file it was ranked from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Board {
    /// Published RFDs by file name, then drafts by file name.
    pub entries: Vec<Entry>,

    pub priority: Priority,
}

impl Board {
    /// Join the RFD files, the board file, and the tickets naming each RFD.
    ///
    /// `tickets` is keyed by the RFD's canonical id (`042`, `D05`), as
    /// [`Links::by_rfd`] gathers it.
    #[must_use]
    pub(crate) fn assemble(
        published: &[Source],
        drafts: &[Source],
        priority: Priority,
        tickets: &BTreeMap<String, Vec<LinkedTicket>>,
    ) -> Self {
        let mut entries = linked(published);
        entries.extend(linked(drafts));

        // Keyed by id, so two files sharing an id both take the last one's
        // dependencies, as the site's board reads them.
        let depends_on: HashMap<String, Vec<String>> = entries
            .iter()
            .map(|entry| {
                let mut ids: Vec<String> = vec![];
                for id in entry
                    .relations
                    .requires
                    .iter()
                    .chain(&entry.relations.extends)
                {
                    if !ids.contains(id) {
                        ids.push(id.clone());
                    }
                }
                (entry.num.clone(), ids)
            })
            .collect();

        let order = priority.order();
        let positions: HashMap<&str, usize> = order
            .iter()
            .copied()
            .chain(priority.backlog.iter().map(String::as_str))
            .enumerate()
            .map(|(position, id)| (id, position))
            .collect();
        let milestones: HashMap<&str, Option<&str>> = priority
            .planned
            .iter()
            .flat_map(|group| {
                group
                    .ids
                    .iter()
                    .map(|id| (id.as_str(), group.milestone.as_deref()))
            })
            .collect();

        for entry in &mut entries {
            let placed = !entry.is_terminal() && positions.contains_key(entry.num.as_str());

            entry.priority = placed
                .then(|| positions.get(entry.num.as_str()).copied())
                .flatten();
            entry.milestone = placed
                .then(|| milestones.get(entry.num.as_str()).copied().flatten())
                .flatten()
                .map(ToOwned::to_owned);
            entry.tickets = tickets.get(&entry.num).cloned().unwrap_or_default();
            entry.in_development = entry
                .tickets
                .iter()
                .any(|ticket| ticket.tracking && ticket.status == Status::InProgress);
            entry.depends_on = depends_on.get(&entry.num).cloned().unwrap_or_default();
        }

        Self { entries, priority }
    }

    /// The position past the last planned RFD: a lower `priority` is planned,
    /// this one or higher is in the backlog.
    #[must_use]
    pub(crate) fn cutoff(&self) -> usize {
        self.priority.order().len()
    }

    /// The entry for the RFD file at `file`, relative to the workspace root.
    #[must_use]
    pub(crate) fn entry(&self, file: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.file == file)
    }
}

/// The entries for one set of files, each with the others in the set that link
/// to it.
fn linked(sources: &[Source]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = sources.iter().map(entry).collect();

    let referenced_by: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            entries
                .iter()
                .filter(|other| other.references.contains(&entry.num))
                .map(|other| other.num.clone())
                .collect()
        })
        .collect();

    for (entry, by) in entries.iter_mut().zip(referenced_by) {
        entry.referenced_by = by;
    }

    entries
}

/// Read one RFD file into an entry, with nothing yet known about the board.
fn entry(source: &Source) -> Entry {
    let num = RfdId::of_file(&source.name).map_or_else(|| "000".to_owned(), |id| id.to_string());
    let lines: Vec<&str> = source.content.lines().collect();
    let header = Header::of(&lines);

    let labels = referenced_labels(&source.content);
    let mut references: Vec<String> = labels
        .iter()
        .filter(|label| !label.starts_with('T') && *label != "000" && **label != num)
        .cloned()
        .collect();
    references.sort();
    let links = labels.into_iter().filter(|label| *label != num).collect();

    let relations = Relations {
        requires: header.citations("Requires"),
        required_by: header.citations("Required by"),
        extends: header.citations("Extends"),
        extended_by: header.citations("Extended by"),
        supersedes: header.citations("Supersedes"),
        superseded_by: header.citations("Superseded by"),
    };

    Entry {
        title: title(&source.content).unwrap_or_else(|| source.name.clone()),
        status: header.field("Status"),
        category: header.field("Category"),
        superseded_by: relations.superseded_by.first().cloned(),
        authors: header.field("Authors"),
        date: header.field("Date"),
        summary: header.wrapped("Summary"),
        slug: source
            .name
            .strip_suffix(".md")
            .unwrap_or(&source.name)
            .to_owned(),
        file: source.file.clone(),
        references,
        links,
        referenced_by: vec![],
        priority: None,
        milestone: None,
        in_development: false,
        tickets: vec![],
        depends_on: vec![],
        relations,
        num,
    }
}

/// What [`load`] read, and what it had to pass over to read it.
#[derive(Debug)]
pub(crate) struct Loaded {
    pub board: Board,

    /// Problems that left part of the board unknown, for the log.
    pub warnings: Vec<String>,
}

/// Read the board: the RFDs in `dir`, its board file, and, when `tickets` names
/// a ticket directory and the keys tickets name RFDs under, the tickets naming
/// each RFD.
///
/// `root` is the workspace root that entries' `file` paths are relative to.
/// A missing drafts directory holds no drafts, and a missing ticket directory
/// no tickets.
/// An unreadable board file or ticket directory leaves the board unranked or
/// nothing in development, with a warning saying so; a missing RFD directory is
/// an error.
pub(crate) fn load(
    root: &Utf8Path,
    dir: &Utf8Path,
    tickets: Option<(&Utf8Path, &Links)>,
) -> Result<Loaded, String> {
    if !dir.is_dir() {
        return Err(format!(
            "There is no RFD directory at {dir}. Set `plugins.command.rfd.options.dir` to the \
             directory holding the RFDs."
        ));
    }

    let mut warnings = vec![];

    let published = sources(root, dir, |id| matches!(id, RfdId::Published(_)))?;
    let drafts_dir = dir.join(DRAFTS_DIR);
    let drafts = if drafts_dir.is_dir() {
        sources(root, &drafts_dir, |id| matches!(id, RfdId::Draft(_)))?
    } else {
        vec![]
    };

    let priority = match priority::read(dir) {
        Ok(priority) => priority.unwrap_or_default(),
        Err(error) => {
            warnings.push(format!("{error}; every RFD reads as unranked."));
            Priority::default()
        }
    };

    let linked = match tickets {
        None => BTreeMap::new(),
        Some((tickets, links)) => match store::list(tickets) {
            Ok(entries) => {
                let unreadable = entries.iter().filter(|entry| entry.ticket.is_err()).count();
                if unreadable > 0 {
                    let files = if unreadable == 1 { "file" } else { "files" };
                    warnings.push(format!(
                        "{unreadable} ticket {files} in {tickets} could not be read; an RFD whose \
                         tracking ticket is among them does not show as in development."
                    ));
                }
                links.by_rfd(&entries)
            }
            Err(error) => {
                warnings.push(format!(
                    "The tickets in {tickets} could not be read ({error}); no RFD shows as in \
                     development."
                ));
                BTreeMap::new()
            }
        },
    };

    Ok(Loaded {
        board: Board::assemble(&published, &drafts, priority, &linked),
        warnings,
    })
}

/// Read the RFD files in `dir` whose id `wanted` accepts, by file name.
///
/// A file counts when its name is an id, a dash, a slug, and `.md`.
fn sources(
    root: &Utf8Path,
    dir: &Utf8Path,
    wanted: fn(RfdId) -> bool,
) -> Result<Vec<Source>, String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("{dir}: {error}"))?;

    let mut names = vec![];
    for entry in entries {
        let entry = entry.map_err(|error| format!("{dir}: {error}"))?;
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };

        let named = RfdId::of_file(&name).is_some_and(wanted);
        let slugged = name
            .split_once('-')
            .and_then(|(_, rest)| rest.strip_suffix(".md"))
            .is_some_and(|slug| !slug.is_empty());
        if named && slugged {
            names.push(name);
        }
    }
    names.sort();

    names
        .into_iter()
        .map(|name| {
            let path = dir.join(&name);
            let content = fs::read_to_string(&path).map_err(|error| format!("{path}: {error}"))?;

            Ok(Source {
                file: relative(root, &path),
                name,
                content,
            })
        })
        .collect()
}

/// `path` relative to `root`, or as it is outside `root`.
///
/// Separators are always `/`: the path is shown to people and the assistant,
/// and the fs tools take forward slashes on every platform.
#[must_use]
pub(crate) fn relative(root: &Utf8Path, path: &Utf8Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .as_str()
        .replace(MAIN_SEPARATOR, "/")
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
