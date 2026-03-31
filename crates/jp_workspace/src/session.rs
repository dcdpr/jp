//! Session identity for parallel conversation tracking.
//!
//! Per-session conversation tracking.
//!
//! A **session** is a terminal context — a tab, window, tmux pane, or scripting
//! environment. Each session independently tracks which conversation it is
//! working on, so multiple terminals in the same workspace can run different
//! conversations in parallel without interfering with each other.
//!
//! This module defines the domain types for session identity. The actual
//! resolution logic (checking environment variables, calling platform APIs)
//! lives in `jp_cli::session`. The session-to-conversation mapping — which
//! conversation is active in which session — is in the sibling
//! [`session_mapping`] module.
//!
//! # Why Sessions Matter
//!
//! Each terminal gets its own conversation pointer. Two tabs open in the same
//! workspace can run independent queries simultaneously — tool calls in one
//! session don't interleave with events from another, and starting a new
//! conversation in one terminal doesn't affect any other terminal.
//!
//! # Identity Resolution
//!
//! Session identity is resolved using a three-layer strategy, checked in order:
//!
//! 1. **`$JP_SESSION`** — Explicit override. Any non-empty string. Takes
//!    priority over everything else. Useful for CI, scripts, SSH, or any
//!    environment where automatic detection is unreliable.
//!
//! 2. **Platform-specific detection** — On Unix, `getsid(0)` returns the
//!    session leader PID (typically the shell spawned by the terminal). On
//!    Windows, `GetConsoleWindow()` returns the console host HWND. Both are
//!    unique per tab/pane and stable across subshells and tmux detach/reattach.
//!
//! 3. **Terminal environment variables** — `$TMUX_PANE`, `$WEZTERM_PANE`,
//!    `$TERM_SESSION_ID` (macOS Terminal.app), `$ITERM_SESSION_ID` (iTerm2).
//!    Only variables with per-tab or per-pane granularity are used. Per-window
//!    variables like `$WT_SESSION`, `$KITTY_WINDOW_ID`, and
//!    `$ALACRITTY_WINDOW_ID` are deliberately excluded because multiple tabs in
//!    the same window share the value.
//!
//! If none of these produce an identity, JP operates without a session.
//! Interactive terminals show a conversation picker; non-interactive
//! environments fail with guidance to use `--id`, `--new`, or `$JP_SESSION`.
//!
//! # Provenance and Stale Detection
//!
//! Each session identity records its [`SessionSource`] — how it was determined.
//! This drives stale mapping cleanup:
//!
//! | Source   | Stale detection                          |
//! |----------|------------------------------------------|
//! | `Getsid` | Check if the session leader PID is still |
//! |          | alive.                                   |
//! | `Hwnd`   | Check if the console host process is     |
//! |          | still alive.                             |
//! | `Env`    | Not possible — the string is opaque.     |
//! |          | Cleaned up only when all referenced      |
//! |          | conversations are deleted.               |
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
//! ~/.local/share/jp/workspace/<workspace-id>/sessions/<session-key>.json
//! ```
//!
//! The `<session-key>` is the [`SessionId`] value (PID string, HWND string,
//! or `$JP_SESSION` value). The file contains a [`SessionMapping`] with the
//! session's conversation history.
//!
//! # Module Boundaries
//!
//! | Location              | Concern                                 |
//! |-----------------------|-----------------------------------------|
//! | This module           | Domain types (`SessionId`,              |
//! |                       | `SessionSource`, `Session`)             |
//! | [`session_mapping`]   | Session-to-conversation mapping         |
//! |                       | (`SessionMapping`, `Workspace` methods) |
//! | `jp_cli::session`     | Resolution logic (`resolve()`, platform |
//! |                       | APIs, env var checks)                   |
//! | `jp_storage::Storage` | File I/O (`load_session_data`,          |
//! |                       | `save_session_data`)                    |
//!
//! See: `docs/rfd/020-parallel-conversations.md`
//!
//! [`session_mapping`]: super::session_mapping
//! [`SessionMapping`]: super::session_mapping::SessionMapping
//! [`Workspace::cleanup_stale_files`]: super::Workspace::cleanup_stale_files

use std::fmt;

use serde::{Deserialize, Serialize};

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

    /// The raw string value, used as the session mapping filename.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
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
    /// Stale detection: check if the PID is still alive.
    Getsid,

    /// Windows `GetConsoleWindow()` — the console window handle.
    ///
    /// Stale detection: check if the console host process is still alive.
    Hwnd,

    /// An environment variable provided the session identity.
    ///
    /// Covers `$JP_SESSION`, `$TMUX_PANE`, `$WEZTERM_PANE`, etc. Stale
    /// detection is not possible for these — cleanup relies on checking whether
    /// the referenced conversation still exists.
    Env {
        /// The name of the environment variable (e.g. `"JP_SESSION"`).
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

/// A resolved session identity, combining the ID with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The session identity value.
    pub id: SessionId,

    /// How the identity was resolved.
    pub source: SessionSource,
}

impl Session {
    /// Create a new `Session` from a `SessionId` using the `Getsid` source.
    #[must_use]
    pub fn getsid(id: SessionId) -> Self {
        Self {
            id,
            source: SessionSource::Getsid,
        }
    }

    /// Create a new `Session` from a `SessionId` using the `Hwnd` source.
    #[must_use]
    pub fn hwnd(id: SessionId) -> Self {
        Self {
            id,
            source: SessionSource::Hwnd,
        }
    }
}
