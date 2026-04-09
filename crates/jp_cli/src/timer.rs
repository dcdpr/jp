//! Cancellable periodic timer utilities.
//!
//! Generic async helpers for spawning interval-based tasks. Used by the tool
//! renderer (argument-receiving indicator), tool coordinator (execution
//! progress), and turn loop (streaming progress).

use std::{fmt::Write as _, sync::Arc, time::Duration};

use jp_printer::Printer;
use tokio::{
    sync::mpsc::Sender,
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

/// Spawns a timer task that sends elapsed [`Duration`] through a channel
/// at a fixed interval.
///
/// After `delay`, the task sends its elapsed time every `interval`. On
/// cancellation (or when the receiver is dropped), the task exits.
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

/// Spawns a `\r`-based timer task that periodically writes a status line.
///
/// After `delay`, the task calls `format_line(elapsed_secs)` every
/// `interval` and writes the result to the printer. On cancellation it
/// clears the line with `\r\x1b[K`.
///
/// Returns `None` if `show` is `false`, in which case nothing is spawned.
pub fn spawn_line_timer(
    printer: Arc<Printer>,
    show: bool,
    delay: Duration,
    interval: Duration,
    format_line: impl Fn(f64) -> String + Send + 'static,
) -> Option<(CancellationToken, tokio::task::JoinHandle<()>)> {
    if !show {
        return None;
    }

    let token = CancellationToken::new();
    let child = token.child_token();
    let interval = interval.max(Duration::from_millis(10));

    let handle = tokio::spawn(async move {
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
                () = child.cancelled() => {
                    let _ = write!(printer.err_writer(), "\r\x1b[K");
                    return;
                }
                _ = ticker.tick() => {
                    let secs = start.elapsed().as_secs_f64();
                    let line = format_line(secs);
                    let _ = write!(printer.err_writer(), "{line}");
                }
            }
        }
    });

    Some((token, handle))
}
