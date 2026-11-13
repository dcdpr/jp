//! Ending a command on a graceful shutdown request.
//!
//! A graceful shutdown is requested by an unhandled or escalated Ctrl-C, or by
//! SIGTERM, and cancels the signal router's shutdown token.
//!
//! [`until_shutdown`] is what every command runs under.
//! The request drops the command, unless part of it holds a claim on
//! [`ShutdownBudgets`]: work that has to be stopped in order claims the time
//! that takes, and the command is dropped once no claim is left or the longest
//! one has run out.

use std::{sync::Arc, time::Duration};

use tokio::{runtime::Handle, sync::watch, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::cmd;

/// The time the running command has claimed to stop its work in, once a
/// graceful shutdown is requested.
///
/// A command with nothing claimed is dropped at the request, which is right for
/// work a drop ends cleanly.
/// Work that has to be stopped in order (a child process asked to exit, a turn
/// left to record where it stopped) cannot be stopped from a drop.
/// The code that owns such work claims a budget with [`Self::claim`] for as
/// long as the work lasts, and watches the shutdown token itself.
///
/// Cloning gives another view of the same claims.
#[derive(Debug, Clone)]
pub(crate) struct ShutdownBudgets {
    /// A watch channel, so the wait in [`until_shutdown`] wakes when a budget
    /// is claimed or released.
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
    /// Claim `budget` to stop work in, until the returned guard is dropped.
    ///
    /// Several claims can be held at once, and the longest one is the budget.
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

    /// The longest budget claimed, or `None` while nothing is claimed.
    #[cfg(test)]
    pub(crate) fn longest(&self) -> Option<Duration> {
        self.claims.borrow().longest()
    }
}

/// Releases its claim on a shutdown budget on drop.
pub(crate) struct ShutdownBudgetGuard {
    claims: Arc<watch::Sender<Claims>>,
    id: ClaimId,
}

impl Drop for ShutdownBudgetGuard {
    fn drop(&mut self) {
        // By id rather than by position, so guards can drop in any order.
        self.claims
            .send_modify(|claims| claims.held.retain(|(id, _)| *id != self.id));
    }
}

/// Identity of a claim, unique among the claims of one [`ShutdownBudgets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClaimId(u64);

/// The budgets claimed and not yet released.
#[derive(Debug, Default)]
struct Claims {
    /// Source for unique claim ids.
    next_id: u64,

    held: Vec<(ClaimId, Duration)>,
}

impl Claims {
    fn longest(&self) -> Option<Duration> {
        self.held.iter().map(|(_, budget)| *budget).max()
    }
}

/// Run `command` until it returns, or until a graceful shutdown request ends
/// it.
///
/// A command holding no claim on `budgets` at the request is dropped at once,
/// which releases its conversation locks and persists its dirty conversations
/// (guard-scoped persistence).
///
/// A command holding one keeps running after the request, until it returns,
/// releases its last claim, or has run for the longest budget claimed since the
/// request, whichever comes first.
///
/// Returns [`cmd::Error::interrupted`] when `command` was dropped.
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

/// Wait for a graceful shutdown request, and then for the budget claimed on
/// `budgets` to be released or to run out.
async fn out_of_budget(shutdown: &CancellationToken, budgets: &ShutdownBudgets) {
    shutdown.cancelled().await;
    let requested = Instant::now();

    let mut claims = budgets.claims.subscribe();
    loop {
        // Marked as seen where it is read, so a claim made or released after
        // this read ends the wait below.
        let Some(budget) = claims.borrow_and_update().longest() else {
            return;
        };

        tokio::select! {
            () = tokio::time::sleep_until(requested + budget) => return,
            Ok(()) = claims.changed() => {}
        }
    }
}

/// Run `then` once a graceful shutdown is requested.
///
/// From a runtime worker, so it runs while the thread that polls the command is
/// blocked.
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
