//! `jp rfd rank`: move an RFD on the priority board.
//!
//! `--before OTHER` and `--after OTHER` put the RFD next to OTHER, in OTHER's
//! milestone group, so a backlog RFD ranked next to a planned one becomes
//! planned.
//! `--backlog` moves it to the backlog.
//!
//! The board keeps every RFD below the RFDs it requires or extends, the way the
//! site's board refuses a drop that would break that order, so a move that
//! would is refused here too.
//! Milestones are created, renamed, and removed on the site.

use std::collections::HashMap;

use crate::{
    Output,
    board::{self, Board, Entry},
    id::RfdId,
    paths::Paths,
    priority::{self, Priority, Slot},
};

/// Where to put an RFD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// Directly above another.
    Before(RfdId),

    /// Directly below another.
    After(RfdId),

    /// In the backlog.
    Backlog,
}

/// Move `id` on the board.
pub(crate) fn rank(paths: &Paths, id: RfdId, place: Place) -> Result<Output, String> {
    let board::Loaded { board, .. } = board::load(&paths.root, &paths.rfds, &paths.tickets)?;
    let entry = entry(paths, &board, id)?;
    if entry.is_terminal() {
        return Err(format!(
            "RFD {id} is {}, and the board ranks open work only.",
            entry.status.as_deref().unwrap_or_default()
        ));
    }

    let mut priority = priority::read_for_update(&paths.rfds)?.unwrap_or_default();
    let this = id.to_string();

    let slot = match place {
        Place::Before(other) | Place::After(other) => {
            if other == id {
                return Err(format!("RFD {id} can't be ranked against itself."));
            }
            let other_entry = self::entry(paths, &board, other)?;
            if other_entry.is_terminal() {
                return Err(format!(
                    "RFD {other} is {}, and holds no place on the board.",
                    other_entry.status.as_deref().unwrap_or_default()
                ));
            }

            priority.remove(&[&this]);
            let anchor = priority.find(&other.to_string()).ok_or_else(|| {
                format!(
                    "RFD {other} is not on the board. Place it first, with `jp rfd rank {other} \
                     --backlog`."
                )
            })?;

            match (place, anchor) {
                (Place::After(_), Slot::Planned { group, index }) => Slot::Planned {
                    group,
                    index: index + 1,
                },
                (Place::After(_), Slot::Backlog(index)) => Slot::Backlog(index + 1),
                _ => anchor,
            }
        }
        Place::Backlog => {
            priority.remove(&[&this]);
            // Below the RFDs it depends on that sit in the backlog too, which
            // otherwise the top of the backlog would put it above.
            let below = priority
                .backlog
                .iter()
                .rposition(|placed| entry.depends_on.contains(placed))
                .map_or(0, |index| index + 1);
            Slot::Backlog(below)
        }
    };

    priority.insert(slot, &this);
    check_order(&board, &priority, entry)?;
    priority::write(&paths.rfds, &priority)?;

    let group = match slot {
        Slot::Planned { group, .. } => match &priority.planned[group].milestone {
            Some(milestone) => format!("in the {milestone} milestone"),
            None => "among the planned RFDs".to_owned(),
        },
        Slot::Backlog(_) => "in the backlog".to_owned(),
    };

    Ok(match place {
        Place::Before(other) => format!("RFD {id} now sits above RFD {other}, {group}.\n"),
        Place::After(other) => format!("RFD {id} now sits below RFD {other}, {group}.\n"),
        Place::Backlog => format!("RFD {id} is in the backlog.\n"),
    }
    .into())
}

/// The board's entry for `id`.
fn entry<'a>(paths: &Paths, board: &'a Board, id: RfdId) -> Result<&'a Entry, String> {
    let file = paths.resolve(id)?;

    board
        .entry(&paths.show(&file))
        .ok_or_else(|| format!("{} is not an RFD: its name has no slug.", paths.show(&file)))
}

/// Refuse an order that puts `moved` above an RFD it depends on, or below one
/// that depends on it.
///
/// Positions are read the way the site's board lays the file out: the planned
/// RFDs, then the backlog, then every open RFD the file doesn't name.
/// RFDs that are done hold no place and constrain nothing.
fn check_order(board: &Board, priority: &Priority, moved: &Entry) -> Result<(), String> {
    let open: Vec<&Entry> = board
        .entries
        .iter()
        .filter(|entry| !entry.is_terminal())
        .collect();

    let mut positions: HashMap<&str, usize> = HashMap::new();
    for id in priority.placed() {
        let next = positions.len();
        positions.entry(id).or_insert(next);
    }
    let mut unplaced: Vec<&str> = open
        .iter()
        .map(|entry| entry.num.as_str())
        .filter(|num| !positions.contains_key(num))
        .collect();
    unplaced.sort_unstable();
    for num in unplaced {
        let next = positions.len();
        positions.insert(num, next);
    }

    let position = |num: &str| positions.get(num).copied();
    let Some(here) = position(&moved.num) else {
        return Ok(());
    };

    for dependency in &moved.depends_on {
        let open_dependency = open.iter().any(|entry| entry.num == *dependency);
        if open_dependency && position(dependency).is_some_and(|there| there > here) {
            return Err(format!(
                "RFD {} depends on RFD {dependency}, which would sit below it. Rank RFD \
                 {dependency} above it first.",
                moved.num
            ));
        }
    }

    for dependent in &open {
        if dependent.depends_on.contains(&moved.num)
            && position(&dependent.num).is_some_and(|there| there < here)
        {
            return Err(format!(
                "RFD {} depends on RFD {}, which would sit below it. Rank RFD {} below it first.",
                dependent.num, moved.num, dependent.num
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "rank_tests.rs"]
mod tests;
