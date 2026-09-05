//! What can go wrong while driving a terminal.

use std::{fmt, io, time::Duration};

use crate::screen::Screen;

/// A failure raised by the harness itself, never by the code under test.
#[derive(thiserror::Error)]
pub enum Error {
    /// No pty could be opened.
    ///
    /// Either the platform has none, or the process has run out of the ones it
    /// is allowed.
    #[error("could not open a pty: {0}")]
    NoPty(String),

    /// The pty is real, but only a child process can write into it.
    ///
    /// Windows' ConPTY is reachable through a spawned process and nothing else,
    /// so code under test has to be driven there rather than in-process.
    #[error("this platform's pty can only be written into by a child process")]
    NoLocalTty,

    /// The operation needs a real pty and the terminal is model-backed.
    #[error("{0} needs a pty-backed terminal; this one is model-backed")]
    NotAPty(&'static str),

    /// The screen did not reach the state that was waited for in time.
    ///
    /// Carries the screen as it stood when the wait gave up, because that is
    /// the whole of what there is to debug.
    #[error("timed out after {elapsed:?} waiting for {what}\n{screen}")]
    Timeout {
        /// What the caller was waiting for, in its own words.
        what: String,

        /// How long the wait lasted.
        elapsed: Duration,

        /// The screen when the wait gave up.
        screen: Screen,
    },

    /// The wait was not satisfied and the screen can no longer change.
    ///
    /// A pty-backed terminal reaches this when the pty closes; a model-backed
    /// one is here from the start, since only a write moves it.
    #[error("{what} did not happen, and the screen can no longer change on its own\n{screen}")]
    Stalled {
        /// What the caller was waiting for, in its own words.
        what: String,

        /// The screen when the wait gave up.
        screen: Screen,
    },

    /// The kernel refused the new terminal size.
    #[error("could not resize the pty: {0}")]
    Resize(String),

    /// A read or write against the pty failed.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// A child process could not be spawned into the pty.
    #[error("could not spawn into the pty: {0}")]
    Spawn(String),
}

impl fmt::Debug for Error {
    /// Render the message rather than the structure.
    ///
    /// These errors reach a person through `unwrap` or `expect`, which print
    /// `Debug`; the derived form would bury the screen that explains the
    /// failure in a field dump.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
