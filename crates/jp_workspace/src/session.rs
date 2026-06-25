//! Session identity for parallel conversation tracking.
//!
//! Per-session conversation tracking.
//!
//! A **session** is a terminal context — a tab, window, tmux pane, or
//! scripting environment.
//! Each session independently tracks which conversation it is working on, so
//! multiple terminals in the same workspace can run different conversations in
//! parallel without interfering with each other.
//!
//! This module defines the domain types for session identity.
//! The actual resolution logic (checking environment variables, calling
//! platform APIs) lives in `jp_cli::session`.
//! The session-to-conversation mapping — which conversation is active in which
//! session — is in the sibling `session_mapping` module.
//!
//! # Why Sessions Matter
//!
//! Each terminal gets its own conversation pointer.
//! Two tabs open in the same workspace can run independent queries
//! simultaneously — tool calls in one session don't interleave with events
//! from another, and starting a new conversation in one terminal doesn't affect
//! any other terminal.
//!
//! # Identity Resolution
//!
//! Session identity is resolved using a three-layer strategy, checked in order:
//!
//! 1. **`$JP_SESSION`** — Explicit override.
//!    Any non-empty string.
//!    Takes priority over everything else.
//!    Useful for CI, scripts, SSH, or any environment where automatic detection
//!    is unreliable.
//!
//! 2. **Platform-specific detection** — On Unix, `getsid(0)` returns the
//!    session leader PID (typically the shell spawned by the terminal).
//!    On Windows, `GetConsoleWindow()` returns the console host HWND.
//!    Both are unique per tab/pane and stable across subshells and tmux
//!    detach/reattach.
//!
//! 3. **Terminal environment variables** — `$TMUX_PANE`, `$WEZTERM_PANE`,
//!    `$TERM_SESSION_ID` (macOS Terminal.app), `$ITERM_SESSION_ID` (iTerm2).
//!    Only variables with per-tab or per-pane granularity are used.
//!    Per-window variables like `$WT_SESSION`, `$KITTY_WINDOW_ID`, and
//!    `$ALACRITTY_WINDOW_ID` are deliberately excluded because multiple tabs in
//!    the same window share the value.
//!
//! If none of these produce an identity, JP operates without a session.
//! Interactive terminals show a conversation picker; non-interactive
//! environments fail with guidance to use `--id`, `--new`, or `$JP_SESSION`.
//!
//! # Provenance and Stale Detection
//!
//! Each session identity records its [`SessionSource`] — how it was
//! determined.
//! This drives stale mapping cleanup:
//!
//! | Source   | Stale detection                            |
//! | -------- | ------------------------------------------ |
//! | `Getsid` | Delete only when the session leader PID is |
//! |          | confirmed dead. A live process keeps its   |
//! |          | session unconditionally.                   |
//! | `Hwnd`   | Delete only when the console window handle |
//! |          | is no longer valid.                        |
//! | `Env`    | Not possible — the string is opaque.       |
//! |          | Cleaned up only when all referenced        |
//! |          | conversations are deleted from disk.       |
//!
//! Stale detection runs at the end of every `jp` invocation via
//! [`Workspace::cleanup_stale_files`].
//!
//! # Storage
//!
//! Session mappings are stored in user-local data, not in the workspace
//! directory:
//!
//! ```txt
//! ~/.local/share/jp/workspace/<workspace-id>/sessions/<storage-key>.json
//! ```
//!
//! The `<storage-key>` encodes the full [`Session`] identity — both the value
//! and its [`SessionSource`] — via [`Session::storage_key`]:
//!
//! ```txt
//! getsid-<pid>.json
//! hwnd-<handle>.json
//! env-<KEY>-<hash(value)>.json
//! ```
//!
//! Encoding the source keeps two sessions that share a value but differ in
//! source from aliasing one file (e.g.
//! `$JP_SESSION=1234` and `$TMUX_PANE=1234`, or an `Env` value that matches a
//! session-leader PID).
//! The file contains a `SessionMapping` with the session's conversation
//! history.
//!
//! # Module Boundaries
//!
//! | Location          | Concern                                 |
//! | ----------------- | --------------------------------------- |
//! | This module       | Domain types (`SessionId`,              |
//! |                   | `SessionSource`, `Session`)             |
//! | `session_mapping` | Session-to-conversation mapping         |
//! |                   | (`SessionMapping`, `Workspace` methods) |
//! | `jp_cli::session` | Resolution logic (`resolve()`, platform |
//! |                   | APIs, env var checks)                   |
//! | `SessionBackend`  | Storage I/O (`load_session`,            |
//! |                   | `save_session`)                         |
//!
//! See: `docs/rfd/020-parallel-conversations.md`
//!
//! [`Workspace::cleanup_stale_files`]: super::Workspace::cleanup_stale_files

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// An opaque session identity.
///
/// The value is platform-dependent: a PID string from `getsid(0)` on Unix, an
/// HWND string on Windows, or an arbitrary string from an environment variable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    /// Create a session ID from any non-empty string.
    ///
    /// Returns `None` if the string is empty.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        if value.is_empty() {
            return None;
        }

        Some(Self(value))
    }

    /// The raw string value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Decode this id as a Unix session-leader PID.
    ///
    /// Inverse of the encoding [`Session::getsid`] applies.
    /// Returns `None` if the id is not a decimal `i32` (an `Env`-sourced value,
    /// or a corrupt file).
    pub(crate) fn as_pid(&self) -> Option<i32> {
        self.0.parse().ok()
    }

    /// Decode this id as a Windows console window handle.
    ///
    /// Inverse of the encoding [`Session::hwnd`] applies.
    /// Returns `None` if the id is not a decimal `isize`.
    pub(crate) fn as_hwnd(&self) -> Option<isize> {
        self.0.parse().ok()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// How the session identity was determined.
///
/// This is stored alongside the session mapping so that stale detection can
/// decide whether the mapping is still valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionSource {
    /// Unix `getsid(0)` — the session leader PID.
    ///
    /// Stale detection: delete only when the PID is confirmed dead.
    Getsid,

    /// Windows `GetConsoleWindow()` — the console window handle.
    ///
    /// Stale detection: delete only when the window handle is no longer valid.
    Hwnd,

    /// An environment variable provided the session identity.
    ///
    /// Covers `$JP_SESSION`, `$TMUX_PANE`, `$WEZTERM_PANE`, etc. Stale
    /// detection is not possible for these — cleanup relies on checking
    /// whether the referenced conversation still exists.
    Env {
        /// The name of the environment variable (e.g.
        /// `"JP_SESSION"`).
        key: String,
    },
}

