//! Background data sources for the dashboard.
//!
//! Each source polls on its own thread and interval, sending an [`Update`] to
//! the UI after every read.
//! Slow sources (such as GitHub network calls) do not block rendering, and a
//! failure in one source does not stop the others.
//!
//! [`spawn`] starts all source threads and returns a [`Refresher`] that wakes
//! them for an immediate poll.

pub mod cache;
pub mod ghostty;
pub mod git;
pub mod github;
pub mod jp;

use std::{
    io,
    process::{Command, Output},
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};

use crate::{app::Update, model::BaseUpdate};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot run `{program}`: {source}")]
    Spawn {
        program: &'static str,
        source: io::Error,
    },

    #[error("`{program}` failed: {message}")]
    Failed {
        program: &'static str,
        message: String,
    },

    #[error("unexpected output from `{program}`: {source}")]
    Parse {
        program: &'static str,
        source: serde_json::Error,
    },

    #[error(transparent)]
    Workspace(#[from] jp_workspace::Error),

    #[error("the JP workspace has no file storage")]
    NoStorage,

    /// Offline cache read missed a required entry.
    #[error("{0} is not cached")]
    NotCached(String),
}

/// Handle that wakes all background source threads for an immediate poll.
#[derive(Clone)]
pub struct Refresher {
    pokes: Vec<Sender<()>>,
}

impl Refresher {
    pub fn refresh(&self) {
        for poke in &self.pokes {
            // Ignore send errors if a source thread has already exited.
            let _gone = poke.send(());
        }
    }
}

/// Spawn all background source threads for the repository containing `dir`.
pub fn spawn(dir: &Utf8Path, updates: &Sender<Update>) -> Refresher {
    let mut jp = jp::Jp::new(dir.to_owned());
    let mut github = github::GitHub::open(dir.to_owned());
    let git_dir = dir.to_owned();
    let base_dir = dir.to_owned();

    // Emit cached pull requests immediately before the first network poll
    // completes.
    if let Some(cached) = github.cached() {
        let _gone = updates.send(Update::PullRequests(Ok(cached)));
    }

    Refresher {
        pokes: vec![
            every(Duration::from_secs(5), updates.clone(), move |_| {
                Update::Worktrees(git::worktrees(&git_dir).map_err(|e| e.to_string()))
            }),
            every(Duration::from_secs(2), updates.clone(), |_| {
                Update::Terminals(ghostty::terminals().map_err(|e| e.to_string()))
            }),
            every(Duration::from_secs(3), updates.clone(), move |_| {
                Update::Jp(jp.read().map_err(|e| e.to_string()))
            }),
            every(Duration::from_secs(30), updates.clone(), move |asked| {
                Update::PullRequests(github.pull_requests(asked).map_err(|e| e.to_string()))
            }),
            // Periodically fast-forward the local default branch so new
            // worktrees start from an up-to-date base.
            every(Duration::from_mins(5), updates.clone(), move |_| {
                let branch = git::default_branch(&base_dir);
                Update::Base(
                    git::update_branch(&base_dir, &branch)
                        .map(|moved| BaseUpdate { branch, moved })
                        .map_err(|e| e.to_string()),
                )
            }),
        ],
    }
}

/// Run `read` on a background thread every `interval` (or immediately when
/// woken via the returned [`Sender`]) until the UI receiver disconnects.
///
/// Passes `true` to `read` when woken by an explicit refresh request, and
/// `false` on scheduled timer ticks.
fn every<F>(interval: Duration, updates: Sender<Update>, mut read: F) -> Sender<()>
where
    F: FnMut(bool) -> Update + Send + 'static,
{
    let (poke, pokes) = mpsc::channel();

    thread::spawn(move || {
        let mut asked = false;
        loop {
            if updates.send(read(asked)).is_err() {
                return;
            }

            asked = match pokes.recv_timeout(interval) {
                Ok(()) => true,
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => return,
            };

            // Drain any additional refresh pokes queued while `read` was running.
            while pokes.try_recv().is_ok() {}
        }
    });

    poke
}

/// Run `command` and return its stdout as UTF-8, or [`Error::Failed`] if it
/// exits non-zero.
fn stdout(program: &'static str, command: &mut Command) -> Result<String, Error> {
    let output = output(program, command)?;
    if !output.status.success() {
        return Err(failure(program, &output.stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Build an [`Error::Failed`] from the first non-empty line of `stderr`.
fn failure(program: &'static str, stderr: &[u8]) -> Error {
    let stderr = String::from_utf8_lossy(stderr);
    let message = stderr
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no error message")
        .trim()
        .to_owned();

    Error::Failed { program, message }
}

/// Run `command` and return its [`Output`] regardless of exit status.
fn output(program: &'static str, command: &mut Command) -> Result<Output, Error> {
    command
        .output()
        .map_err(|source| Error::Spawn { program, source })
}

/// Resolve symlinks in `path`, keeping it as given when it cannot be resolved.
///
/// Sources report the same directory in different spellings (a shell may keep
/// the symlinked path it was `cd`'d through, `git` and `lsof` report the real
/// one), and matching them up needs one spelling.
fn canonical(path: &str) -> Utf8PathBuf {
    Utf8Path::new(path)
        .canonicalize_utf8()
        .unwrap_or_else(|_| Utf8PathBuf::from(path))
}
