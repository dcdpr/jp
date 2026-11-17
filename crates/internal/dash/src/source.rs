//! Background data sources for the dashboard.
//!
//! Each source polls on its own thread and interval, sending an [`Update`] to
//! the UI after every read.
//! Slow sources (such as GitHub network calls) do not block rendering, and a
//! failure in one source does not stop the others.
//!
//! [`spawn`] starts all source threads and returns a [`Refresher`] that wakes
//! them for an immediate poll.
//! The GitHub source also drives the merge queue, so it polls again shortly
//! after the queue changes something on GitHub.

pub mod cache;
pub mod ghostty;
pub mod git;
pub mod github;
pub mod jp;

use std::{
    io,
    process::{Command, Output},
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread,
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};

use self::github::QueueControl;
use crate::{app::Update, model::BaseUpdate};

/// Delay before re-polling GitHub after the merge queue mutates remote state.
const SOON: Duration = Duration::from_secs(5);

/// How often the GitHub source polls pull requests and moves the merge queue.
const PULL_REQUESTS_INTERVAL: Duration = Duration::from_secs(30);

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
    pokes: Vec<Sender<bool>>,

    /// The GitHub source's poke, also in `pokes`.
    github: Sender<bool>,
}

impl Refresher {
    pub fn refresh(&self) {
        for poke in &self.pokes {
            // Ignore send errors if a source thread has already exited.
            let _gone = poke.send(true);
        }
    }

    /// Wake the GitHub source without forcing a full review-thread refresh.
    pub fn refresh_pull_requests(&self) {
        let _gone = self.github.send(false);
    }
}

/// Spawn all background source threads for the repository containing `dir`,
/// with `control` pausing the merge queue or lifting its holds.
pub fn spawn(dir: &Utf8Path, updates: &Sender<Update>, control: Arc<QueueControl>) -> Refresher {
    let mut jp = jp::Jp::new(dir.to_owned());
    let mut github = github::GitHub::open(dir.to_owned());
    let git_dir = dir.to_owned();
    let base_dir = dir.to_owned();
    let queue_updates = updates.clone();
    let github_poke: Arc<OnceLock<Sender<bool>>> = Arc::default();
    let soon = Arc::clone(&github_poke);

    // Emit cached pull requests immediately before the first network poll
    // completes.
    if let Some(cached) = github.cached() {
        let _gone = updates.send(Update::PullRequests(Ok(cached)));
    }

    let github = github_poke
        .get_or_init(|| {
            every(PULL_REQUESTS_INTERVAL, updates.clone(), move |asked| {
                let poll = github.pull_requests(asked, &control);
                for event in &poll.events {
                    if event.notify {
                        notify(&event.message);
                    }
                    let _gone = queue_updates.send(Update::Notice(event.message.clone()));
                }
                let _gone = queue_updates.send(Update::Queue(poll.role));
                if poll.changed {
                    poll_soon(&soon);
                }
                Update::PullRequests(poll.prs.map_err(|e| e.to_string()))
            })
        })
        .clone();

    Refresher {
        github: github.clone(),
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
            github,
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

/// Schedule a non-forced poll on `poke` after [`SOON`].
fn poll_soon(poke: &Arc<OnceLock<Sender<bool>>>) {
    let Some(poke) = poke.get().cloned() else {
        return;
    };
    thread::spawn(move || {
        thread::sleep(SOON);
        let _gone = poke.send(false);
    });
}

/// Display `message` as a macOS desktop notification, ignoring any failure.
fn notify(message: &str) {
    // Pass `message` via `argv` rather than interpolating it into the script.
    let _unshown = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title \"dash\"",
            "-e",
            "end run",
            message,
        ])
        .output();
}

/// Run `read` on a background thread every `interval` (or immediately when
/// woken via the returned [`Sender`]) until the UI receiver disconnects.
///
/// Passes `true` to `read` when woken by an explicit refresh request (a poke
/// carrying `true`), and `false` on timer ticks and other pokes.
fn every<F>(interval: Duration, updates: Sender<Update>, mut read: F) -> Sender<bool>
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
                Ok(asked) => asked,
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => return,
            };

            // Drain any additional pokes queued while `read` was running.
            while let Ok(more) = pokes.try_recv() {
                asked |= more;
            }
        }
    });

    poke
}

/// Apply `work` to every item in `items` across at most `workers` concurrent
/// threads, returning results in input order.
fn parallel<T, R, F>(items: &[T], workers: usize, work: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    let next = AtomicUsize::new(0);
    let done = Mutex::new(Vec::with_capacity(items.len()));

    thread::scope(|scope| {
        for _ in 0..workers.clamp(1, items.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        return;
                    };
                    let result = work(item);
                    done.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((index, result));
                }
            });
        }
    });

    let mut done = done.into_inner().unwrap_or_else(PoisonError::into_inner);
    done.sort_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, result)| result).collect()
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

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
