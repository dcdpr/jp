//! Sort orders accepted by `jp ticket list --order` and `ticket_list`'s `order`
//! parameter.
//!
//! Valid orders are `board`, `filed`, or any sortable label key in the board's
//! vocabulary (`max = 1` with `type = "date"` or `ordered = true`).
//! Both `filed` and label keys accept a `:desc` suffix (or `:asc`) to control
//! sort direction.
//! [`names`] returns the list of valid orders for a vocabulary, used in error
//! messages and synced into the `ticket_list` tool schema.

use std::cmp::Ordering;

use ticket::{Direction, SortKey, Ticket, TicketId, Vocabulary};

pub(crate) const BOARD: &str = "board";
const FILED: &str = "filed";

const DESCENDING: &str = "desc";
const ASCENDING: &str = "asc";

/// Ticket listing order.
pub(crate) enum ListOrder<'v> {
    /// Group by status (Todo, In Progress, Done) and order each column by the
    /// board's ranking.
    Board,

    /// Sort across all statuses by `Sort`.
    Sorted(Sort<'v>),
}

/// Cross-column sort criterion.
pub(crate) enum Sort<'v> {
    /// Sort by filing date, breaking ties by ticket ID.
    Filed(Direction),

    /// Sort by a label key's value, placing tickets without the label last and
    /// breaking ties by filing order.
    Label(SortKey<'v>, Direction),
}

impl<'v> ListOrder<'v> {
    /// Parse an order specifier (one of [`names`], or `<key>:asc`), returning
    /// `None` if the order is unknown or its label key is not sortable in
    /// `vocabulary`.
    ///
    /// Built-in order names (`board`, `filed`) take precedence over label keys
    /// with the same name.
    pub(crate) fn parse(text: &str, vocabulary: &'v Vocabulary) -> Option<Self> {
        let text = text.trim();
        if text == BOARD {
            return Some(Self::Board);
        }

        let (name, direction) = match text.split_once(':') {
            None => (text, Direction::Ascending),
            Some((name, ASCENDING)) => (name, Direction::Ascending),
            Some((name, DESCENDING)) => (name, Direction::Descending),
            Some(_) => return None,
        };
        let sort = match name {
            BOARD => return None,
            FILED => Sort::Filed(direction),
            key => Sort::Label(vocabulary.sort_key(key)?, direction),
        };

        Some(Self::Sorted(sort))
    }
}

impl Sort<'_> {
    pub(crate) fn compare(&self, a: (TicketId, &Ticket), b: (TicketId, &Ticket)) -> Ordering {
        match self {
            Self::Filed(Direction::Ascending) => filed(a).cmp(&filed(b)),
            Self::Filed(Direction::Descending) => filed(b).cmp(&filed(a)),
            // Break ties by filing order, oldest first, in both directions.
            // The date comes before the ID because an imported ticket keeps
            // its original date but is allocated a newer ID.
            Self::Label(key, direction) => key
                .compare(&a.1.metadata.labels, &b.1.metadata.labels, *direction)
                .then_with(|| filed(a).cmp(&filed(b))),
        }
    }
}

/// Sort key for filing order: filing date, then ticket ID.
fn filed((id, ticket): (TicketId, &Ticket)) -> (&str, TicketId) {
    (&ticket.metadata.date, id)
}

/// Return all valid order specifiers for `vocabulary`: `board`, `filed`,
/// `filed:desc`, and `<key>` / `<key>:desc` for each sortable label key.
///
/// Label keys matching a built-in order name (`board`, `filed`) are omitted.
pub(crate) fn names(vocabulary: &Vocabulary) -> Vec<String> {
    let keys = vocabulary
        .sortable_keys()
        .filter(|key| ![BOARD, FILED].contains(key));

    let mut names = vec![BOARD.to_owned()];
    for name in [FILED].into_iter().chain(keys) {
        names.push(name.to_owned());
        names.push(format!("{name}:{DESCENDING}"));
    }

    names
}

/// Format the error message for an invalid order argument (e.g. `order` or
/// `--order`), listing all valid orders for `vocabulary`.
pub(crate) fn refusal(argument: &str, vocabulary: &Vocabulary) -> String {
    format!(
        "`{argument}` must be one of: {}.",
        names(vocabulary).join(", ")
    )
}
