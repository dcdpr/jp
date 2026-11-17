//! Merge-queue ordering and front-entry state machine.
//!
//! Queue [`Entry`]s are built from open pull requests: either a standalone
//! labeled pull request, or a stack slice from the bottom of the stack up to
//! its highest labeled pull request (because GitHub merges a stacked pull
//! request together with all pull requests below it).
//!
//! [`assign`] computes each entry's [`QueueState`] and applies it to every
//! member pull request.
//! Eligible entries are ordered by their bottom pull request number (lowest
//! first), except that the current `front` entry retains the lead until it
//! merges or is held back.
//! Entries that are drafts, have failing checks, target a non-default base
//! branch, have merge conflicts, or match a [`Hold`] on their current head
//! commits are held back out of line.
//!
//! [`next`] decides the next action for the front entry: update it with the
//! default branch, wait on checks or mergeability, merge it, or place a
//! conflict hold.
//! This module does no I/O.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::model::{Checks, PullRequest};

/// State of a pull request in the merge queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueState {
    /// Waiting in line (`position` is 1-based, with `1` at the front).
    Waiting { position: usize },

    /// At the front of the queue, executing `Step`.
    Front(Step),

    /// Held back because the pull request (or a stack member) is a draft.
    Draft,

    /// Held back because a check failed; resumes when a new commit is pushed.
    Failing,

    /// Held back because the entry targets a branch other than the default
    /// branch.
    OtherBase(String),

    /// Held back due to merge conflicts (reported by GitHub or encountered
    /// during an update or rebase) until a head commit changes.
    Conflicts,

    /// Held back because an update, rebase, or merge failed with this reason,
    /// until a head commit changes.
    Blocked(String),
}

/// Active step for the entry at the front of the merge queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Updating or rebasing onto the default branch.
    Updating,

    /// Waiting for required or running CI checks to complete.
    WaitingForChecks,

    /// Waiting for GitHub to compute whether the entry is mergeable.
    Checking,

    /// Merge request submitted to GitHub.
    Merging,
}

impl Step {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Updating => "updating",
            Self::WaitingForChecks => "waiting for CI",
            Self::Checking => "checking",
            Self::Merging => "merging",
        }
    }
}

/// Hold recorded on a queue entry after a conflict or refused operation.
///
/// The hold remains active until one of the entry's head commits changes or the
/// user re-queues a member manually.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    /// [`Entry::signature`] that this hold applies to.
    pub sha: String,
    pub reason: Reason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reason {
    Conflicts,
    Blocked(String),
}

impl Reason {
    /// The queue state an entry held for this reason shows.
    #[must_use]
    pub fn state(&self) -> QueueState {
        match self {
            Self::Conflicts => QueueState::Conflicts,
            Self::Blocked(reason) => QueueState::Blocked(reason.clone()),
        }
    }
}

/// Role of a `dash` process in the repository's merge queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueRole {
    /// Holds the queue lock and drives updates and merges.
    Driving,

    /// Another `dash` process holds the queue lock for this repository.
    Watching,

    /// Paused by the user in this process.
    Paused,
}

/// Unit of work in the merge queue: a single pull request, or a contiguous
/// slice of a stack from its bottom up to its highest labeled pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry<'a> {
    /// Pull requests in the entry, ordered from bottom to top (never empty).
    pub members: Vec<&'a PullRequest>,
}

impl Entry<'_> {
    /// Number of the bottom pull request, used as the entry's stable key.
    #[must_use]
    pub fn key(&self) -> u64 {
        self.members[0].number
    }

    /// Top pull request in the entry (merging it merges the entire entry).
    #[must_use]
    pub fn top(&self) -> &PullRequest {
        self.members[self.members.len() - 1]
    }

    #[must_use]
    pub fn stacked(&self) -> bool {
        self.members[0].stack.is_some()
    }

    /// Space-separated head commit SHAs of all members from bottom to top.
    #[must_use]
    pub fn signature(&self) -> String {
        let shas: Vec<&str> = self.members.iter().map(|pr| pr.sha.as_str()).collect();
        shas.join(" ")
    }

    /// Display label: `#7` for a single pull request, or `#7..#9` for a stack.
    #[must_use]
    pub fn name(&self) -> String {
        match self.members.as_slice() {
            [only] => format!("#{}", only.number),
            _ => format!("#{}..#{}", self.key(), self.top().number),
        }
    }

    #[must_use]
    pub fn contains(&self, number: u64) -> bool {
        self.members.iter().any(|pr| pr.number == number)
    }

    /// Member pull request numbers from bottom to top.
    #[must_use]
    pub fn numbers(&self) -> Vec<u64> {
        self.members.iter().map(|pr| pr.number).collect()
    }
}

