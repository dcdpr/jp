//! Data model for dashboard rows and source snapshots.
//!
//! Every source reports paths or branch names; [`rows`] attaches each report to
//! the git worktree it belongs to.
//! This module does no I/O.

use std::collections::HashSet;

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};

use crate::queue::QueueState;

/// A git worktree of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: Utf8PathBuf,

    /// Checked-out branch name without `refs/heads/`, or `None` when `HEAD` is
    /// detached.
    pub branch: Option<String>,
}

/// A terminal surface open in the terminal emulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    /// Emulator-assigned surface identifier used to focus the tab.
    pub id: String,

    /// Working directory last reported by the surface's shell.
    pub cwd: Utf8PathBuf,

    /// Title reported by the terminal surface.
    pub name: String,

    /// Short TTY device name matching `ps` output (e.g. `ttys001`), if reported
    /// by Ghostty.
    pub tty: Option<String>,
}

/// A `jp` process holding a conversation lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpRun {
    pub pid: u32,
    pub cwd: Utf8PathBuf,

    /// Locked conversation's title, or its ID when untitled.
    pub conversation: String,

    /// Timestamp of the earliest unanswered inquiry in the current turn, if
    /// any.
    pub waiting_since: Option<DateTime<Utc>>,
}

/// Most recent conversation observed running in a working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpRecent {
    pub cwd: Utf8PathBuf,

    /// Timestamp of the conversation's last recorded event, or `None` if the
    /// conversation is empty or its metadata cannot be read.
    pub last_event_at: Option<DateTime<Utc>>,
}

/// A shell's JP session and its conversation history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpSession {
    /// Session leader PID used to identify the session.
    pub pid: u32,

    /// Interactive shell PID (either `pid` itself or its child when spawned by
    /// `login`).
    pub shell: u32,

    /// Whether the shell is a descendant of the Ghostty process.
    pub ghostty: bool,

    /// Short TTY device name matching `ps` output (e.g. `ttys001`).
    pub tty: Option<String>,

    pub cwd: Utf8PathBuf,

    /// Session conversations, ordered with the active conversation first and
    /// then by most recent activity.
    pub conversations: Vec<SessionConversation>,

    /// Number of older conversations in the session omitted after
    /// `conversations`.
    pub more: usize,
}

impl JpSession {
    #[must_use]
    pub fn active(&self) -> Option<&SessionConversation> {
        self.conversations
            .iter()
            .find(|conversation| conversation.active)
    }
}

/// Conversation entry in a session's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConversation {
    pub id: String,
    pub title: Option<String>,
    pub active: bool,
    pub pinned: bool,
    pub events: usize,

    /// Timestamp of the last recorded event, or `None` if the conversation is
    /// empty.
    pub last_event_at: Option<DateTime<Utc>>,

    pub status: Status,
}

/// Execution state of a conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    /// No `jp` process holds the conversation lock.
    #[default]
    Idle,

    /// A `jp` process holds the conversation lock.
    Running,

    /// A `jp` process holds the lock and is waiting for a response to an
    /// inquiry.
    Waiting { since: DateTime<Utc> },
}

impl Status {
    #[must_use]
    pub const fn held(waiting_since: Option<DateTime<Utc>>) -> Self {
        match waiting_since {
            Some(since) => Self::Waiting { since },
            None => Self::Running,
        }
    }
}

/// Outcome of fast-forwarding the local default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUpdate {
    /// Name of the default branch (e.g. `main`).
    pub branch: String,

    /// Whether the branch ref advanced.
    pub moved: bool,
}

/// An open pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub url: String,

    /// Local branch name used by `gh pr checkout`: the PR's head ref, or
    /// `<owner>/<branch>` when a fork's head ref collides with the base
    /// repository's default branch.
    pub head: String,

    /// The head commit.
    pub sha: String,
    pub draft: bool,
    pub checks: Checks,

    /// Total review threads on the pull request, including pending reviews.
    pub threads: u32,

    /// Number of sampled review threads that have at least one reply.
    pub answered: u32,

    /// Whether the pull request carries the merge-queue label.
    pub labeled: bool,

    /// The branch it merges into, when that isn't the repository's default
    /// branch: for a stacked pull request, the branch the whole stack merges
    /// into.
    pub base: Option<String>,

    /// Where it sits in a GitHub stack of pull requests, if it is in one.
    pub stack: Option<StackPlace>,

    /// The head branch on GitHub, when it lives in the repository itself;
    /// `None` for a fork's branch, which the queue can't delete.
    pub remote_branch: Option<String>,

    /// GitHub's answer to whether it merges into its base without conflicts;
    /// `None` while GitHub works it out, and for pull requests in no queue
    /// entry, which aren't asked.
    pub mergeable: Option<bool>,

    /// Where it stands in the merge queue, set by [`queue::assign`] and updated
    /// by the queue's driver; `None` without the queue label.
    ///
    /// [`queue::assign`]: crate::queue::assign
    pub queue: Option<QueueState>,
}

