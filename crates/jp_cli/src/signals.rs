use async_stream::stream;
use futures::{Stream, StreamExt as _};
use tokio::{runtime::Runtime, sync::broadcast};
use tracing::{error, info};

pub type ShutdownTx = broadcast::Sender<()>;
pub type SignalTx = broadcast::Sender<SignalTo>;
pub type SignalRx = broadcast::Receiver<SignalTo>;

/// Control messages used to drive application lifecycle events.
#[derive(Debug, Clone, PartialEq)]
pub enum SignalTo {
    /// Reload config from the filesystem.
    ReloadFromDisk,
    /// Shutdown process, with a grace period.
    Shutdown,
    /// Shutdown process immediately.
    Quit,
}

/// A container to hold both the signal handler and the receiver of OS signals.
pub struct SignalPair {
    #[expect(dead_code)]
    pub handler: SignalHandler,
    pub receiver: SignalRx,
}

impl SignalPair {
    /// Create a new signal handler pair, and set them up to receive OS signals.
    pub fn new(runtime: &Runtime) -> Self {
        let (handler, receiver) = SignalHandler::new();

        #[cfg(unix)]
        let signals = os_signals(runtime);

        // If we passed `runtime` here, we would get the following:
        // error[E0521]: borrowed data escapes outside of associated function
        #[cfg(windows)]
        let signals = os_signals();

        handler.forever(runtime, signals);
        Self { handler, receiver }
    }
}

/// `SignalHandler` is a general `ControlTo` message receiver and transmitter.
/// It's used by OS signals and commands to surface control events to the root
/// of the application.
pub struct SignalHandler {
    tx: SignalTx,
    shutdown_txs: Vec<ShutdownTx>,
}

impl SignalHandler {
    /// Create a new signal handler with space for 128 control messages at a
    /// time, to ensure the channel doesn't overflow and drop signals.
    pub fn new() -> (Self, SignalRx) {
        let (tx, rx) = broadcast::channel(128);
        let handler = Self {
            tx,
            shutdown_txs: vec![],
        };

        (handler, rx)
    }

    /// Clones the transmitter.
    pub fn clone_tx(&self) -> SignalTx {
        self.tx.clone()
    }

    /// Takes a stream who's elements are convertible to [`SignalTo`], and
    /// spawns a permanent task for transmitting to the receiver.
    fn forever<T, S>(&self, runtime: &Runtime, stream: S)
    where
        T: Into<SignalTo> + Send + Sync,
        S: Stream<Item = T> + 'static + Send,
    {
        let tx = self.clone_tx();

        runtime.spawn(async move {
            tokio::pin!(stream);

            while let Some(value) = stream.next().await {
                if let Err(error) = tx.send(value.into()) {
                    error!(%error, "Couldn't send OS signal.");
                }
            }
        });
    }

    /// Shutdown active signal handlers.
    #[expect(dead_code)]
    pub fn clear(&mut self) {
        for shutdown_tx in self.shutdown_txs.drain(..) {
            // An error just means the channel was already shut down; safe to
            // ignore.
            _ = shutdown_tx.send(());
        }
    }
}

/// Signals from OS/user.
#[cfg(unix)]
fn os_signals(runtime: &Runtime) -> impl Stream<Item = SignalTo> + use<> {
    use tokio::signal::unix::{signal, SignalKind};

    // The `signal` function must be run within the context of a Tokio runtime.
    runtime.block_on(async {
        let mut sigint = signal(SignalKind::interrupt()).expect("Failed to set up SIGINT handler.");
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler.");
        let mut sigquit = signal(SignalKind::quit()).expect("Failed to set up SIGQUIT handler.");
        let mut sighup = signal(SignalKind::hangup()).expect("Failed to set up SIGHUP handler.");

        stream!({
            loop {
                let signal = jp_macro::select!(
                    sigint.recv(),
                    |_signal| {
                        info!(message = "Signal received.", signal = "SIGINT");
                        SignalTo::Shutdown
                    },
                    sigterm.recv(),
                    |_signal| {
                        info!(message = "Signal received.", signal = "SIGTERM");
                        SignalTo::Shutdown
                    },
                    sigquit.recv(),
                    |_signal| {
                        info!(message = "Signal received.", signal = "SIGQUIT");
                        SignalTo::Quit
                    },
                    sighup.recv(),
                    |_signal| {
                        info!(message = "Signal received.", signal = "SIGHUP");
                        SignalTo::ReloadFromDisk
                    },
                );

                yield signal;
            }
        })
    })
}

/// Signals from OS/user.
#[cfg(windows)]
fn os_signals() -> impl Stream<Item = SignalTo> {
    use futures::future::FutureExt;

    stream! {
        loop {
            let signal = tokio::signal::ctrl_c().map(|_| SignalTo::Shutdown(None)).await;

            yield signal;
        }
    }
}
