//! The board: which column a ticket sits in, and where within it.
//!
//! A ticket's status is its column, and lives in the ticket's own file.
//! The order within a column belongs to no single ticket, so it lives in one
//! file next to them, [`BOARD_FILE`], and a re-rank rewrites that file alone:
//!
//! ```json
//! {
//!   "todo": ["T-02wt0kx", "T-02wt0m3"],
//!   "in_progress": [],
//!   "done": []
//! }
//! ```
//!
//! A ticket the file doesn't name sorts below the ones it does: oldest first,
//! or newest first in Done.
//! A new ticket therefore enters its column unranked, and a board needs no
//! order file until something is ranked.
//!
//! [`Order`] is the file's contents and the pure operations on them;
//! [`read()`], [`write()`], [`rank()`], and [`move_to()`] touch the disk.
//! [`rank()`] and [`move_to()`] hold the board's lock, [`LOCK_FILE`], from
//! their first read to their last write.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io,
};

use camino::{Utf8Path, Utf8PathBuf};
use doc::format::Formatting;
use serde::{Deserialize, Serialize};

use crate::{
    Status, TicketId,
    store::{self, Error},
};

/// The order file, inside the ticket directory.
pub const BOARD_FILE: &str = ".board.json";

/// The lock file, inside the ticket directory.
///
/// A writer holds an exclusive lock on it while it reads the tickets and the
/// order file, decides where a ticket goes, and writes the result, so two
/// writers, in one process or in several, never both read one order and each
/// write back their own.
/// It is created on first use and never removed: a lock file unlinked on
/// release lets the next writer lock a new file while a waiting one still locks
/// the old one.
pub const LOCK_FILE: &str = ".board.lock";

/// Where a ticket lands relative to the one it is ranked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Directly above it.
    Before,
    /// Directly below it.
    After,
}

/// Where [`move_to`] puts a ticket in the column it moves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Directly above or below another ticket in the column.
    Next(Place, TicketId),
    /// Above every other ticket in the column.
    Top,
    /// Below every other ticket in the column.
    Bottom,
}

/// The ranked tickets of each column, top first.
///
/// Entries are kept as written: one naming a ticket that has since changed
/// column, or been deleted, reads fine and is skipped when the column is laid
/// out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    #[serde(default)]
    pub todo: Vec<String>,
    #[serde(default)]
    pub in_progress: Vec<String>,
    #[serde(default)]
    pub done: Vec<String>,
}

impl Order {
    /// The ranked entries of the column holding `status`.
    #[must_use]
    pub fn column(&self, status: Status) -> &[String] {
        match status {
            Status::Todo => &self.todo,
            Status::InProgress => &self.in_progress,
            Status::Done => &self.done,
        }
    }

    fn column_mut(&mut self, status: Status) -> &mut Vec<String> {
        match status {
            Status::Todo => &mut self.todo,
            Status::InProgress => &mut self.in_progress,
            Status::Done => &mut self.done,
        }
    }

    /// Lay out one column the way the board shows it.
    ///
    /// `tickets` are the tickets at `status`.
    /// Those the column ranks come first, in its order; the rest follow by id,
    /// oldest first, or newest first in Done.
    #[must_use]
    pub fn arrange(&self, status: Status, tickets: &[TicketId]) -> Vec<TicketId> {
        // Collected rather than searched per comparison. A ticket named twice
        // takes its last position, as it does on the site's board.
        let ranks: HashMap<&str, usize> = self
            .column(status)
            .iter()
            .enumerate()
            .map(|(rank, id)| (id.as_str(), rank))
            .collect();

        let mut keyed: Vec<(Option<usize>, TicketId)> = tickets
            .iter()
            .map(|id| (ranks.get(id.to_string().as_str()).copied(), *id))
            .collect();

        keyed.sort_by(|(a_rank, a), (b_rank, b)| match (a_rank, b_rank) {
            (Some(a_rank), Some(b_rank)) => a_rank.cmp(b_rank),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) if status == Status::Done => b.cmp(a),
            (None, None) => a.cmp(b),
        });

