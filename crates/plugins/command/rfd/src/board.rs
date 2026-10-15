//! The priority board: every RFD, and where it stands.
//!
//! The board joins three sources:
//!
//! - The RFD files: published ones in the RFD directory, drafts in its
//!   `drafts/` directory.
//!
//! - The board file, [`PRIORITY_FILE`] in the RFD directory, which ranks open
//!   work into milestone groups above an unranked backlog:
//!
//!   ```json
//!   {
//!     "planned": [
//!       { "milestone": "Internal Release v0.1", "ids": ["044", "D50"] },
//!       { "milestone": null, "ids": ["053"] }
//!     ],
//!     "backlog": ["010", "D01"]
//!   }
//!   ```
//!
//! - The tickets: an RFD is in development while its tracking ticket, the one
//!   labelled `type=tracking` that implements it, is In Progress.
//!
//! [`Board::assemble`] joins sources already read, and [`load`] reads them.
//! Nothing here validates the board: an id it names that no RFD carries is
//! skipped, not reported.

use std::{
    collections::{BTreeSet, HashMap},
    fs, io,
    path::MAIN_SEPARATOR,
};

use camino::Utf8Path;
use serde::Serialize;
use serde_json::Value;
use ticket::{Status, store};

use crate::{
    document::{Header, referenced_labels, title},
    id::{DRAFTS_DIR, RfdId},
};

/// The board file, inside the RFD directory.
pub(crate) const PRIORITY_FILE: &str = ".priority.json";

/// The label marking a ticket as its RFD's tracking ticket.
const TRACKING_LABEL: (&str, &str) = ("type", "tracking");

/// Statuses that take an RFD off the board, which ranks open work only.
const TERMINAL_STATUSES: [&str; 3] = ["Implemented", "Superseded", "Abandoned"];

/// One milestone group of the planned list: the RFDs ranked above that
/// milestone's marker, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Group {
    /// `None` for the planned RFDs no milestone claims, which sit below the
    /// last marker.
    pub milestone: Option<String>,
    pub ids: Vec<String>,
}

/// The board file, normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Priority {
    /// The planned groups in board order, always ending with the unassigned
    /// (`milestone: None`) group.
    pub planned: Vec<Group>,

    /// The unranked backlog below the planned list.
    pub backlog: Vec<String>,
}

impl Default for Priority {
    fn default() -> Self {
        Self::normalize(&Value::Null)
    }
}

impl Priority {
    /// Read a board file's contents into a fixed shape.
    ///
    /// The older shape, a flat `order` array in place of `planned`, reads as a
    /// single unassigned group.
    /// Missing or malformed fields read as empty, the way a missing file is an
    /// empty board.
    #[must_use]
    pub(crate) fn normalize(raw: &Value) -> Self {
        let mut planned: Vec<Group> = match raw.get("planned").and_then(Value::as_array) {
            Some(groups) => groups
                .iter()
                .map(|group| Group {
                    milestone: group.get("milestone").and_then(text),
                    ids: ids(group.get("ids")),
                })
                .collect(),
            None => vec![Group {
                milestone: None,
                ids: ids(raw.get("order")),
            }],
        };

        if planned.last().is_none_or(|group| group.milestone.is_some()) {
            planned.push(Group {
                milestone: None,
                ids: vec![],
            });
        }

        Self {
            planned,
            backlog: ids(raw.get("backlog")),
        }
    }

    /// The planned ids, flattened in board order.
    ///
    /// Its length is the cutoff: a position below it is planned, one at or past
    /// it is in the backlog.
    #[must_use]
    pub(crate) fn order(&self) -> Vec<&str> {
        self.planned
            .iter()
            .flat_map(|group| group.ids.iter().map(String::as_str))
            .collect()
    }
}

/// An id list from the board file.
/// Numbers are read as their digits, since the file is hand-editable.
fn ids(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(text).collect())
        .unwrap_or_default()
}

/// A string or a number from the board file, as text; `None` for anything else,
/// `null` included.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

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
/// Serialized as one entry of `jp rfd list --json`.
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
    /// Join the RFD files, the board file, and the RFDs in development.
    ///
    /// `in_development` holds ids in their canonical form (`042`, `D05`).
    #[must_use]
    pub(crate) fn assemble(
        published: &[Source],
        drafts: &[Source],
        priority: Priority,
        in_development: &BTreeSet<String>,
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
            entry.in_development = in_development.contains(&entry.num);
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
        depends_on: vec![],
        relations,
        num,
    }
}

/// The RFDs whose tracking ticket is In Progress, by canonical id.
///
/// A tracking ticket carries the `type=tracking` label and names its RFD in
/// `Implements`.
/// Phase tickets name the same RFD without the label and don't count.
/// Tickets that didn't parse are passed over.
#[must_use]
pub(crate) fn in_development(tickets: &[store::Entry]) -> BTreeSet<String> {
    tickets
        .iter()
        .filter_map(|entry| entry.ticket.as_ref().ok())
        .filter(|ticket| {
            ticket.metadata.status == Status::InProgress
                && ticket
                    .metadata
                    .labels
                    .contains(TRACKING_LABEL.0, TRACKING_LABEL.1)
        })
        .filter_map(|ticket| ticket.metadata.implements.as_deref())
        .filter_map(|implements| implements.parse::<RfdId>().ok())
        .map(|id| id.to_string())
        .collect()
}

/// What [`load`] read, and what it had to pass over to read it.
#[derive(Debug)]
pub(crate) struct Loaded {
    pub board: Board,

    /// Problems that left part of the board unknown, for the log.
    pub warnings: Vec<String>,
}

/// Read the board: the RFDs in `dir`, its board file, and the tickets in
/// `tickets`.
///
/// `root` is the workspace root that entries' `file` paths are relative to.
/// A missing drafts directory holds no drafts, and a missing ticket directory
/// no tickets.
/// An unreadable board file or ticket directory leaves the board unranked or
/// nothing in development, with a warning saying so; a missing RFD directory is
/// an error.
pub(crate) fn load(root: &Utf8Path, dir: &Utf8Path, tickets: &Utf8Path) -> Result<Loaded, String> {
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

    let priority_path = dir.join(PRIORITY_FILE);
    let priority = match fs::read_to_string(&priority_path) {
        Ok(source) => match serde_json::from_str::<Value>(&source) {
            Ok(raw) => Priority::normalize(&raw),
            Err(error) => {
                warnings.push(format!(
                    "{priority_path} is not valid JSON ({error}); every RFD reads as unranked."
                ));
                Priority::default()
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Priority::default(),
        Err(error) => {
            warnings.push(format!(
                "{priority_path} could not be read ({error}); every RFD reads as unranked."
            ));
            Priority::default()
        }
    };

    let in_development = match store::list(tickets) {
        Ok(entries) => {
            let unreadable = entries.iter().filter(|entry| entry.ticket.is_err()).count();
            if unreadable > 0 {
                let files = if unreadable == 1 { "file" } else { "files" };
                warnings.push(format!(
                    "{unreadable} ticket {files} in {tickets} could not be read; an RFD whose \
                     tracking ticket is among them does not show as in development."
                ));
            }
            in_development(&entries)
        }
        Err(error) => {
            warnings.push(format!(
                "The tickets in {tickets} could not be read ({error}); no RFD shows as in \
                 development."
            ));
            BTreeSet::new()
        }
    };

    Ok(Loaded {
        board: Board::assemble(&published, &drafts, priority, &in_development),
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
