//! Cancellable periodic timer utilities.
//!
//! [`spawn_tick_sender`] drives the tool renderer's argument-receiving
//! indicator and the tool coordinator's execution progress, both of which
//! consume elapsed time as a stream of events rather than drawing it.
//! Everything that draws its own row uses the printer's status regions instead.

use std::time::Duration;

use tokio::{
    sync::mpsc::Sender,
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