        keyed.into_iter().map(|(_, id)| id).collect()
    }

    /// Move `id` directly before or after `other`, both tickets at `status`.
    ///
    /// The column is written out down to the lowest ticket that has to be
    /// ranked to hold its place: the moved one, or one ranked already.
    /// Tickets below that stay unranked and keep sorting by id, and entries
    /// naming tickets no longer at `status` are dropped from the column.
    ///
    /// Returns `false`, and changes nothing, when `id` or `other` is missing
    /// from `tickets` or they are the same ticket.
    pub fn rank(
        &mut self,
        status: Status,
        tickets: &[TicketId],
        id: TicketId,
        place: Place,
        other: TicketId,
    ) -> bool {
        if id == other || !tickets.contains(&id) || !tickets.contains(&other) {
            return false;
        }

        let ranked: HashSet<String> = self.column(status).iter().cloned().collect();

        let mut laid_out = self.arrange(status, tickets);
        laid_out.retain(|ticket| *ticket != id);

        let Some(anchor) = laid_out.iter().position(|ticket| *ticket == other) else {
            return false;
        };
        let at = match place {
            Place::Before => anchor,
            Place::After => anchor + 1,
        };
        laid_out.insert(at, id);

        let keep = laid_out
            .iter()
            .rposition(|ticket| *ticket == id || ranked.contains(&ticket.to_string()))
            .map_or(0, |last| last + 1);

        *self.column_mut(status) = laid_out[..keep].iter().map(ToString::to_string).collect();

        true
    }

    /// Drop every entry naming `id`, in every column.
    ///
    /// Returns whether any entry was dropped.
    fn forget(&mut self, id: TicketId) -> bool {
        let name = id.to_string();
        let mut dropped = false;
        for column in [&mut self.todo, &mut self.in_progress, &mut self.done] {
            let count = column.len();
            column.retain(|entry| *entry != name);
            dropped |= column.len() != count;
        }

        dropped
    }
}

/// Read the board's order from the ticket directory.
///
/// A board without an order file has ranked nothing yet, which is not an error.
/// A file that is there but doesn't hold an order is, so a write never replaces
/// what it couldn't read.
pub fn read(dir: &Utf8Path) -> Result<Order, Error> {
    let path = dir.join(BOARD_FILE);
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Order::default()),
        Err(error) => return Err(error.into()),
    };

    if source.trim().is_empty() {
        return Ok(Order::default());
    }

    serde_json::from_str(&source).map_err(|error| Error::Board {
        path,
        reason: error.to_string(),
    })
}

/// Write the board's order into the ticket directory.
///
/// Laid out the way the site's board writes it, two-space indentation and a
/// trailing newline, so the file doesn't change shape with whoever wrote it
/// last.
pub fn write(dir: &Utf8Path, order: &Order) -> Result<(), Error> {
    let path = dir.join(BOARD_FILE);
    let mut json = serde_json::to_string_pretty(order).map_err(|error| Error::Board {
        path: path.clone(),
        reason: error.to_string(),
    })?;
    json.push('\n');

    fs::write(path, json)?;

    Ok(())
}

/// Rank `id` directly before or after `other` on the board, returning the
/// column both sit in.
///
/// Both have to be readable tickets at the same status: the order within one
/// column says nothing about another.
pub fn rank(dir: &Utf8Path, id: TicketId, place: Place, other: TicketId) -> Result<Status, Error> {
    if id == other {
        return Err(Error::SameTicket(id));
    }

    let _lock = lock(dir)?;
    let entries = store::list(dir)?;
    let status = status_of(&entries, id)?;
    let other_status = status_of(&entries, other)?;
    if status != other_status {
        return Err(Error::OtherColumn {
            id,
            status,
            other,
            other_status,
        });
    }

    let column: Vec<TicketId> = entries
        .iter()
        .filter(|entry| {
            entry
                .ticket
                .as_ref()
                .is_ok_and(|ticket| ticket.metadata.status == status)
        })
        .map(|entry| entry.id)
        .collect();

    let mut order = read(dir)?;
    // Both tickets were just found at `status`, so the move always lands.
    order.rank(status, &column, id, place, other);
    write(dir, &order)?;

    Ok(status)
}

