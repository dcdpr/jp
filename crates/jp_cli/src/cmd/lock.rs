//! Shared conversation lock acquisition with polling, timeout, and interactive
//! prompts.
//!
//! [`acquire_lock`] polls `Workspace::lock_conversation` at 500ms intervals
//! until the lock is acquired or a timeout is reached. On timeout, interactive
//! terminals get a prompt; non-interactive environments get `LockTimeout`.
//!
//! While polling, a timer indicator is shown (controlled by [`LockWaitConfig`])
//! so the user sees immediate feedback. The timer is cleared before the
//! interactive prompt appears.
//!
//! The prompt options are controlled by [`LockRequest::allow_new`] and
//! [`LockRequest::allow_fork`]. When both are false, only "Continue waiting"
//! and "Cancel" are shown.

use std::{
    env,
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use inquire::Select;
use jp_config::style::lock_wait::LockWaitConfig;
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
/// On timeout in interactive terminals, shows a selection prompt. The available
/// options depend on `allow_new` and `allow_fork`. In non-interactive
/// environments, fails with `LockTimeout`.
///
/// While polling, a `\r`-based timer line shows how long the CLI has been
/// waiting, giving the user immediate visual feedback.
pub(crate) fn acquire_lock(mut r: LockRequest<'_>) -> Result<LockOutcome> {
    let id = r.handle.id();
    let timeout = lock_timeout();
    let start = Instant::now();

    // First attempt — no timer yet.
    r.handle = match r.workspace.lock_conversation(r.handle, r.session)? {
        LockResult::Acquired(lock) => return Ok(LockOutcome::Acquired(lock)),
        _ if !r.is_tty => return Err(Error::LockTimeout(id)),
        LockResult::AlreadyLocked(handle) => handle,
    };

    // Build timer message from current lock holder info.
    let holder = lock_holder_description(r.workspace, id);
    let timer = LockTimer::spawn(r.printer, &r.lock_wait, &holder);

    loop {
        thread::sleep(Duration::from_millis(500));

        r.handle = match r.workspace.lock_conversation(r.handle, r.session)? {
            LockResult::Acquired(lock) => {
                timer.cancel();
                return Ok(LockOutcome::Acquired(lock));
            }
            LockResult::AlreadyLocked(handle) => handle,
        };

        if start.elapsed() >= timeout {
            timer.cancel();
            return prompt_contention(r);
        }
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

/// Build a human-readable description of the lock holder for the timer line.
fn lock_holder_description(workspace: &Workspace, id: ConversationId) -> String {
    match workspace.conversation_lock_info(&id) {
        Some(LockInfo { pid, session, .. }) => match &session {
            Some(s) => format!("by pid {pid} in session {s}"),
            None => format!("by pid {pid}"),
        },
        None => "by another session".to_owned(),
    }
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

/// A background thread that shows a waiting indicator with an incrementing
/// seconds counter. Cancelled via an [`AtomicBool`].
struct LockTimer {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LockTimer {
    /// Spawn the timer thread. Respects `config.show` - returns a no-op timer
    /// when the indicator is disabled.
    fn spawn(printer: &Printer, config: &LockWaitConfig, holder: &str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));

        if !config.show {
            return Self { stop, handle: None };
        }

        let delay = Duration::from_secs(config.delay_secs.into());
        let interval =
            Duration::from_millis(config.interval_ms.into()).max(Duration::from_millis(50));
        let printer = printer.clone();
        let holder = holder.to_owned();
        let stop_flag = stop.clone();

        let handle = thread::spawn(move || {
            let start = Instant::now();

            // Wait for the initial delay, checking for cancellation.
            while start.elapsed() < delay {
                if stop_flag.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }

            // Show the timer until cancelled.
            loop {
                if stop_flag.load(Ordering::Relaxed) {
                    let _ = write!(printer.err_writer(), "\r\x1b[K");
                    return;
                }

                let secs = start.elapsed().as_secs();
                let _ = write!(
                    printer.err_writer(),
                    "\r\x1b[K⏱ Waiting for conversation lock to be released {holder} ({secs}s)"
                );

                thread::sleep(interval);
            }
        });

        Self {
            stop,
            handle: Some(handle),
        }
    }

    /// Signal the timer to stop and wait for the thread to finish.
    fn cancel(self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle {
            drop(handle.join());
        }
    }
}
