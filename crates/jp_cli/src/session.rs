//! Session identity resolution.
//!
//! Resolves the current terminal session using a three-layer strategy:
//!
//! 1. `$JP_SESSION` environment variable (explicit override)
//! 2. Platform-specific automatic detection (`getsid` on Unix,
//!    `GetConsoleWindow` on Windows)
//! 3. Terminal-specific environment variables (`$TMUX_PANE`, etc.)
//!
//! See: `docs/rfd/020-parallel-conversations.md`

use std::env;

use jp_workspace::session::{Session, SessionId, SessionSource};
use tracing::debug;

/// Terminal-specific environment variables that provide per-tab/per-pane
/// session granularity.
///
/// Only variables with per-tab or per-pane granularity are included. Per-window
/// variables (e.g. `$WT_SESSION`, `$KITTY_WINDOW_ID`, `$ALACRITTY_WINDOW_ID`)
/// are deliberately excluded because multiple tabs in the same window share the
/// value.
const TERMINAL_SESSION_VARS: &[(&str, &str)] = &[
    ("TMUX_PANE", "tmux"),
    ("WEZTERM_PANE", "WezTerm"),
    ("TERM_SESSION_ID", "macOS Terminal.app"),
    ("ITERM_SESSION_ID", "iTerm2"),
];

/// Resolve the session identity for the current process.
///
/// Returns `None` if no session identity can be determined (e.g. no controlling
/// terminal, no `$JP_SESSION`, and no recognized terminal env vars).
pub(crate) fn resolve() -> Option<Session> {
    if let Some(session) = from_env("JP_SESSION") {
        debug!(id = session.id.as_str(), "Session from $JP_SESSION.");
        return Some(session);
    }

    if let Some(session) = from_platform() {
        debug!(id = session.id.as_str(), source = ?session.source, "Session from platform API.");
        return Some(session);
    }

    for &(var, terminal) in TERMINAL_SESSION_VARS {
        let Some(session) = from_env(var) else {
            continue;
        };

        debug!(
            id = session.id.as_str(),
            var, terminal, "Session from terminal env var."
        );

        return Some(session);
    }

    debug!("No session identity found.");
    None
}

/// Try to build a session from an environment variable.
fn from_env(key: &str) -> Option<Session> {
    let val = env::var(key).ok()?;
    let id = SessionId::new(val)?;
    let source = SessionSource::env(key);

    Some(Session { id, source })
}

/// Platform-specific automatic session detection.
///
/// `getsid(0)` returns the session leader PID — typically the login shell
/// spawned by the terminal. Unique per tab/window/tmux-pane, stable across
/// subshells, stable across tmux detach/reattach.
#[cfg(unix)]
fn from_platform() -> Option<Session> {
    // Safety: getsid is a standard POSIX function. Passing 0 queries the
    // calling process's session leader, which is always valid.
    let sid = unsafe { libc::getsid(0) };
    if sid < 0 {
        return None;
    }

    let id = SessionId::new(sid.to_string())?;
    Some(Session::getsid(id))
}

/// Platform-specific automatic session detection.
///
/// `GetConsoleWindow` returns a window handle or null. No preconditions beyond
/// being a console application.
#[cfg(windows)]
fn from_platform() -> Option<Session> {
    // Safety: GetConsoleWindow returns a window handle or null. No
    // preconditions beyond being a console application.
    let hwnd = unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() };
    if hwnd.is_null() {
        return None;
    }

    let id = SessionId::new(format!("{hwnd:?}"))?;
    Some(Session::hwnd(id))
}

/// Platform-specific automatic session detection.
///
/// Only Unix and Windows are supported.
#[cfg(not(any(unix, windows)))]
fn from_platform() -> Option<Session> {
    None
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
