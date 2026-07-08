//! OS signal routing.
//!
//! [`SignalRouter`] is the process's single consumer of OS signals (RFD 045).
//! It owns the root shutdown [`CancellationToken`], tracks Ctrl-C escalation,
//! and dispatches the first Ctrl-C press to the topmost entry on a LIFO stack
//! of scoped interrupt handlers.
//!
//! Ctrl-C escalates: the first press notifies the topmost registered handler
//! (or requests a graceful shutdown when nothing handles interrupts), a second
//! press within the cooldown window bypasses all handlers and cancels the
//! shutdown token, and any press after shutdown has begun exits the process
//! immediately.
//! SIGTERM requests a graceful shutdown; SIGQUIT exits.
//! Neither goes through the handler stack.
//!
//! Scopes that can act on an interrupt (an event loop that shows an interrupt
//! menu, for example) register themselves with [`SignalRouter::push_handler`]
//! and poll the returned receiver alongside their other event sources.
//! The interrupt logic runs in the registering event loop's own context, never
//! on the router's signal task, so handlers can block on interactive prompts
//! and act on the result immediately.
//!
//! Code without an interrupt handler cooperates through the shutdown token
//! ([`SignalRouter::shutdown_token`]) instead: awaiting its cancellation (or
//! checking `is_cancelled`) is how teardown and long-running work observe a
//! graceful shutdown request.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use futures::{Stream, StreamExt as _};
use tokio::{
    runtime::{Handle, Runtime},
    sync::mpsc::{self, error::TrySendError},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::debug;

/// Exit code for a process terminated by escalated Ctrl-C presses (128 +
/// SIGINT).
const SIGINT_EXIT_CODE: i32 = 130;

/// Exit code for a process terminated by SIGQUIT (128 + SIGQUIT).
const SIGQUIT_EXIT_CODE: i32 = 131;

/// A raw OS signal, as consumed by the router's signal task.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OsSignal {
    /// SIGINT (Ctrl-C): routed through escalation and the handler stack.
    Interrupt,

    /// SIGTERM (Ctrl-Break on Windows): requests a graceful shutdown.
    Terminate,

    /// SIGQUIT: exits the process.
    #[cfg_attr(windows, allow(clippy::allow_attributes, dead_code))]
    Quit,
}

/// Where the router delivered a signal.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Routed {
    /// The topmost handler on the stack was notified.
    Handler,

    /// The shutdown token was cancelled (graceful shutdown).
    Shutdown,

    /// The process must exit immediately with the given code.
    Exit(i32),
}

/// Routes OS signals to interrupt handlers, the shutdown token, or the process
/// exit path.
///
/// Created once at application startup; the embedded signal task lives for the
/// duration of the process.
pub struct SignalRouter {
    inner: Arc<RouterInner>,

    /// Keeps the signal-consuming task attached to the router.
    /// The task runs until the signal source ends (never, for the OS-backed
    /// source); the handle is never awaited or aborted.
    _signal_task: JoinHandle<()>,
}

impl SignalRouter {
    /// Create the router and start consuming OS signals on `runtime`.
    ///
    /// `escalation_cooldown` is how long the Ctrl-C escalation counter survives
    /// without a new press; a press arriving after the window counts as a fresh
    /// first press.
    pub fn new(runtime: &Runtime, escalation_cooldown: Duration) -> Self {
        #[cfg(unix)]
        let signals = os_signals(runtime);

        // If we passed `runtime` here, we would get the following:
        // error[E0521]: borrowed data escapes outside of associated function
        #[cfg(windows)]
        let signals = os_signals();

        Self::with_signal_source(runtime.handle(), signals, escalation_cooldown, |code| {
            std::process::exit(code)
        })
    }

    /// Create the router from an arbitrary signal source and exit action.
    ///
    /// This is the dependency-inversion seam behind [`Self::new`], which binds
    /// `signals` to the OS and `on_exit` to [`std::process::exit`].
    /// Tests bind an in-memory channel and an exit recorder instead, driving
    /// the real routing logic end to end without ending the test process.
    ///
    /// `on_exit` runs on the signal task when routing escalates to a process
    /// exit ([`Routed::Exit`]).
    /// The task keeps consuming signals if `on_exit` returns (production's exit
    /// action never does), so a recording action observes every exit decision,
    /// not just the first.
    pub(crate) fn with_signal_source(
        handle: &Handle,
        signals: impl Stream<Item = OsSignal> + Send + 'static,
        escalation_cooldown: Duration,
        on_exit: impl Fn(i32) + Send + 'static,
    ) -> Self {
        let inner = RouterInner::new(escalation_cooldown);

        let router = inner.clone();
        let signal_task = handle.spawn(async move {
            tokio::pin!(signals);

            while let Some(signal) = signals.next().await {
                match router.route(signal) {
                    Routed::Exit(code) => on_exit(code),
                    routed => debug!(?signal, ?routed, "Routed OS signal."),
                }
            }
        });

        Self {
            inner,
            _signal_task: signal_task,
        }
    }

