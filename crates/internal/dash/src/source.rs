//! Everything that reads the outside world.
//!
//! Each source runs on its own thread and interval, and sends the UI an
//! [`Update`] after every read: the fresh snapshot, or the error the read hit.
//! A slow source (GitHub takes about a second) never stalls the screen, and a
//! failing one leaves the others running.
//!
//! [`spawn`] starts them all and returns a [`Refresher`] that makes every
//! source read again straight away.

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

/// Why a source could not be read.
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

    /// A read from the cache alone needed something the cache does not hold.
    #[error("{0} is not cached")]
    NotCached(String),
}

/// Makes every source read again without waiting for its interval.
#[derive(Clone)]
pub struct Refresher {
    pokes: Vec<Sender<()>>,
}

impl Refresher {
    pub fn refresh(&self) {
        for poke in &self.pokes {
            // A source whose thread is gone has nothing to refresh.
            let _gone = poke.send(());
        }
    }
}

/// Start every source, reading the repository that contains `dir`.
pub fn spawn(dir: &Utf8Path, updates: &Sender<Update>) -> Refresher {
    let mut jp = jp::Jp::new(dir.to_owned());
    let mut github = github::GitHub::open(dir.to_owned());
    let git_dir = dir.to_owned();
    let base_dir = dir.to_owned();

    // What the last run saw, on screen before the first network read returns.
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
            // Keeps the local default branch at its upstream, so a new worktree
            // starts from the latest `main` without waiting on a fetch.
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

/// Run `read` on a new thread every `interval`, or sooner when poked, until the
/// UI stops listening.
///
/// `read` is told whether the user asked for this read, so a source can do work
/// it otherwise spaces out.
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

            // Pokes that piled up during the read ask for the same thing.
            while pokes.try_recv().is_ok() {}
        }
    });

    poke
}

/// Run a command and return its standard output, failing when it exits
/// unsuccessfully.
fn stdout(program: &'static str, command: &mut Command) -> Result<String, Error> {
    let output = output(program, command)?;
    if !output.status.success() {
        return Err(failure(program, &output.stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The error for a command that failed, from the first line of its standard
/// error.
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

/// Run a command and return what it produced, whatever its exit status.
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
