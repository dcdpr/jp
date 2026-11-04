//! What the dashboard knows, and how the sources join into rows.
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
    /// The process that holds the lock.
    pub pid: u32,

    /// The process's working directory.
    pub cwd: Utf8PathBuf,

    /// The locked conversation's title, or its ID when it has none.
    pub conversation: String,

    /// When the turn started waiting on the user to answer a question, if it
    /// is.
    pub waiting_since: Option<DateTime<Utc>>,
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

    /// The shell's working directory.
    pub cwd: Utf8PathBuf,

    /// The session's conversations: the active one first, then the rest by most
    /// recent activity.
    pub conversations: Vec<SessionConversation>,

    /// How many older conversations the session has beyond `conversations`.
    pub more: usize,
}

impl JpSession {
    /// The session's active conversation.
    #[must_use]
    pub fn active(&self) -> Option<&SessionConversation> {
        self.conversations
            .iter()
            .find(|conversation| conversation.active)
    }
}

/// A conversation a session activated: what `jp conversation ls` lists for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConversation {
    /// The conversation's ID, as `jp` prints it.
    pub id: String,

    pub title: Option<String>,

    /// Whether it is the session's active conversation.
    pub active: bool,

    pub pinned: bool,

    /// The number of events in the conversation.
    pub events: usize,

    /// When its last event was recorded.
    /// For a finished turn that is the assistant's reply.
    /// `None` for an empty conversation.
    pub last_event_at: Option<DateTime<Utc>>,

    pub status: Status,
}

/// What a `jp` process is doing with a conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    /// No `jp` process holds it.
    #[default]
    Idle,

    /// A `jp` process holds it, working on a turn.
    Running,

    /// A `jp` process holds it, and has asked the user a question that is not
    /// answered yet.
    Waiting { since: DateTime<Utc> },
}

impl Status {
    /// The status of a conversation a `jp` process holds, waiting on the user
    /// since `waiting_since`, if it is.
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

/// What a row stands for, and what identifies it across refreshes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RowId {
    /// A worktree, by its root directory.
    Worktree(Utf8PathBuf),

    /// An open pull request whose branch no worktree has checked out, by its
    /// number.
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

    /// The open pull request whose head is the worktree's branch.
    /// Always set for a pull request without a worktree.
    pub pr: Option<PullRequest>,
}

impl Row {
    /// Whether `tab` certainly runs `session`'s shell.
    ///
    /// Certain when the two share a terminal device.
    /// A Ghostty build that does not report the tab's device leaves only the
    /// directory to go on, which is certain only when the tab is the only one
    /// in its directory and the session the only Ghostty shell there.
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

    /// Whether the worktree has anything going on: an open terminal, a running
    /// `jp`, or an open pull request.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.terminals.is_empty() || !self.runs.is_empty() || self.pr.is_some()
    }
}

/// Join the sources into one row per worktree, sorted by worktree name,
/// followed by one row per pull request whose branch no worktree has checked
/// out, sorted by number.
///
/// A terminal, process, or session belongs to the worktree whose path is the
/// longest prefix of its working directory, so one sitting in a subdirectory
/// still counts.
/// Those outside every worktree are dropped.
#[must_use]
pub fn rows(
    worktrees: &[Worktree],
    terminals: &[Terminal],
    runs: &[JpRun],
    sessions: &[JpSession],
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
        pr: Some(pr.clone()),
    }));
    rows
}

/// A worktree's sessions, placed in its tabs.
#[derive(Debug, PartialEq, Eq)]
pub struct Placed<'a> {
    /// The sessions of each of the row's tabs, in the row's tab order.
    pub tabs: Vec<Vec<&'a JpSession>>,

    /// Sessions no tab could be found for.
    pub other: Vec<&'a JpSession>,
}

/// Place each of the row's sessions in the tab its shell runs in: the tab on
/// the same terminal device.
///
/// A tab whose device Ghostty does not report (an older build) is matched by
/// directory instead: a session of a shell inside Ghostty goes to such a tab in
/// the same directory, several of them taking its sessions in order, the oldest
/// session (lowest leader PID) to the first.
/// That guess is wrong when tabs were reordered; [`Row::owns`] says when there
/// was no guessing.
/// A session no tab matches goes to no tab.
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
