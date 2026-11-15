//! The orders `jp ticket list --order` and `ticket_list`'s `order` accept.
//!
//! An order is `board`, `filed`, or a label key the board can sort by (a `date`
//! key or an `ordered` one, held to one value per ticket), and `filed` and a
//! key take `:desc` to reverse them.
//! [`names`] lists every order a vocabulary makes possible, which is what a
//! refusal names and what `jp ticket tools sync` offers the assistant.

use std::cmp::Ordering;

use ticket::{Direction, SortKey, Ticket, TicketId, Vocabulary};

/// Each column of the board in its order.
pub(crate) const BOARD: &str = "board";

/// By the date each ticket was filed.
const FILED: &str = "filed";

const DESCENDING: &str = "desc";
const ASCENDING: &str = "asc";

/// How a listing orders its tickets.
pub(crate) enum ListOrder<'v> {
    /// Todo, In Progress, then Done, each column in the board's order.
    Board,

    /// Across every column, by `Sort`.
    Sorted(Sort<'v>),
}

/// An order across every column.
pub(crate) enum Sort<'v> {
    /// By the date each ticket was filed, then by id.
    Filed(Direction),

    /// By the value each ticket carries under a label key, tickets without one
    /// last, then by id.
    Label(SortKey<'v>, Direction),
}

impl<'v> ListOrder<'v> {
    /// Read an order as [`names`] spells it, `:asc` also accepted, or `None`
    /// when `vocabulary` makes no such order.
    ///
    /// A built-in order wins over a label key of the same name.
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
    /// Compare two tickets, each with its id.
    pub(crate) fn compare(&self, a: (TicketId, &Ticket), b: (TicketId, &Ticket)) -> Ordering {
        match self {
            Self::Filed(Direction::Ascending) => filed(a).cmp(&filed(b)),
            Self::Filed(Direction::Descending) => filed(b).cmp(&filed(a)),
            // Ties fall back to filing order, oldest first, whichever the
            // direction: the label decides, and the id only keeps the result
            // stable.
            Self::Label(key, direction) => key
                .compare(&a.1.metadata.labels, &b.1.metadata.labels, *direction)
                .then_with(|| a.0.cmp(&b.0)),
        }
    }
}

/// A ticket's place in filing order: its date, then its id, which counts up as
/// tickets are filed.
fn filed((id, ticket): (TicketId, &Ticket)) -> (&str, TicketId) {
    (&ticket.metadata.date, id)
}

/// Every order `vocabulary` makes possible, as [`ListOrder::parse`] reads them:
/// `board`, `filed`, and each key the board can sort by, each but `board` also
/// with `:desc`.
///
/// A key named like a built-in order is left out, since the built-in wins.
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

/// The refusal for an order `vocabulary` doesn't make, naming the argument as
/// the caller spells it, such as `order` or `--order`.
pub(crate) fn refusal(argument: &str, vocabulary: &Vocabulary) -> String {
    format!(
        "`{argument}` must be one of: {}.",
        names(vocabulary).join(", ")
    )
}
