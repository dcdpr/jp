//! Graceful shutdown coordination for CLI commands.
//!
//! A graceful shutdown is triggered by an unhandled or escalated Ctrl-C or by
//! SIGTERM, which cancels the signal router's shutdown token.
//!
//! Every command runs under [`until_shutdown`].
//! When shutdown is requested, the command future is dropped immediately unless
//! it holds a claim in [`ShutdownBudgets`].
//! Subsystems that need asynchronous cleanup (such as stopping a command plugin
//! or waiting for delegated turns to persist state) claim a budget via
//! [`ShutdownBudgets::claim`] and observe the shutdown token themselves.
//! [`until_shutdown`] waits until all claims are released or the longest claim
//! expires before dropping the command.

use std::{sync::Arc, time::Duration};

use tokio::{runtime::Handle, sync::watch, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::cmd;

/// Tracks grace-period budgets claimed by the running command for graceful
/// shutdown.
///
/// Commands without active claims are dropped as soon as shutdown is requested.
/// Subsystems that cannot clean up synchronously on drop claim a budget with
/// [`Self::claim`] for the duration of their work and handle the shutdown token
/// directly.
///
/// Cloning shares the underlying claim set.
#[derive(Debug, Clone)]
pub(crate) struct ShutdownBudgets {
    /// Notifies [`until_shutdown`] when claims are added or removed.
    claims: Arc<watch::Sender<Claims>>,
}

impl Default for ShutdownBudgets {
    fn default() -> Self {
        let (claims, _) = watch::channel(Claims::default());

        Self {
            claims: Arc::new(claims),
        }
    }
}

impl ShutdownBudgets {
    /// Claim `budget` for graceful shutdown cleanup until the returned guard is
    /// dropped.
    ///
    /// When multiple claims are active, the maximum claimed duration applies.
    #[must_use]
    pub(crate) fn claim(&self, budget: Duration) -> ShutdownBudgetGuard {
        let mut id = ClaimId(0);

        self.claims.send_modify(|claims| {
            id = ClaimId(claims.next_id);
            claims.next_id += 1;
            claims.held.push((id, budget));
        });

        ShutdownBudgetGuard {
            claims: Arc::clone(&self.claims),
            id,
        }
    }

    /// Return the maximum claimed budget, or `None` if no claims are active.
    #[cfg(test)]
    pub(crate) fn longest(&self) -> Option<Duration> {
        self.claims.borrow().longest()
    }
}

/// Guard that releases a [`ShutdownBudgets`] claim when dropped.
pub(crate) struct ShutdownBudgetGuard {
    claims: Arc<watch::Sender<Claims>>,
    id: ClaimId,
}

impl Drop for ShutdownBudgetGuard {
    fn drop(&mut self) {
        self.claims
            .send_modify(|claims| claims.held.retain(|(id, _)| *id != self.id));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClaimId(u64);

#[derive(Debug, Default)]
struct Claims {
    next_id: u64,
    held: Vec<(ClaimId, Duration)>,
}

impl Claims {
    fn longest(&self) -> Option<Duration> {
        self.held.iter().map(|(_, budget)| *budget).max()
    }
}

/// Run `command` until it completes or a graceful shutdown budget expires.
///
/// If `command` holds no claims in `budgets` when `shutdown` is cancelled, it
/// is dropped immediately, releasing conversation locks and flushing dirty
/// conversations via guard-scoped persistence.
///
/// If `command` holds one or more claims, it continues running until it
/// returns, releases all claims, or exceeds the longest claimed budget
/// (measured from when `shutdown` was cancelled).
///
/// Returns [`cmd::Error::interrupted`] if `command` is dropped due to shutdown.
pub(crate) async fn until_shutdown(
    command: impl Future<Output = cmd::Output>,
    shutdown: &CancellationToken,
    budgets: &ShutdownBudgets,
) -> cmd::Output {
    tokio::select! {
        biased;
        output = command => output,
        () = out_of_budget(shutdown, budgets) => Err(cmd::Error::interrupted()),
    }
}

/// Wait for `shutdown` to be cancelled, then wait until all claims in `budgets`
/// are released or the longest claimed duration elapses.
async fn out_of_budget(shutdown: &CancellationToken, budgets: &ShutdownBudgets) {
    shutdown.cancelled().await;
    let requested = Instant::now();

    let mut claims = budgets.claims.subscribe();
    loop {
        let Some(budget) = claims.borrow_and_update().longest() else {
            return;
        };

        tokio::select! {
            () = tokio::time::sleep_until(requested + budget) => return,
            Ok(()) = claims.changed() => {}
        }
    }
}

/// Spawn a task on `runtime` that runs `then` when `shutdown` is cancelled.
///
/// Running on a runtime worker ensures `then` executes even if the main command
/// thread is blocked in a synchronous prompt.
pub(crate) fn on_shutdown(
    runtime: &Handle,
    shutdown: CancellationToken,
    then: impl FnOnce() + Send + 'static,
) {
    drop(runtime.spawn(async move {
        shutdown.cancelled().await;
        then();
    }));
}

#[cfg(test)]
#[path = "shutdown_tests.rs"]
mod tests;
