//! Shared conversation lock acquisition with polling, timeout, and interactive
//! prompts.
//!
//! [`acquire_lock`] polls `Workspace::lock_conversation` at 500ms intervals
//! until the lock is acquired or a timeout is reached. On timeout (or ctrl-c),
//! interactive terminals get a prompt; non-interactive environments get
//! `LockTimeout`.
//!
//! While polling, a timer indicator is shown (controlled by [`LockWaitConfig`])
//! so the user sees immediate feedback. The timer is cleared before the
//! interactive prompt appears.
//!
//! The prompt options are controlled by [`LockRequest::allow_new`] and
//! [`LockRequest::allow_fork`]. When both are false, only "Continue waiting"
//! and "Cancel" are shown.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use inquire::Select;
use jp_config::style::lock_wait::LockWaitConfig;
use jp_conversation::ConversationId;
use jp_printer::Printer;
use jp_storage::lock::LockInfo;
use jp_workspace::{ConversationHandle, ConversationLock, LockResult, Workspace, session::Session};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    ctx::Ctx,
    error::{Error, Result},
    signals::SignalRx,
    timer::spawn_line_timer,
};

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
    pub signals: SignalRx,

    /// Lock-wait progress indicator configuration.
    pub lock_wait: LockWaitConfig,

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
            signals: ctx.signals.receiver.resubscribe(),
            lock_wait: ctx.config().style.lock_wait.clone(),
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
/// On timeout (or ctrl-c) in interactive terminals, shows a selection prompt.
/// The available options depend on `allow_new` and `allow_fork`. In
/// non-interactive environments, fails with `LockTimeout`.
///
/// While polling, a `\r`-based timer line shows how long the CLI has been
/// waiting, giving the user immediate visual feedback.
pub(crate) async fn acquire_lock(mut r: LockRequest<'_>) -> Result<LockOutcome> {
    let id = r.handle.id();
    let timeout = Duration::from_secs(u64::from(r.lock_wait.timeout_secs));
    let start = Instant::now();

    // First attempt — no timer yet.
    r.handle = match r.workspace.lock_conversation(r.handle, r.session)? {
        LockResult::Acquired(lock) => return Ok(LockOutcome::Acquired(lock)),
        _ if !r.is_tty => return Err(Error::LockTimeout(id)),
        LockResult::AlreadyLocked(handle) => handle,
    };

    let holder = lock_holder_description(r.workspace, id);
    let timer = spawn_lock_timer(r.printer, &r.lock_wait, &holder, timeout);

    loop {
        // Wait for the next poll tick, but also listen for OS signals.
        // On ctrl-c (Shutdown), skip straight to the interactive prompt.
        tokio::select! {
            biased;
            Ok(_) = r.signals.recv() => {
                cancel_timer(timer).await;
                return prompt_contention(r).await;
            }
            () = tokio::time::sleep(Duration::from_millis(500)) => {}
        }

        r.handle = match r.workspace.lock_conversation(r.handle, r.session)? {
            LockResult::Acquired(lock) => {
                cancel_timer(timer).await;
                return Ok(LockOutcome::Acquired(lock));
            }
            LockResult::AlreadyLocked(handle) => handle,
        };

        if start.elapsed() >= timeout {
            cancel_timer(timer).await;
            return prompt_contention(r).await;
        }
    }
}

async fn prompt_contention(r: LockRequest<'_>) -> Result<LockOutcome> {
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
        "Continue waiting" => Box::pin(acquire_lock(r)).await,
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

/// Build a compact lock holder description for the timer line.
fn lock_holder_description(workspace: &Workspace, id: ConversationId) -> String {
    match workspace.conversation_lock_info(&id) {
        Some(LockInfo { pid, session, .. }) => match &session {
            Some(s) => format!("(pid: {pid}, session: {s})"),
            None => format!("(pid: {pid})"),
        },
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Lock-wait timer (delegates to the shared spawn_line_timer infrastructure)
// ---------------------------------------------------------------------------

type Timer = Option<(CancellationToken, JoinHandle<()>)>;

fn spawn_lock_timer(
    printer: &Printer,
    config: &LockWaitConfig,
    holder: &str,
    timeout: Duration,
) -> Timer {
    let holder = holder.to_owned();
    let total = timeout.as_secs_f64();
    spawn_line_timer(
        Arc::new(printer.clone()),
        config.show,
        Duration::from_secs(u64::from(config.delay_secs)),
        Duration::from_millis(u64::from(config.interval_ms)),
        move |elapsed| {
            let remaining = (total - elapsed).max(0.0);
            format!("\r\x1b[K⏱ Pending conversation lock {holder} ({remaining:.1}s)")
        },
    )
}

async fn cancel_timer(timer: Timer) {
    if let Some((token, handle)) = timer {
        token.cancel();
        drop(handle.await);
    }
}
