//! Cancellable periodic timer utilities.
//!
//! Generic async helpers for spawning interval-based tasks.
//! Used by the tool renderer (argument-receiving indicator), tool coordinator
//! (execution progress), and turn loop (streaming progress).

use std::{fmt::Write as _, sync::Arc, time::Duration};

use jp_printer::Printer;
use tokio::{
    sync::{mpsc::Sender, watch},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

/// Spawns a timer task that sends elapsed [`Duration`] through a channel at a
/// fixed interval.
///
/// After `delay`, the task sends its elapsed time every `interval`.
/// On cancellation (or when the receiver is dropped), the task exits.
///
/// Returns `None` if `show` is `false`, in which case nothing is spawned.
pub fn spawn_tick_sender(
    tx: Sender<Duration>,
    show: bool,
    delay: Duration,
    interval: Duration,
) -> Option<CancellationToken> {
    if !show {
        return None;
    }

    let token = CancellationToken::new();
    let child = token.child_token();
    let interval = interval.max(Duration::from_millis(10));

    tokio::spawn(async move {
        let start = Instant::now();

        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = child.cancelled() => { return; }
        }

        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;
                () = child.cancelled() => { return; }
                _ = ticker.tick() => {
                    if tx.send(start.elapsed()).await.is_err() {
                        return;
                    }
                }
            }
        }
    });

    Some(token)
}

/// Handle to a running line timer.
///
/// Carries a status channel: [`Self::set_status`] replaces the detail string
/// passed to the timer's format closure, and the line redraws immediately
/// rather than waiting for the next tick.
///
/// Dropping the handle cancels the timer; the task clears its line
/// asynchronously.
/// Call [`Self::finish`] instead when the caller is about to write persistent
/// output: it waits for the line-clear to complete, so the terminal row is
/// guaranteed clean when it returns.
pub struct LineTimer {
    token: CancellationToken,
    handle: Option<JoinHandle<()>>,
    status: watch::Sender<Option<String>>,
}

impl LineTimer {
    /// Replace the status detail passed to the timer's format closure.
    ///
    /// Triggers an immediate redraw (once the timer's initial delay has
    /// passed).
    pub fn set_status(&self, status: impl Into<String>) {
        self.status.send_replace(Some(status.into()));
    }

    /// Cancel the timer and wait for its line-clear to complete.
    ///
    /// After this returns, the terminal row is clean and the caller may safely
    /// render persistent content.
    pub async fn finish(mut self) {
        self.token.cancel();
        if let Some(handle) = self.handle.take() {
            drop(handle.await);
        }
    }
}

impl Drop for LineTimer {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

/// Spawns a `\r`-based timer task that periodically writes a status line.
///
/// After `delay`, the task calls `format_line(elapsed_secs, status)` every
/// `interval` and writes the result to the printer.
/// The status detail starts as `None` and is replaced via
/// [`LineTimer::set_status`]; a status change redraws the line immediately.
/// On cancellation the task clears the line with `\r\x1b[K`.
///
/// Returns `None` if `show` is `false`, in which case nothing is spawned.
pub fn spawn_line_timer(
    printer: Arc<Printer>,
    show: bool,
    delay: Duration,
    interval: Duration,
    format_line: impl Fn(f64, Option<&str>) -> String + Send + 'static,
) -> Option<LineTimer> {
    if !show {
        return None;
    }

    let token = CancellationToken::new();
    let child = token.child_token();
    let interval = interval.max(Duration::from_millis(10));
    let (status_tx, mut status_rx) = watch::channel(None::<String>);

    let handle = tokio::spawn(async move {
        let start = Instant::now();

        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = child.cancelled() => { return; }
        }

        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // Disabled once the sender side is gone, so a dropped handle doesn't
        // turn the `changed()` arm into a busy loop while the cancellation
        // (raised by `LineTimer::drop` before the sender drops) propagates.
        let mut status_open = true;

        loop {
            tokio::select! {
                biased;
                () = child.cancelled() => {
                    let _ = write!(printer.err_writer(), "\r\x1b[K");
                    return;
                }
                _ = ticker.tick() => {}
                changed = status_rx.changed(), if status_open => {
                    if changed.is_err() {
                        status_open = false;
                        continue;
                    }
                }
            }

            let secs = start.elapsed().as_secs_f64();
            let status = status_rx.borrow_and_update();
            let line = format_line(secs, status.as_deref());
            drop(status);
            let _ = write!(printer.err_writer(), "{line}");
        }
    });

    Some(LineTimer {
        token,
        handle: Some(handle),
        status: status_tx,
    })
}

#[cfg(test)]
#[path = "timer_tests.rs"]
mod tests;