    /// The root shutdown token.
    ///
    /// Cancelled when a graceful shutdown is requested: an unhandled or
    /// escalated Ctrl-C, or SIGTERM.
    /// Any async code can await the token (or check `is_cancelled`) to stop
    /// cooperatively.
    #[must_use]
    pub fn shutdown_token(&self) -> CancellationToken {
        self.inner.shutdown_token.clone()
    }

    /// Register an interrupt handler scope.
    ///
    /// Returns a guard (drop to deregister) and a receiver that fires when
    /// SIGINT arrives while this handler is topmost.
    /// The registering event loop polls the receiver alongside its other
    /// sources and runs its interrupt logic in its own context when the
    /// receiver fires.
    #[must_use]
    pub fn push_handler(&self) -> (InterruptGuard, mpsc::Receiver<()>) {
        self.inner.push_handler()
    }

    /// Called by a handler's event loop when it declines to handle the current
    /// interrupt.
    ///
    /// The router notifies the next handler on the stack, or falls back to
    /// graceful shutdown when no other handler exists.
    pub fn decline(&self) {
        self.inner.notify_next_or_shutdown();
    }
}

/// Deregisters its interrupt handler from the router's stack on drop.
pub struct InterruptGuard {
    inner: Arc<RouterInner>,
    id: HandlerId,
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        self.inner.remove(self.id);
    }
}

/// Identity of a registered handler, unique for the process lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HandlerId(u64);

/// A handler scope on the router's stack.
struct RegisteredHandler {
    id: HandlerId,

    /// Notifies the handler's event loop that SIGINT arrived.
    /// The event loop runs the interrupt logic; the router never does.
    notify_tx: mpsc::Sender<()>,
}

/// Ctrl-C press tracking for escalation.
#[derive(Debug)]
struct EscalationState {
    /// Consecutive presses within the cooldown window.
    presses: u32,

    /// When the most recent press arrived.
    last_press: Option<Instant>,

    /// How long the counter survives without a new press.
    cooldown: Duration,
}

impl EscalationState {
    fn new(cooldown: Duration) -> Self {
        Self {
            presses: 0,
            last_press: None,
            cooldown,
        }
    }

    /// Record a press at `now`, returning the press count it escalated to.
    ///
    /// The count restarts at 1 when the previous press is older than the
    /// cooldown.
    fn bump(&mut self, now: Instant) -> u32 {
        self.presses = match self.last_press {
            Some(last) if now.duration_since(last) <= self.cooldown => self.presses + 1,
            _ => 1,
        };
        self.last_press = Some(now);
        self.presses
    }
}

struct RouterInner {
    /// The LIFO stack of interrupt handler scopes.
    /// Only the topmost entry is notified on the first Ctrl-C press.
    stack: Mutex<Vec<RegisteredHandler>>,

    /// Source for unique handler ids.
    next_handler_id: AtomicU64,

    /// Escalation state (press count, last timestamp).
    escalation: Mutex<EscalationState>,

    /// Cancelled on graceful shutdown (unhandled or escalated Ctrl-C, SIGTERM).
    shutdown_token: CancellationToken,
}

impl RouterInner {
    /// Create the router state with an empty handler stack.
    fn new(escalation_cooldown: Duration) -> Arc<Self> {
        Arc::new(Self {
            stack: Mutex::new(Vec::new()),
            next_handler_id: AtomicU64::new(0),
            escalation: Mutex::new(EscalationState::new(escalation_cooldown)),
            shutdown_token: CancellationToken::new(),
        })
    }

    fn route(&self, signal: OsSignal) -> Routed {
        self.route_at(signal, Instant::now())
    }

    fn route_at(&self, signal: OsSignal, now: Instant) -> Routed {
        match signal {
            OsSignal::Interrupt => self.route_interrupt(now),

            // SIGTERM requests a graceful shutdown; it never goes through the
            // handler stack.
            OsSignal::Terminate => {
                self.shutdown_token.cancel();
                Routed::Shutdown
            }

            // SIGQUIT exits the process immediately.
            OsSignal::Quit => Routed::Exit(SIGQUIT_EXIT_CODE),
        }
    }

    /// Route a Ctrl-C press through the escalation ladder.
    ///
    /// First press: notify the topmost handler, or request a graceful shutdown
    /// when no handler is registered.
    /// Second press within the cooldown: bypass all handlers and request a
    /// graceful shutdown.
    /// Any press once shutdown has begun: exit the process.
    fn route_interrupt(&self, now: Instant) -> Routed {
        let presses = self
            .escalation
            .lock()
            .expect("escalation state lock poisoned")
            .bump(now);

        // Shutdown is already in progress; the user is done waiting.
        if self.shutdown_token.is_cancelled() || presses >= 3 {
            return Routed::Exit(SIGINT_EXIT_CODE);
        }

        if presses == 2 {
            self.shutdown_token.cancel();
            return Routed::Shutdown;
        }

        if let Some(notify_tx) = self.topmost() {
            return match notify_tx.try_send(()) {
                // A full channel means the handler already has a pending
                // interrupt notification; nothing to add.
                Ok(()) | Err(TrySendError::Full(())) => Routed::Handler,

                // The handler's event loop is gone but its guard hasn't
                // dropped yet. Treat it as declined and fall back to
                // graceful shutdown.
                Err(TrySendError::Closed(())) => {
                    self.shutdown_token.cancel();
                    Routed::Shutdown
                }
            };
        }

        self.shutdown_token.cancel();
        Routed::Shutdown
    }

