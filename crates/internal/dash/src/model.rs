//! Data model for dashboard rows and source snapshots.
//!
//! Every source reports paths or branch names; [`rows`] attaches each report to
//! the git worktree it belongs to.
//! This module does no I/O.

use std::collections::HashSet;

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};

/// A git worktree of the repository the dashboard watches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// The worktree's root directory.
    pub path: Utf8PathBuf,

    /// The checked-out branch, without `refs/heads/`.
    /// `None` for a detached `HEAD`.
    pub branch: Option<String>,
}

/// A terminal surface open in the terminal emulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    /// The emulator's identifier for the surface, used to focus it.
    pub id: String,

    /// The directory the shell in the surface reported last.
    pub cwd: Utf8PathBuf,

    /// The surface's title, which shells and programs set to what is running in
    /// it.
    pub name: String,

    /// The surface's terminal device, as `ps` names it (`ttys001`).
    /// `None` from a Ghostty build that does not report it.
    pub tty: Option<String>,
}

/// A `jp` process holding a conversation lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpRun {
    pub pid: u32,
    pub cwd: Utf8PathBuf,

    /// The locked conversation's title, or its ID when it has none.
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

/// A shell's JP session: the conversations `jp` activated in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpSession {
    /// The session leader's process ID, which names the session.
    pub pid: u32,

    /// The process ID of the session's shell: the leader itself, or its child
    /// when the leader is `login`.
    pub shell: u32,

    /// Whether the shell runs inside Ghostty, so one of its tabs can hold it.
    pub ghostty: bool,

    /// The shell's terminal device, as `ps` names it (`ttys001`).
    pub tty: Option<String>,

    pub cwd: Utf8PathBuf,

    /// The session's conversations: the active one first, then the rest by most
    /// recent activity.
    pub conversations: Vec<SessionConversation>,

    /// How many older conversations the session has beyond `conversations`.
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

/// The last update of the local default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUpdate {
    /// The default branch, such as `main`.
    pub branch: String,

    /// Whether the update moved it.
    pub moved: bool,
}

/// An open pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub url: String,

    /// The branch the pull request merges from.
    pub head: String,
    pub draft: bool,
    pub checks: Checks,

    /// Review threads on the pull request, pending ones included.
    pub threads: u32,

    /// Review threads with at least one reply.
    pub answered: u32,
}

/// The outcome of the checks on a pull request's head commit.
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

    /// Whether the row is a worktree, not a pull request without one.
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

/// Join the sources into one row per worktree, sorted by worktree name,
/// followed by one row per pull request whose branch no worktree has checked
/// out, sorted by number.
///
/// A terminal, process, session, or last run belongs to the worktree whose path
/// is the longest prefix of its working directory, so one sitting in a
/// subdirectory still counts.
/// Those outside every worktree are dropped.
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
        .map(|worktree| Row {
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
        })
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

    rows.extend(unclaimed.into_iter().map(|pr| Row {
        id: RowId::PullRequest(pr.number),
        name: String::new(),
        branch: Some(pr.head.clone()),
        terminals: vec![],
        runs: vec![],
        sessions: vec![],
        last_run: None,
        pr: Some(pr.clone()),
    }));
    rows
}

/// A worktree's sessions, placed in its tabs.
#[derive(Debug, PartialEq, Eq)]
pub struct Placed<'a> {
    /// The sessions of each of the row's tabs, in the row's tab order.
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

/// The index of the worktree containing `path`, preferring the deepest one.
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