impl SessionSource {
    /// Create a session source from an environment variable name.
    #[must_use]
    pub fn env(key: impl Into<String>) -> Self {
        Self::Env { key: key.into() }
    }
}

impl fmt::Display for SessionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Getsid => write!(f, "getsid"),
            Self::Hwnd => write!(f, "hwnd"),
            Self::Env { key } => write!(f, "env:{key}"),
        }
    }
}

/// Encode a session identity into its filesystem-safe storage key.
///
/// Shared by [`Session::storage_key`] and session-store cleanup, which rebuilds
/// the key from a stored mapping.
/// RFD 087's user-global workspace session store reuses this same scheme.
pub(crate) fn session_storage_key(id: &SessionId, source: &SessionSource) -> String {
    // Every interpolated segment is sanitized: the id and the env key both reach
    // this function from deserialized mappings as well as from a live session,
    // and `SessionId::new` / `SessionSource::env` accept arbitrary strings, so
    // neither can be trusted to be a bare value. Sanitizing keeps a `/` or `..`
    // from escaping the `sessions/` directory when the result becomes a
    // filename. The env *value* is hashed, which is path-safe on its own.
    match source {
        SessionSource::Getsid => format!("getsid-{}", sanitize_path_segment(id.as_str())),
        SessionSource::Hwnd => format!("hwnd-{}", sanitize_path_segment(id.as_str())),
        SessionSource::Env { key } => {
            let key = sanitize_path_segment(key);
            let digest = format!("{:x}", Sha256::digest(id.as_str().as_bytes()));
            format!("env-{key}-{}", &digest[..16])
        }
    }
}

/// Whether `segment` is a single path component safe to use as a filename.
///
/// Rejects empty strings, `.` / `..`, and anything containing a path separator,
/// so an untrusted value can't escape its directory when used directly as a
/// filename (e.g. the legacy bare-value session-store probe).
pub(crate) fn is_safe_path_segment(segment: &str) -> bool {
    !segment.is_empty() && segment != "." && segment != ".." && !segment.contains(['/', '\\'])
}

/// Replace any character that isn't a safe filename character with `_`.
///
/// Env var names are conventionally `[A-Za-z0-9_]`, so real keys pass through
/// unchanged; this only neutralizes path separators and `..` from a tampered or
/// externally constructed source before it is formatted into a path.
fn sanitize_path_segment(segment: &str) -> String {
    segment
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A resolved session identity, combining the ID with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The session identity value.
    pub id: SessionId,

    /// How the identity was resolved.
    pub source: SessionSource,
}

impl Session {
    /// Filesystem-safe key encoding the full session identity.
    ///
    /// The session source is part of the key, so two sessions that share a
    /// value but differ in source never collide on one file:
    ///
    /// - `getsid-<pid>`
    /// - `hwnd-<handle>`
    /// - `env-<KEY>-<hash(value)>`
    ///
    /// The opaque `Env` value is hashed, which both disambiguates distinct
    /// variables holding the same value and keeps unsafe characters out of the
    /// filename.
    #[must_use]
    pub fn storage_key(&self) -> String {
        session_storage_key(&self.id, &self.source)
    }

    /// Build a `Getsid` session from a Unix session-leader PID.
    ///
    /// Owns the PID-to-id encoding so it stays the inverse of
    /// `SessionId::as_pid`, which stale detection uses to decode it.
    #[must_use]
    pub fn getsid(pid: i32) -> Self {
        Self {
            id: SessionId(pid.to_string()),
            source: SessionSource::Getsid,
        }
    }

    /// Build an `Hwnd` session from a Windows console window handle.
    ///
    /// Owns the handle-to-id encoding so it stays the inverse of
    /// `SessionId::as_hwnd`, which stale detection uses to decode it.
    #[must_use]
    pub fn hwnd(handle: isize) -> Self {
        Self {
            id: SessionId(handle.to_string()),
            source: SessionSource::Hwnd,
        }
    }
}