/// A ticket [`move_to`] moved: where it is, and the status it held before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub path: Utf8PathBuf,
    pub previous: Status,
}

/// Move `id` to the column `status` names, placed there as `at` says.
///
/// Every check runs before anything is written, so a refused move changes
/// nothing: a ticket to place next to has to be another readable ticket already
/// at `status`, and the order file has to be readable.
///
/// [`Placement::Top`] and [`Placement::Bottom`] place the ticket above or below
/// every other ticket in the column as it stands while the board's lock is
/// held; in a column holding no other ticket they place nothing.
/// A ticket that changes column loses any rank it held in any column, so
/// without a placement it joins its new column below the ranked tickets, even
/// in a column it was ranked in before.
/// The order file is written only when an entry changes, so a board ranking
/// nothing keeps having no order file.
pub fn move_to(
    dir: &Utf8Path,
    formatting: &Formatting,
    id: TicketId,
    status: Status,
    at: Option<Placement>,
) -> Result<Moved, Error> {
    if matches!(at, Some(Placement::Next(_, other)) if other == id) {
        return Err(Error::SameTicket(id));
    }

    let _lock = lock(dir)?;
    let entries = store::list(dir)?;
    let from = status_of(&entries, id)?;
    if let Some(Placement::Next(_, other)) = at {
        let other_status = status_of(&entries, other)?;
        if other_status != status {
            return Err(Error::NotInColumn {
                other,
                other_status,
                status,
            });
        }
    }

    let mut order = read(dir)?;

    // The column as it stands once `id` has joined it.
    let column: Vec<TicketId> = entries
        .iter()
        .filter(|entry| {
            entry.id == id
                || entry
                    .ticket
                    .as_ref()
                    .is_ok_and(|ticket| ticket.metadata.status == status)
        })
        .map(|entry| entry.id)
        .collect();

    let others = || {
        order
            .arrange(status, &column)
            .into_iter()
            .filter(|other| *other != id)
    };
    let next_to = match at {
        None => None,
        Some(Placement::Next(place, other)) => Some((place, other)),
        Some(Placement::Top) => others().next().map(|first| (Place::Before, first)),
        Some(Placement::Bottom) => others().next_back().map(|last| (Place::After, last)),
    };

    let (path, previous) = store::set_status(dir, formatting, id, status)?;

    let mut changed = from != status && order.forget(id);
    if let Some((place, other)) = next_to {
        // Both tickets were just found at `status`, so the move always lands.
        changed |= order.rank(status, &column, id, place, other);
    }
    if changed {
        write(dir, &order)?;
    }

    Ok(Moved { path, previous })
}

/// Take the board's lock in `dir`, held until the returned file is dropped.
///
/// Waits for a writer holding it already.
/// A directory that doesn't exist holds no tickets to write, and so nothing to
/// lock: `None`.
fn lock(dir: &Utf8Path) -> Result<Option<File>, Error> {
    let file = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    file.lock()?;

    Ok(Some(file))
}

/// The status of the ticket `wanted` among `entries`.
///
/// A ticket that isn't there, or doesn't parse, is an error naming it.
fn status_of(entries: &[store::Entry], wanted: TicketId) -> Result<Status, Error> {
    let entry = entries
        .iter()
        .find(|entry| entry.id == wanted)
        .ok_or(Error::NoSuchTicket(wanted))?;

    entry
        .ticket
        .as_ref()
        .map(|ticket| ticket.metadata.status)
        .map_err(|error| Error::Malformed {
            id: wanted,
            error: error.clone(),
        })
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
