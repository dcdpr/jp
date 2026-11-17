//! Merge-queue order, and the next step for the entry at its front.
//!
//! The queue is made of [`Entry`]s: a labeled pull request outside a stack, or
//! a stack's pull requests from the bottom up to its highest labeled one, since
//! GitHub merges a stacked pull request together with everything below it.
//!
//! [`assign`] decides each entry's place: in line, at the front, or held back
//! as a draft, for failing checks, for targeting a branch other than the
//! repository's default branch, or for a [`Hold`] the queue placed on its head
//! commits.
//! Every pull request in an entry gets the entry's state.
//! Entries in line are ordered by their bottom pull request's number, lowest
//! first, except that the current front stays first until it merges or drops
//! out.
//!
//! [`next`] decides what the queue does with the front: bring it up to date,
//! wait, merge it, or hold it for conflicts.
//! This module does no I/O.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::model::{Checks, PullRequest};

/// Where a labeled pull request stands in the merge queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueState {
    /// In line to be merged, `position` counting from 1 for the next one.
    Waiting { position: usize },

    /// At the front of the queue, being moved toward a merge.
    Front(Step),

    /// A draft, which the queue doesn't merge.
    Draft,

    /// Has a failing check, so it waits for a push that fixes it.
    Failing,

    /// Merges into this branch rather than the repository's default branch.
    OtherBase(String),

    /// Conflicts with its base: GitHub reports a member it can't merge, or the
    /// queue's update or rebase hit a conflict, until a head commit changes.
    Conflicts,

    /// The queue's update or merge failed with this reason, until its head
    /// commit changes.
    Blocked(String),
}

/// What the queue is doing with the pull request at its front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The default branch is being merged into it.
    Updating,

    /// Its checks are still running, or haven't reported yet.
    WaitingForChecks,

    /// GitHub hasn't finished working out whether it can merge.
    Checking,

    /// The merge was requested.
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

/// An entry the queue stopped trying to merge, until one of its head commits
/// changes or it is put back in the queue by hand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    /// The entry's [`signature`] the hold applies to.
    ///
    /// [`signature`]: Entry::signature
    pub sha: String,
    pub reason: Reason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reason {
    Conflicts,
    Blocked(String),
}

/// Which `dash` process acts on the repository's merge queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueRole {
    /// This process updates and merges pull requests.
    Driving,

    /// Another `dash` process in the same repository drives the queue.
    Watching,

    /// The queue is paused in this process.
    Paused,
}

/// Pull requests the queue merges together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry<'a> {
    /// Bottom of the stack first; a single pull request outside a stack.
    /// Never empty.
    pub members: Vec<&'a PullRequest>,
}

impl Entry<'_> {
    /// The bottom pull request's number, which identifies the entry.
    #[must_use]
    pub fn key(&self) -> u64 {
        self.members[0].number
    }

    /// The highest pull request, the one merging takes everything up to.
    #[must_use]
    pub fn top(&self) -> &PullRequest {
        self.members[self.members.len() - 1]
    }

    #[must_use]
    pub fn stacked(&self) -> bool {
        self.members[0].stack.is_some()
    }

    /// Every member's head commit, bottom first: a change to any of them is a
    /// new attempt at the entry.
    #[must_use]
    pub fn signature(&self) -> String {
        let shas: Vec<&str> = self.members.iter().map(|pr| pr.sha.as_str()).collect();
        shas.join(" ")
    }

    /// `#7`, or `#7..#9` for several pull requests.
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

    /// The members' numbers, bottom first.
    #[must_use]
    pub fn numbers(&self) -> Vec<u64> {
        self.members.iter().map(|pr| pr.number).collect()
    }
}

/// The merge-queue entries in `prs`, by key.
///
/// A stack only forms an entry when one of its pull requests carries the queue
/// label, and the entry stops at the highest labeled one.
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

/// Set [`PullRequest::queue`] on every pull request in `prs`, and sort them by
/// number.
///
/// A pull request in no entry gets `None`.
/// The entry keyed `front` stays first in line while it is still eligible.
pub fn assign(prs: &mut [PullRequest], front: Option<u64>, holds: &HashMap<u64, Hold>) {
    prs.sort_by_key(|pr| pr.number);

    let mut states: HashMap<u64, QueueState> = HashMap::new();
    let mut line = vec![];
    for entry in entries(prs) {
        let members = entry.numbers();
        let hold = holds
            .get(&entry.key())
            .filter(|hold| hold.sha == entry.signature());

        let state = if entry.members.iter().any(|pr| pr.draft) {
            QueueState::Draft
        } else if let Some(base) = &entry.top().base {
            QueueState::OtherBase(base.clone())
        } else if let Some(hold) = hold {
            match &hold.reason {
                Reason::Conflicts => QueueState::Conflicts,
                Reason::Blocked(reason) => QueueState::Blocked(reason.clone()),
            }
        } else if entry.members.iter().any(|pr| pr.mergeable == Some(false)) {
            QueueState::Conflicts
        } else if entry.members.iter().any(|pr| pr.checks.failed > 0) {
            QueueState::Failing
        } else {
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

/// The number of entries in line in `prs`, the front included.
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

/// The entry at the front of the queue, if any.
#[must_use]
pub fn front(prs: &[PullRequest]) -> Option<Entry<'_>> {
    entries(prs).into_iter().find(|entry| {
        matches!(
            entry.members[0].queue,
            Some(QueueState::Front(_) | QueueState::Waiting { position: 1 })
        )
    })
}

/// What the queue knows about the entry at its front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// Its checks, with required checks that haven't reported counted as
    /// pending.
    pub checks: Checks,

    /// Commits on the default branch it doesn't have yet.
    pub behind: u64,

    /// GitHub's answer to whether it can merge; `None` while GitHub works it
    /// out.
    pub mergeable: Option<bool>,

    /// Whether its head commit has had no checks at all for long enough to
    /// assume none are coming.
    pub quiet: bool,
}

/// What the queue does next with the pull request at its front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Merge the default branch into it.
    Update,

    /// Nothing to do yet.
    Wait(Step),

    /// Merge it.
    Merge,

    /// Hold it: it conflicts with the default branch.
    Conflicts,
}

/// Decide the next step for the pull request at the front of the queue.
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