/// A pull request's place in a GitHub stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackPlace {
    /// The stack's number.
    pub stack: u64,

    /// Position in the stack, counting from 1 at the bottom.
    pub position: usize,

    /// Number of pull requests in the stack.
    pub size: usize,

    /// The pull request's branch on GitHub.
    pub branch: String,

    /// Head commit of the pull request below it in the stack; `None` at the
    /// bottom.
    pub below: Option<String>,
}

/// CI check counts for a pull request's head commit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Checks {
    pub passed: u32,
    pub pending: u32,
    pub failed: u32,
}

impl Checks {
    #[must_use]
    pub const fn total(self) -> u32 {
        self.passed + self.pending + self.failed
    }
}

/// Stable identifier for a table row across refreshes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RowId {
    /// A worktree, by its root directory.
    Worktree(Utf8PathBuf),

    /// An open pull request with no local worktree, identified by PR number.
    PullRequest(u64),
}

/// One worktree with everything the sources attached to it, or an open pull
/// request that has no worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: RowId,

    /// The worktree directory's name.
    /// Empty for a pull request without a worktree.
    pub name: String,

    /// The worktree's branch, or the pull request's head branch.
    pub branch: Option<String>,

    /// Terminals whose working directory is inside the worktree, in the order
    /// the terminal emulator lists them.
    pub terminals: Vec<Terminal>,

    /// `jp` processes running inside the worktree.
    pub runs: Vec<JpRun>,

    /// JP sessions of shells inside the worktree.
    pub sessions: Vec<JpSession>,

    /// Timestamp of the last event on the conversation most recently run in the
    /// worktree.
    pub last_run: Option<DateTime<Utc>>,

    /// The open pull request whose head is the worktree's branch.
    /// Always set for a pull request without a worktree.
    pub pr: Option<PullRequest>,
}

impl Row {
    /// Build a row for `worktree`, attaching the matching pull request from
    /// `prs` if one shares its branch.
    fn for_worktree(worktree: &Worktree, prs: &[PullRequest]) -> Self {
        Self {
            name: worktree
                .path
                .file_name()
                .unwrap_or(worktree.path.as_str())
                .to_owned(),
            id: RowId::Worktree(worktree.path.clone()),
            branch: worktree.branch.clone(),
            terminals: vec![],
            runs: vec![],
            sessions: vec![],
            last_run: None,
            pr: worktree
                .branch
                .as_deref()
                .and_then(|branch| prs.iter().find(|pr| pr.head == branch))
                .cloned(),
        }
    }

    /// Build a row for an open pull request that has no local worktree.
    fn for_pull_request(pr: &PullRequest) -> Self {
        Self {
            id: RowId::PullRequest(pr.number),
            name: String::new(),
            branch: Some(pr.head.clone()),
            terminals: vec![],
            runs: vec![],
            sessions: vec![],
            last_run: None,
            pr: Some(pr.clone()),
        }
    }

    /// Return `true` if `tab` unambiguously hosts `session`'s shell.
    ///
    /// When Ghostty reports the tab's TTY, matches by TTY device.
    /// Otherwise, falls back to matching by working directory only when `tab`
    /// is the sole tab in that directory and `session` is the sole Ghostty
    /// shell there.
    #[must_use]
    pub fn owns(&self, tab: &Terminal, session: &JpSession) -> bool {
        if tab.tty.is_some() {
            return tab.tty == session.tty;
        }

        let tabs = self
            .terminals
            .iter()
            .filter(|other| other.cwd == tab.cwd)
            .count();
        let shells = self
            .sessions
            .iter()
            .filter(|other| other.ghostty && other.cwd == tab.cwd)
            .count();

        session.ghostty && session.cwd == tab.cwd && tabs == 1 && shells == 1
    }

    /// Return `true` if this row represents a local worktree rather than an
    /// unchecked-out pull request.
    #[must_use]
    pub const fn is_worktree(&self) -> bool {
        matches!(self.id, RowId::Worktree(_))
    }

