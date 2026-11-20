//! Shared conversation lock acquisition with polling, timeout, and interactive
//! prompts.
//!
//! [`acquire_lock`] takes the lock of a conversation in the conversation list,
//! and [`wait_until_free`] waits until another process lets go of any
//! conversation's lock, for a caller that takes the lock itself (an archived
//! conversation, or one that does not load).
//! Both poll at 500ms intervals until the lock is free or a timeout is reached.
//! On timeout (or ctrl-c), interactive terminals get a prompt; non-interactive
//! environments get `LockTimeout` at once, without waiting.
//!
//! While polling, a status region is shown (controlled by [`LockWaitConfig`])
//! so the user sees immediate feedback.
//! It is released before the interactive prompt appears.
//!
//! The prompt options are controlled by [`LockRequest::allow_new`] and
//! [`LockRequest::allow_fork`].
//! When both are false, only "Continue waiting" and "Cancel" are shown.

use std::time::{Duration, Instant};

use inquire::Select;
use jp_config::style::lock_wait::LockWaitConfig;
use jp_conversation::ConversationId;
use jp_printer::{Printer, RegionStyle, StatusRegion};
use jp_storage::lock::LockInfo;
use jp_workspace::{ConversationHandle, ConversationLock, LockResult, Workspace, session::Session};

use crate::{
    ctx::Ctx,
    error::{Error, Result},
    signals::SignalRouter,
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

    /// Whether a user is present to answer the contention prompt.
    pub interactive: bool,
    pub session: Option<&'a Session>,
    pub printer: &'a Printer,
    pub signals: &'a SignalRouter,

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
            interactive: ctx.term.interactive,
            session: ctx.session.as_ref(),
            printer: &ctx.printer,
            signals: &ctx.signals,
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

    fn wait(&self) -> Wait<'a> {
        Wait {
            workspace: self.workspace,
            interactive: self.interactive,
            printer: self.printer,
            signals: self.signals,
            lock_wait: self.lock_wait.clone(),
            allow_new: self.allow_new,
            allow_fork: self.allow_fork,
        }
    }
}

/// Acquire an exclusive conversation lock with polling and timeout.
///
/// On timeout (or ctrl-c) in interactive terminals, shows a selection prompt.
/// The available options depend on `allow_new` and `allow_fork`.
/// In non-interactive environments, fails with `LockTimeout`.
///
/// While polling, a status region counts down how much of the wait is left,
/// giving the user immediate visual feedback.
pub(crate) async fn acquire_lock(r: LockRequest<'_>) -> Result<LockOutcome> {
    let id = r.handle.id();
    let wait = r.wait();
    let (workspace, session) = (r.workspace, r.session);

    let waited = wait
        .until(id, || {
            let handle = workspace.acquire_conversation(&id)?;
            Ok(match workspace.lock_conversation(handle, session)? {
                LockResult::Acquired(lock) => Some(lock),
                LockResult::AlreadyLocked(_) => None,
            })
        })
        .await?;

    Ok(match waited {
        Waited::Got(lock) => LockOutcome::Acquired(lock),
        Waited::New => LockOutcome::NewConversation,
        Waited::Fork => LockOutcome::ForkConversation(workspace.acquire_conversation(&id)?),
    })
}

/// Wait until no other process holds conversation `id`'s lock.
///
/// For a conversation outside the conversation list, whose lock the caller
/// takes itself once this returns.
/// Another process can take the lock again in between; a caller that then finds
/// it held waits again.
/// Behaves as [`acquire_lock`] does: a status region while polling, the
/// "Continue waiting" or "Cancel" prompt on timeout, and `LockTimeout` at once
/// when nobody can answer it.
pub(crate) async fn wait_until_free(ctx: &Ctx, id: ConversationId) -> Result<()> {
    let wait = Wait {
        workspace: &ctx.workspace,
        interactive: ctx.term.interactive,
        printer: &ctx.printer,
        signals: &ctx.signals,
        lock_wait: ctx.config().style.lock_wait.clone(),
        allow_new: false,
        allow_fork: false,
    };

    match wait
        .until(id, || {
            Ok(ctx.workspace.conversation_lock_is_free(&id)?.then_some(()))
        })
        .await?
    {
        Waited::Got(()) => Ok(()),
        Waited::New | Waited::Fork => Err(Error::LockTimeout(id)),
    }
}

/// How a wait for a lock ended.
enum Waited<T> {
    /// The attempt succeeded.
    Got(T),

    /// The user chose to start a new conversation.
    New,