/// Build the merge-queue entries from `prs`, sorted by [`Entry::key`].
///
/// A stack forms an entry only if at least one of its pull requests carries the
/// queue label; the entry includes all members from the bottom up to the
/// highest labeled pull request.
#[must_use]
pub fn entries(prs: &[PullRequest]) -> Vec<Entry<'_>> {
    let mut entries = vec![];
    let mut stacks: BTreeMap<u64, Vec<&PullRequest>> = BTreeMap::new();

    for pr in prs {
        match &pr.stack {
            Some(place) => stacks.entry(place.stack).or_default().push(pr),
            None if pr.labeled => entries.push(Entry { members: vec![pr] }),
            None => {}
        }
    }
    for mut members in stacks.into_values() {
        members.sort_by_key(|pr| pr.stack.as_ref().map_or(0, |place| place.position));
        if let Some(top) = members.iter().rposition(|pr| pr.labeled) {
            members.truncate(top + 1);
            entries.push(Entry { members });
        }
    }

    entries.sort_by_key(Entry::key);
    entries
}

/// Assign [`PullRequest::queue`] for every pull request in `prs` and sort `prs`
/// by number.
///
/// Pull requests not belonging to any queue entry get `None`.
/// If the entry keyed `front` is still eligible, it stays at position `1`.
pub fn assign(prs: &mut [PullRequest], front: Option<u64>, holds: &HashMap<u64, Hold>) {
    prs.sort_by_key(|pr| pr.number);

    let mut states: HashMap<u64, QueueState> = HashMap::new();
    let mut line = vec![];
    for entry in entries(prs) {
        let members = entry.numbers();
        let Some(state) = held_back(&entry, holds) else {
            line.push((Some(entry.key()) != front, entry.key(), members));
            continue;
        };
        for number in members {
            states.insert(number, state.clone());
        }
    }

    line.sort_unstable();
    for (position, (_, _, members)) in line.into_iter().enumerate() {
        for number in members {
            states.insert(number, QueueState::Waiting {
                position: position + 1,
            });
        }
    }

    for pr in prs {
        pr.queue = states.remove(&pr.number);
    }
}

/// Why `entry` is held back from the line, or `None` if it is in line.
fn held_back(entry: &Entry<'_>, holds: &HashMap<u64, Hold>) -> Option<QueueState> {
    if entry.members.iter().any(|pr| pr.draft) {
        return Some(QueueState::Draft);
    }
    if let Some(base) = &entry.top().base {
        return Some(QueueState::OtherBase(base.clone()));
    }
    let hold = holds
        .get(&entry.key())
        .filter(|hold| hold.sha == entry.signature());
    if let Some(hold) = hold {
        return Some(hold.reason.state());
    }
    if entry.members.iter().any(|pr| pr.mergeable == Some(false)) {
        return Some(QueueState::Conflicts);
    }
    if entry.members.iter().any(|pr| pr.checks.failed > 0) {
        return Some(QueueState::Failing);
    }
    None
}

/// Count the queue entries waiting in line (including the front entry).
#[must_use]
pub fn waiting(prs: &[PullRequest]) -> usize {
    entries(prs)
        .iter()
        .filter(|entry| {
            matches!(
                entry.members[0].queue,
                Some(QueueState::Waiting { .. } | QueueState::Front(_))
            )
        })
        .count()
}

/// Return the entry at the front of the queue, if any.
#[must_use]
pub fn front(prs: &[PullRequest]) -> Option<Entry<'_>> {
    entries(prs).into_iter().find(|entry| {
        matches!(
            entry.members[0].queue,
            Some(QueueState::Front(_) | QueueState::Waiting { position: 1 })
        )
    })
}

/// Readiness snapshot for the entry at the front of the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// Combined check counts across all members, with unreported required
    /// checks counted as pending.
    pub checks: Checks,

    /// Total commits that members are behind their respective base branches.
    pub behind: u64,

    /// Combined mergeability across all members (`None` while GitHub is still
    /// computing it for any member).
    pub mergeable: Option<bool>,

    /// Whether the entry's head commits have had zero checks for long enough to
    /// conclude that no checks will run.
    pub quiet: bool,
}

/// Next action for the entry at the front of the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Update or rebase the entry onto the default branch.
    Update,

    /// Wait in `Step` without mutating the entry.
    Wait(Step),

    /// Merge the entry.
    Merge,

    /// Place a conflict hold on the entry.
    Conflicts,
}

/// Decide the next action for the front queue entry from its [`Status`].
#[must_use]
pub fn next(status: &Status) -> Next {
    let checks = status.checks;

    if status.mergeable == Some(false) {
        Next::Conflicts
    } else if status.behind > 0 {
        Next::Update
    } else if checks.pending > 0 || checks.failed > 0 || (checks.total() == 0 && !status.quiet) {
        Next::Wait(Step::WaitingForChecks)
    } else if status.mergeable.is_none() {
        Next::Wait(Step::Checking)
    } else {
        Next::Merge
    }
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
