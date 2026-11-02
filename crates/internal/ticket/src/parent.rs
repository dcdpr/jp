//! The parent relation: a ticket that groups the tickets filed under it.
//!
//! A child names its parent in its own `Parent` field, and a parent names
//! nothing, so its children are every ticket naming it.
//! Filing a child writes the child's file alone.
//!
//! The relation is one level deep, and a parent outlives its work:
//!
//! - a ticket's parent is another ticket that exists;
//! - a parent has no parent of its own;
//! - a Done parent has no open children, so closing a parent waits on its
//!   children and nothing closes one on their behalf.
//!
//! [`check`] holds one change to these rules and [`check_delete`] a deletion;
//! [`problems`] finds every ticket breaking them, for a directory edited by
//! hand.
//! Everything here is pure: callers pass the tickets they read.

use std::fmt;

use crate::{Status, Ticket, TicketId};

/// What the rules need to know about one ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member {
    pub id: TicketId,
    pub status: Status,
    pub parent: Option<TicketId>,
}

impl Member {
    /// The member `ticket`, filed under `id`, is.
    #[must_use]
    pub fn of(id: TicketId, ticket: &Ticket) -> Self {
        Self {
            id,
            status: ticket.metadata.status,
            parent: ticket.metadata.parent,
        }
    }
}

/// A change the rules refuse, or a ticket that breaks them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A ticket named as its own parent.
    Itself(TicketId),

    /// The parent named is not a ticket.
    NoSuchParent {
        id: Option<TicketId>,
        parent: TicketId,
    },

    /// The parent named sits under a parent of its own.
    Nested {
        parent: TicketId,
        grandparent: TicketId,
    },

    /// A ticket with children, given a parent.
    HasChildren {
        id: TicketId,
        children: Vec<TicketId>,
    },

    /// An open ticket under a Done parent.
    ClosedParent {
        id: Option<TicketId>,
        parent: TicketId,
    },

    /// A Done ticket with open children.
    OpenChildren {
        id: TicketId,
        children: Vec<TicketId>,
    },

    /// A ticket with children, deleted.
    Orphans {
        id: TicketId,
        children: Vec<TicketId>,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Itself(id) => write!(f, "{id} can't be its own parent."),
            Self::NoSuchParent {
                id: Some(id),
                parent,
            } => {
                write!(
                    f,
                    "{id} names {parent} as its parent, and there is no ticket {parent}."
                )
            }
            Self::NoSuchParent { id: None, parent } => {
                write!(f, "No ticket {parent} to file under.")
            }
            Self::Nested {
                parent,
                grandparent,
            } => write!(
                f,
                "{parent} sits under {grandparent}, and a ticket under a parent can't be a parent \
                 itself. File under {grandparent} instead."
            ),
            Self::HasChildren { id, children } => write!(
                f,
                "{id} has children ({}), and a parent can't sit under another ticket.",
                join(children)
            ),
            Self::ClosedParent {
                id: Some(id),
                parent,
            } => write!(
                f,
                "{id} would be open under {parent}, which is Done. Reopen {parent} first."
            ),
            Self::ClosedParent { id: None, parent } => write!(
                f,
                "{parent} is Done, so no open ticket can be filed under it. Reopen {parent} first."
            ),
            Self::OpenChildren { id, children } => write!(
                f,
                "{id} has open children: {}. Close them, or move them out from under {id}, first.",
                join(children)
            ),
            Self::Orphans { id, children } => write!(
                f,
                "{id} has children: {}. Give them another parent, or none, before deleting it.",
                join(children)
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// The children of `id` among `members`, in the order `members` holds them.
#[must_use]
pub fn children(members: &[Member], id: TicketId) -> Vec<TicketId> {
    members
        .iter()
        .filter(|member| member.parent == Some(id) && member.id != id)
        .map(|member| member.id)
        .collect()
}

/// Whether the ticket `id`, at `status` and under `parent`, keeps the rules.
///
/// `members` is every ticket as it stands, `id`'s current state included; `id`
/// is `None` for a ticket not filed yet.
/// Only what the change touches is checked, so a ticket broken by hand can
/// still be moved: a parent is held to the rules when it changes, and the
/// statuses of a ticket and its relatives when it opens or closes.
///
/// # Errors
///
/// Returns the first rule the change breaks.
pub fn check(
    members: &[Member],
    id: Option<TicketId>,
    status: Status,
    parent: Option<TicketId>,
) -> Result<(), Refusal> {
    let current = id.and_then(|id| members.iter().find(|member| member.id == id));
    let reparented = current.is_none_or(|current| current.parent != parent);
    let opened = status != Status::Done && current.is_none_or(|c| c.status == Status::Done);
    let closed = status == Status::Done && current.is_none_or(|c| c.status != Status::Done);

    if let Some(parent) = parent
        && (reparented || opened)
    {
        if id == Some(parent) {
            return Err(Refusal::Itself(parent));
        }
        let Some(found) = members.iter().find(|member| member.id == parent) else {
            return Err(Refusal::NoSuchParent { id, parent });
        };
        if reparented {
            if let Some(grandparent) = found.parent {
                return Err(Refusal::Nested {
                    parent,
                    grandparent,
                });
            }
            if let Some(id) = id {
                let children = children(members, id);
                if !children.is_empty() {
                    return Err(Refusal::HasChildren { id, children });
                }
            }
        }
        if status != Status::Done && found.status == Status::Done {
            return Err(Refusal::ClosedParent { id, parent });
        }
    }

    if closed && let Some(id) = id {
        let open = open_children(members, id);
        if !open.is_empty() {
            return Err(Refusal::OpenChildren { id, children: open });
        }
    }

    Ok(())
}

/// Whether the ticket `id` can be deleted without leaving children behind.
///
/// # Errors
///
/// Returns [`Refusal::Orphans`], naming the children.
pub fn check_delete(members: &[Member], id: TicketId) -> Result<(), Refusal> {
    let children = children(members, id);
    if children.is_empty() {
        return Ok(());
    }

    Err(Refusal::Orphans { id, children })
}

/// Every ticket among `members` that breaks the rules, with how.
///
/// A child is reported for its parent: one naming itself, a ticket that isn't
/// there, or a parent under another.
/// A Done parent is reported once for all of its open children.
#[must_use]
pub fn problems(members: &[Member]) -> Vec<(TicketId, Refusal)> {
    let mut found = vec![];
    for member in members {
        if let Some(parent) = member.parent {
            let refusal = if parent == member.id {
                Some(Refusal::Itself(parent))
            } else {
                match members.iter().find(|other| other.id == parent) {
                    None => Some(Refusal::NoSuchParent {
                        id: Some(member.id),
                        parent,
                    }),
                    Some(other) => other.parent.map(|grandparent| Refusal::Nested {
                        parent,
                        grandparent,
                    }),
                }
            };
            if let Some(refusal) = refusal {
                found.push((member.id, refusal));
            }
        }

        if member.status == Status::Done {
            let open = open_children(members, member.id);
            if !open.is_empty() {
                found.push((member.id, Refusal::OpenChildren {
                    id: member.id,
                    children: open,
                }));
            }
        }
    }

    found
}

/// The children of `id` that aren't Done.
fn open_children(members: &[Member], id: TicketId) -> Vec<TicketId> {
    members
        .iter()
        .filter(|member| {
            member.parent == Some(id) && member.id != id && member.status != Status::Done
        })
        .map(|member| member.id)
        .collect()
}

fn join(ids: &[TicketId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "parent_tests.rs"]
mod tests;
