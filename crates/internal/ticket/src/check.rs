//! Hold a whole ticket directory to the rules each write is held to.
//!
//! A write is checked when it is made, so a ticket edited by hand, or filed
//! before a rule existed, is only caught by reading the directory again.
//! [`report`] does that over files already read; `store::check` reads them.

use std::fmt;

use camino::Utf8PathBuf;
use jp_label::{Clash, Rejected};

use crate::{Labels, ParseError, TicketId, Vocabulary, parse, store::held_to_one};

/// A ticket directory as read.
#[derive(Debug, Default)]
pub struct Directory {
    /// Files named in the four-digit `NNNN-slug.md` form ids had before they
    /// carried a time bucket, which no command reads.
    pub legacy: Vec<Utf8PathBuf>,

    /// Every ticket file, ordered by id, so files claiming one id sit together.
    pub tickets: Vec<TicketFile>,
}

/// One ticket file and what it holds.
#[derive(Debug)]
pub struct TicketFile {
    pub id: TicketId,
    pub path: Utf8PathBuf,
    pub source: String,
}

/// What reading a ticket directory found.
#[derive(Debug)]
pub struct Report {
    /// How many ticket files were read.
    pub tickets: usize,

    /// Every problem, file-wide ones first, then each ticket's in id order,
    /// then labels shared across tickets.
    pub problems: Vec<Problem>,
}

/// Something in a ticket directory that a command would refuse to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A file in the four-digit form ids had before they carried a time bucket;
    /// no command reads it.
    LegacyName(Utf8PathBuf),

    /// No epoch file beside existing tickets, so no new id can be allocated.
    MissingEpoch(Utf8PathBuf),

    /// An epoch file that doesn't hold an epoch.
    MalformedEpoch {
        path: Utf8PathBuf,
        error: ParseError,
    },

    /// Files claiming one id, so a reference to it can't tell them apart.
    Duplicate {
        id: TicketId,
        paths: Vec<Utf8PathBuf>,
    },

    /// A ticket file that isn't a well-formed ticket.
    Malformed {
        id: TicketId,
        path: Utf8PathBuf,
        error: ParseError,
    },

    /// Labels the board doesn't define, or a key the board bounds that holds
    /// too few or too many.
    ///
    /// Carries no list of the labels a ticket could carry instead: a report
    /// names many tickets, and `jp ticket labels` lists them once.
    Labels {
        id: TicketId,
        path: Utf8PathBuf,
        rejected: Rejected,
    },

    /// A label the board lets one ticket carry, on several.
    Shared { clash: Clash, ids: Vec<TicketId> },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LegacyName(path) => write!(
                f,
                "{path} is named in the four-digit `NNNN-slug.md` form, which no command reads. \
                 Rename it to the `<id>-slug.md` form."
            ),
            Self::MissingEpoch(path) => write!(
                f,
                "{path} is missing, so no new ticket can be filed. Restore it from version \
                 control."
            ),
            Self::MalformedEpoch { path, error } => write!(f, "{path}: {error}"),
            Self::Duplicate { id, paths } => write!(
                f,
                "{id} is claimed by more than one file: {}. Give all but one a fresh id with `jp \
                 ticket refresh PATH`.",
                join(paths)
            ),
            Self::Malformed { path, error, .. } => {
                write!(f, "{path} is not a well-formed ticket: {error}")
            }
            Self::Labels { id, rejected, .. } => write!(f, "{id}: {rejected}"),
            Self::Shared { clash, ids } => write!(
                f,
                "`{clash}` is on {}, and {}.",
                join(ids),
                held_to_one(clash)
            ),
        }
    }
}

/// Check every ticket in `directory` against `vocabulary`.
///
/// A ticket's labels are read as written, whether or not the rest of the file
/// parses, so one with a broken header still reports what its labels break.
/// Retired values a ticket carries are accepted: retiring a value keeps the
/// tickets that have it valid.
#[must_use]
pub fn report(directory: &Directory, vocabulary: &Vocabulary) -> Report {
    let mut problems: Vec<Problem> = directory
        .legacy
        .iter()
        .cloned()
        .map(Problem::LegacyName)
        .collect();
    problems.extend(duplicates(&directory.tickets));

    let mut labelled = vec![];
    for ticket in &directory.tickets {
        if let Err(error) = parse::document(&ticket.source) {
            problems.push(Problem::Malformed {
                id: ticket.id,
                path: ticket.path.clone(),
                error,
            });
        }

        let tokens = parse::label_tokens(&ticket.source);
        let carried = Labels::from_tokens_lossy(&tokens).0;
        if let Err(mut rejected) = vocabulary.resolve_against(&tokens, &carried) {
            rejected.addable.clear();
            rejected.open_keys.clear();
            problems.push(Problem::Labels {
                id: ticket.id,
                path: ticket.path.clone(),
                rejected,
            });
        }
        labelled.push((ticket.id, carried));
    }
    problems.extend(shared(vocabulary, &labelled));

    Report {
        tickets: directory.tickets.len(),
        problems,
    }
}

/// Every id more than one file claims.
fn duplicates(tickets: &[TicketFile]) -> Vec<Problem> {
    tickets
        .chunk_by(|a, b| a.id == b.id)
        .filter(|claiming| claiming.len() > 1)
        .filter_map(|claiming| {
            Some(Problem::Duplicate {
                id: claiming.first()?.id,
                paths: claiming.iter().map(|ticket| ticket.path.clone()).collect(),
            })
        })
        .collect()
}

/// Each label the vocabulary lets one ticket carry that several of `labelled`
/// do, with every ticket carrying it.
///
/// Values match without case, as they do when a write is checked.
fn shared(vocabulary: &Vocabulary, labelled: &[(TicketId, Labels)]) -> Vec<Problem> {
    if !vocabulary
        .facets()
        .any(|(_, facet)| facet.unique().is_some())
    {
        return vec![];
    }

    let mut groups: Vec<(Clash, Vec<TicketId>)> = vec![];
    for (index, (id, labels)) in labelled.iter().enumerate() {
        for (other, theirs) in labelled.iter().skip(index + 1) {
            for clash in vocabulary.clashes(labels, theirs) {
                let known = groups.iter().position(|(known, _)| same(known, &clash));
                let group = known.unwrap_or_else(|| {
                    groups.push((clash, vec![]));
                    groups.len() - 1
                });
                let ids = &mut groups[group].1;
                for id in [*id, *other] {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        }
    }

    groups
        .into_iter()
        .map(|(clash, mut ids)| {
            ids.sort_unstable();
            Problem::Shared { clash, ids }
        })
        .collect()
}

/// Whether two clashes name the same label: the key as the vocabulary spells
/// it, and the value without case.
fn same(a: &Clash, b: &Clash) -> bool {
    a.key == b.key
        && match (&a.value, &b.value) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            (None, None) => true,
            _ => false,
        }
}

fn join(items: &[impl fmt::Display]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