    /// Clone the topmost handler's notification channel.
    fn topmost(&self) -> Option<mpsc::Sender<()>> {
        self.stack
            .lock()
            .expect("handler stack lock poisoned")
            .last()
            .map(|handler| handler.notify_tx.clone())
    }

    /// Register a handler scope: push a fresh notification channel onto the
    /// stack and return the deregistration guard plus the receiver.
    fn push_handler(self: &Arc<Self>) -> (InterruptGuard, mpsc::Receiver<()>) {
        let (notify_tx, notify_rx) = mpsc::channel(1);
        let id = HandlerId(self.next_handler_id.fetch_add(1, Ordering::Relaxed));
        self.stack
            .lock()
            .expect("handler stack lock poisoned")
            .push(RegisteredHandler { id, notify_tx });

        (
            InterruptGuard {
                inner: self.clone(),
                id,
            },
            notify_rx,
        )
    }

    /// Remove a handler by id.
    ///
    /// Id-based rather than positional so guards can drop in any order: early
    /// returns and panic unwinding never corrupt the stack.
    fn remove(&self, id: HandlerId) {
        let mut stack = self.stack.lock().expect("handler stack lock poisoned");
        if let Some(index) = stack.iter().position(|handler| handler.id == id) {
            stack.remove(index);
        }
    }

    /// Notify the handler below the topmost one, or request a graceful shutdown
    /// when no other handler exists.
    fn notify_next_or_shutdown(&self) {
        let next = {
            let stack = self.stack.lock().expect("handler stack lock poisoned");
            stack
                .iter()
                .rev()
                .nth(1)
                .map(|handler| handler.notify_tx.clone())
        };

        let Some(notify_tx) = next else {
            self.shutdown_token.cancel();
            return;
        };

        match notify_tx.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Closed(())) => self.shutdown_token.cancel(),
        }
    }
}

/// Signals from OS/user.
#[cfg(unix)]
fn os_signals(runtime: &Runtime) -> impl Stream<Item = OsSignal> + use<> {
    use async_stream::stream;
    use tokio::signal::unix::{SignalKind, signal};
    use tracing::info;

    // The `signal` function must be run within the context of a Tokio runtime.
    runtime.block_on(async {
        let mut sigint = signal(SignalKind::interrupt()).expect("Failed to set up SIGINT handler.");
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler.");
        let mut sigquit = signal(SignalKind::quit()).expect("Failed to set up SIGQUIT handler.");

        stream!({
            loop {
                let signal = jp_macro::select!(
                    sigint.recv(), // ctrl-c
                    |_signal| {
                        info!(message = "Signal received.", signal = "SIGINT");
                        OsSignal::Interrupt
                    },
                    sigterm.recv(),
                    |_signal| {
                        info!(message = "Signal received.", signal = "SIGTERM");
                        OsSignal::Terminate
                    },
                    sigquit.recv(),
                    |_signal| {
                        info!(message = "Signal received.", signal = "SIGQUIT");
                        OsSignal::Quit
                    },
                );

                yield signal;
            }
        })
    })
}

/// Signals from OS/user.
#[cfg(windows)]
fn os_signals() -> impl Stream<Item = OsSignal> {
    use async_stream::stream;
    use tokio::signal::windows::{ctrl_break, ctrl_c};
    use tracing::info;

    stream! {
        let mut ctrl_c = ctrl_c().expect("Failed to set up Ctrl-C handler.");
        let mut ctrl_break = ctrl_break().expect("Failed to set up Ctrl-Break handler.");

        loop {
            // Ctrl-C carries interrupt semantics and escalates like SIGINT.
            // A parent process can only target a specific child's process
            // group with Ctrl-Break, so it maps to the graceful shutdown
            // request a supervisor expects from SIGTERM.
            let signal = jp_macro::select!(
                ctrl_c.recv(),
                |_signal| {
                    info!(message = "Signal received.", signal = "CTRL_C");
                    OsSignal::Interrupt
                },
                ctrl_break.recv(),
                |_signal| {
                    info!(message = "Signal received.", signal = "CTRL_BREAK");
                    OsSignal::Terminate
                },
            );

            yield signal;
        }
    }
}

#[cfg(test)]
#[path = "signals_testing.rs"]
pub(crate) mod testing;

#[cfg(test)]
#[path = "signals_tests.rs"]
mod tests;