    /// The user chose to fork the conversation.
    Fork,
}

/// The user's answer to the contention prompt.
enum Choice {
    Wait,
    New,
    Fork,
    Cancel,
}

/// What waiting for a lock shows and offers.
struct Wait<'a> {
    workspace: &'a Workspace,
    interactive: bool,
    printer: &'a Printer,
    signals: &'a SignalRouter,
    lock_wait: LockWaitConfig,
    allow_new: bool,
    allow_fork: bool,
}

impl Wait<'_> {
    /// Run `attempt` until it returns `Some`, waiting for conversation `id`'s
    /// lock between attempts.
    async fn until<T>(
        &self,
        id: ConversationId,
        mut attempt: impl FnMut() -> Result<Option<T>>,
    ) -> Result<Waited<T>> {
        loop {
            if let Some(got) = attempt()? {
                return Ok(Waited::Got(got));
            }
            if !self.interactive {
                return Err(Error::LockTimeout(id));
            }
            if let Some(got) = self.poll(id, &mut attempt).await? {
                return Ok(Waited::Got(got));
            }

            match self.prompt(id)? {
                Choice::Wait => {}
                Choice::New => return Ok(Waited::New),
                Choice::Fork => return Ok(Waited::Fork),
                Choice::Cancel => return Err(Error::LockTimeout(id)),
            }
        }
    }

    /// Retry `attempt` every 500ms until it succeeds, the wait times out, or
    /// the user presses ctrl-c; `None` for the last two.
    async fn poll<T>(
        &self,
        id: ConversationId,
        attempt: &mut impl FnMut() -> Result<Option<T>>,
    ) -> Result<Option<T>> {
        let timeout = Duration::from_secs(u64::from(self.lock_wait.timeout_secs));
        let start = Instant::now();

        let holder = lock_holder_description(self.workspace, id);
        let mut region = claim_lock_region(self.printer, &self.lock_wait, &holder, timeout);

        // While waiting, a Ctrl-C press skips straight to the contention prompt.
        // The guard drops before the prompt shows, so a press while the prompt
        // is up escalates through the router instead of being swallowed here.
        let (_interrupt_guard, mut interrupt_rx) = self.signals.push_handler();

        loop {
            // Wait for the next poll tick, but also listen for interrupts.
            tokio::select! {
                biased;
                Some(notice) = interrupt_rx.recv() => {
                    region.release();
                    // The press is answered by the contention prompt, so it no
                    // longer counts toward the escalation ladder: a press after
                    // the prompt is answered should reach a handler, not bypass
                    // the stack.
                    notice.handled();
                    return Ok(None);
                }
                () = tokio::time::sleep(Duration::from_millis(500)) => {}
            }

            if let Some(got) = attempt()? {
                region.release();
                return Ok(Some(got));
            }

            if start.elapsed() >= timeout {
                region.release();
                return Ok(None);
            }
        }
    }

    fn prompt(&self, id: ConversationId) -> Result<Choice> {
        let msg = lock_contention_message(self.workspace, id);

        let mut options = vec!["Continue waiting"];
        if self.allow_new {
            options.push("Start a new conversation");
        }
        if self.allow_fork {
            options.push("Fork this conversation");
        }
        options.push("Cancel");

        // The prompt channel, not the error stream: this asks the user a
        // question, and a run that silences chrome must not silence the
        // question it is blocking on.
        let selected =
            Select::new(&msg, options).prompt_with_writer(&mut self.printer.prompt_writer())?;

        Ok(match selected {
            "Continue waiting" => Choice::Wait,
            "Start a new conversation" => Choice::New,
            "Fork this conversation" => Choice::Fork,
            _ => Choice::Cancel,
        })
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

/// Claim the status region that counts down the remaining wait.
///
/// Returns an inert region when `style.lock_wait.show` is off, or when the
/// terminal cannot carry one.
fn claim_lock_region(
    printer: &Printer,
    config: &LockWaitConfig,
    holder: &str,
    timeout: Duration,
) -> StatusRegion {
    if !config.show {
        return StatusRegion::inert();
    }

    let holder = holder.to_owned();
    let total = timeout.as_secs_f64();

    printer.status_region(RegionStyle::new(
        Duration::from_secs(u64::from(config.delay_secs)),
        Duration::from_millis(u64::from(config.interval_ms)),
        move |elapsed, _detail| {
            let remaining = (total - elapsed).max(0.0);
            format!("⏱ Pending conversation lock {holder} ({remaining:.1}s)")
        },
    ))
}
