//! Shared conversation lock acquisition with polling, timeout, and interactive
//! prompts.
//!
//! [`acquire_lock`] polls `Workspace::lock_conversation` at 500ms intervals
//! until the lock is acquired or a timeout is reached. On timeout, interactive
//! terminals get a prompt; non-interactive environments get `LockTimeout`.
//!
//! The prompt options are controlled by [`LockRequest::allow_new`] and
//! [`LockRequest::allow_fork`]. When both are false, only "Continue waiting"
//! and "Cancel" are shown.

use std::{
    env, thread,
    time::{Duration, Instant},
};

use inquire::Select;
use jp_conversation::ConversationId;
use jp_printer::Printer;
use jp_storage::lock::LockInfo;
use jp_workspace::{ConversationHandle, ConversationLock, LockResult, Workspace, session::Session};

use crate::{
    ctx::Ctx,
    error::{Error, Result},
};

const LOCK_DURATION_ENV: &str = "JP_LOCK_DURATION";

/// Result of attempting to acquire a conversation lock.
pub(crate) enum LockOutcome {
    /// Lock acquired successfully.
    Acquired(ConversationLock),

    /// User chose to start a new conversation instead.
    NewConversation,

    /// User chose to fork the locked conversation.
    ForkConversation(ConversationHandle),
}

/// Parameters for [`acquire_lock`].
pub(crate) struct LockRequest<'a> {
    pub workspace: &'a Workspace,
    pub handle: ConversationHandle,
    pub is_tty: bool,
    pub session: Option<&'a Session>,
    pub printer: &'a Printer,

    /// Whether to offer "Start a new conversation" on contention.
    pub allow_new: bool,

    /// Whether to offer "Fork this conversation" on contention.
    pub allow_fork: bool,
}

impl<'a> LockRequest<'a> {
    pub fn from_ctx(handle: ConversationHandle, ctx: &'a Ctx) -> Self {
        Self {
            workspace: &ctx.workspace,
            handle,
            is_tty: ctx.term.is_tty,
            session: ctx.session.as_ref(),
            printer: &ctx.printer,
            allow_new: false,
            allow_fork: false,
        }
    }

    #[must_use]
    pub fn allow_new(mut self, new: bool) -> Self {
        self.allow_new = new;
        self
    }

    #[must_use]
    pub fn allow_fork(mut self, fork: bool) -> Self {
        self.allow_fork = fork;
        self
    }
}

/// Acquire an exclusive conversation lock with polling and timeout.
///
/// On timeout in interactive terminals, shows a selection prompt. The available
/// options depend on `allow_new` and `allow_fork`. In non-interactive
/// environments, fails with `LockTimeout`.
pub(crate) fn acquire_lock(mut r: LockRequest<'_>) -> Result<LockOutcome> {
    let id = r.handle.id();
    let timeout = lock_timeout();
    let start = Instant::now();

    loop {
        r.handle = match r.workspace.lock_conversation(r.handle, r.session)? {
            LockResult::Acquired(lock) => return Ok(LockOutcome::Acquired(lock)),
            _ if !r.is_tty => return Err(Error::LockTimeout(id)),
            LockResult::AlreadyLocked(handle) => handle,
        };

        if start.elapsed() < timeout {
            thread::sleep(Duration::from_millis(500));
            continue;
        }

        return prompt_contention(r);
    }
}

fn prompt_contention(r: LockRequest<'_>) -> Result<LockOutcome> {
    let id = r.handle.id();
    let msg = lock_contention_message(r.workspace, id);

    let mut options = vec!["Continue waiting"];
    if r.allow_new {
        options.push("Start a new conversation");
    }
    if r.allow_fork {
        options.push("Fork this conversation");
    }
    options.push("Cancel");

    let selected = Select::new(&msg, options).prompt_with_writer(&mut r.printer.err_writer())?;

    match selected {
        "Continue waiting" => acquire_lock(r),
        "Start a new conversation" => Ok(LockOutcome::NewConversation),
        "Fork this conversation" => Ok(LockOutcome::ForkConversation(r.handle)),
        _ => Err(Error::LockTimeout(id)),
    }
}

fn lock_contention_message(workspace: &Workspace, id: ConversationId) -> String {
    let mut msg = format!("Conversation {id} is locked ");
    match workspace.conversation_lock_info(&id) {
        Some(LockInfo { pid, session, .. }) => {
            let who = match &session {
                Some(s) => format!("pid {pid}, session {s}"),
                None => format!("pid {pid}"),
            };
            msg.push_str(&format!("({who})"));
        }
        None => msg.push_str("by another session."),
    }

    msg
}

/// Create a lock timeout from the [`LOCK_DURATION_ENV`] environment variable,
/// defaulting to 30 seconds.
///
/// The timeout is parsed as a [`humantime::Duration`].
fn lock_timeout() -> Duration {
    env::var(LOCK_DURATION_ENV)
        .ok()
        .and_then(|val| val.parse::<humantime::Duration>().ok())
        .map_or(Duration::from_secs(30), Duration::from)
}