    /// Return the latest event timestamp across the worktree's sessions and its
    /// most recent run.
    #[must_use]
    pub fn last_activity(&self) -> Option<DateTime<Utc>> {
        self.sessions
            .iter()
            .flat_map(|session| &session.conversations)
            .filter_map(|conversation| conversation.last_event_at)
            .chain(self.last_run)
            .max()
    }

    /// Return `true` if the row has an open terminal, a running `jp` process,
    /// or an open pull request.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.terminals.is_empty() || !self.runs.is_empty() || self.pr.is_some()
    }
}

/// Join source snapshots into one [`Row`] per worktree (sorted by worktree
/// name), followed by one [`Row`] per open pull request without a local
/// worktree (sorted by PR number).
///
/// Terminals, running `jp` processes, sessions, and recent runs are assigned to
/// the worktree whose path is the longest prefix of their working directory.
/// Entries outside every worktree are ignored.
#[must_use]
pub fn rows(
    worktrees: &[Worktree],
    terminals: &[Terminal],
    runs: &[JpRun],
    sessions: &[JpSession],
    recent: &[JpRecent],
    prs: &[PullRequest],
) -> Vec<Row> {
    let mut rows: Vec<Row> = worktrees
        .iter()
        .map(|worktree| Row::for_worktree(worktree, prs))
        .collect();

    for terminal in terminals {
        if let Some(index) = owner(&terminal.cwd, worktrees) {
            rows[index].terminals.push(terminal.clone());
        }
    }

    for run in runs {
        if let Some(index) = owner(&run.cwd, worktrees) {
            rows[index].runs.push(run.clone());
        }
    }

    for session in sessions {
        if let Some(index) = owner(&session.cwd, worktrees) {
            rows[index].sessions.push(session.clone());
        }
    }

    for recent in recent {
        if let Some(index) = owner(&recent.cwd, worktrees) {
            let row = &mut rows[index];
            row.last_run = row.last_run.max(recent.last_event_at);
        }
    }

    rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));

    let claimed: HashSet<u64> = rows
        .iter()
        .filter_map(|row| Some(row.pr.as_ref()?.number))
        .collect();
    let mut unclaimed: Vec<&PullRequest> = prs
        .iter()
        .filter(|pr| !claimed.contains(&pr.number))
        .collect();
    unclaimed.sort_by_key(|pr| pr.number);

    rows.extend(unclaimed.into_iter().map(Row::for_pull_request));
    rows
}

/// Sessions in a worktree grouped by their matched terminal tab.
#[derive(Debug, PartialEq, Eq)]
pub struct Placed<'a> {
    /// Sessions matched to each tab in `row.terminals`, in tab order.
    pub tabs: Vec<Vec<&'a JpSession>>,

    /// Sessions that could not be matched to an open tab.
    pub other: Vec<&'a JpSession>,
}

/// Match each session in `row` to the terminal tab running its shell.
///
/// Prefers an exact TTY match.
/// For tabs without a reported TTY, assigns Ghostty sessions in the same
/// working directory in ascending PID order across the tabs with the fewest
/// assigned sessions.
/// [`Row::owns`] reports whether the match is unambiguous.
/// Unmatched sessions are placed in [`Placed::other`].
#[must_use]
pub fn place(row: &Row) -> Placed<'_> {
    let mut tabs = vec![vec![]; row.terminals.len()];
    let mut other = vec![];

    let mut sessions: Vec<&JpSession> = row.sessions.iter().collect();
    sessions.sort_by_key(|session| session.pid);

    let mut taken: Vec<usize> = vec![0; row.terminals.len()];
    for session in sessions {
        let exact = row
            .terminals
            .iter()
            .position(|tab| tab.tty.is_some() && tab.tty == session.tty);
        if let Some(index) = exact {
            tabs[index].push(session);
            continue;
        }

        let candidates: Vec<usize> = row
            .terminals
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.tty.is_none() && session.ghostty && tab.cwd == session.cwd)
            .map(|(index, _)| index)
            .collect();

        // The first tab in the directory with the fewest sessions so far.
        match candidates.iter().min_by_key(|index| taken[**index]) {
            Some(&index) => {
                taken[index] += 1;
                tabs[index].push(session);
            }
            None => other.push(session),
        }
    }

    Placed { tabs, other }
}

/// Return the index of the worktree with the longest path prefix matching
/// `path`.
fn owner(path: &Utf8Path, worktrees: &[Worktree]) -> Option<usize> {
    worktrees
        .iter()
        .enumerate()
        .filter(|(_, worktree)| path.starts_with(&worktree.path))
        .max_by_key(|(_, worktree)| worktree.path.components().count())
        .map(|(index, _)| index)
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
