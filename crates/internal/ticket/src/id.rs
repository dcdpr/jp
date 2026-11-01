//! Ticket identifiers.
//!
//! A ticket id is an epoch id with the `T` prefix, as in `T-02wt0kx`: seven
//! characters, five counting five-second buckets since the ticket directory's
//! epoch and two separating ids in one bucket.
//! The format and its allocation are `doc::id`'s and `doc::files`', for any
//! kind of document named this way; [`TicketKind`] is the ticket's.

use doc::id::{Id, Kind};

/// The kind of document a [`TicketId`] names.
#[derive(Debug)]
pub enum TicketKind {}

impl Kind for TicketKind {
    const PREFIX: char = 'T';
    const NOUN: &'static str = "ticket";
    const WRITER: &'static str = "JP-TICKET";
}

/// A ticket id.
///
/// The canonical form is `T-02wt0kx`.
/// Input additionally accepts `T02wt0kx`, the bare body, and any case.
pub type TicketId = Id<TicketKind>;

#[cfg(test)]
#[path = "id_tests.rs"]
mod tests;
