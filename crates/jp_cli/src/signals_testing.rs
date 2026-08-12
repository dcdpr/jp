//! Test-only construction of [`SignalRouter`]s: an in-memory signal channel and
//! a recording exit action replace OS signals and [`std::process::exit`].
//!
//! The routing logic under test is the real thing — [`SignalRouter`] with its
//! actual signal task — only the process boundaries (signal source, exit) are
//! inverted through [`SignalRouter::with_signal_source`].

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{runtime::Handle, sync::mpsc};
use tokio_stream::wrappers::ReceiverStream;

use super::{OsSignal, SignalRouter};

/// The injection half of a [`test_router`]: sends signals and observes exit
/// decisions.
pub(crate) struct TestSignals {
    /// Feeds the router's signal task.
    tx: mpsc::Sender<OsSignal>,

    /// Exit codes recorded by the router's exit action.
    exit_codes: Arc<Mutex<Vec<i32>>>,
}

impl TestSignals {
    /// Deliver a Ctrl-C press through the router's signal task, exactly as an
    /// OS SIGINT would arrive.
    ///
    /// Delivery is asynchronous: the send queues the signal, and the signal
    /// task routes it when scheduled.
    /// Tests observe the effect through the routed outcome (a notified handler,
    /// the shutdown token, or a recorded exit code), never through the send
    /// itself.
    pub(crate) async fn interrupt(&self) {
        self.tx
            .send(OsSignal::Interrupt)
            .await
            .expect("signal task should be alive");
    }

    /// The exit codes the router's exit action was invoked with, in order.
    ///
    /// In production these would each have been a [`std::process::exit`].
    pub(crate) fn exit_codes(&self) -> Vec<i32> {
        self.exit_codes
            .lock()
            .expect("exit codes lock poisoned")
            .clone()
    }
}

/// A [`SignalRouter`] driven by an in-memory channel instead of OS signals,
/// with the default 2-second escalation cooldown.
///
/// The exit action records codes instead of ending the process, so tests can
/// drive the full escalation ladder — including the exit rung — in-process.
///
/// Must be called from within a tokio runtime context.
pub(crate) fn test_router() -> (SignalRouter, TestSignals) {
    let (tx, rx) = mpsc::channel(16);
    let exit_codes = Arc::new(Mutex::new(Vec::new()));

    let recorder = Arc::clone(&exit_codes);
    let router = SignalRouter::with_signal_source(
        &Handle::current(),
        ReceiverStream::new(rx),
        Duration::from_secs(2),
        move |code| {
            recorder
                .lock()
                .expect("exit codes lock poisoned")
                .push(code);
        },
    );

    (router, TestSignals { tx, exit_codes })
}

/// A [`SignalRouter`] with no signal source, for tests that never deliver
/// signals.
///
/// The sending half is dropped, so the signal task ends immediately; handler
/// registration, declining, and the shutdown token all work as usual.
///
/// Must be called from within a tokio runtime context.
pub(crate) fn detached_router() -> SignalRouter {
    test_router().0
}
