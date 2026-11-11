//! `jp-serve-chat`: a command plugin (`jp serve chat`) that relays messages
//! between a chat app and JP turns.
//!
//! The plugin does not call models or execute tools itself; the `jp` host runs
//! each turn under the configured persona and configuration, just like `jp
//! query`.
//!
//! - [`transport`] defines the chat transport interface; [`signal`] implements
//!   it over `signal-cli` in JSON-RPC mode.
//! - [`bridge`] routes messages from allowed senders to daily conversations and
//!   schedules turns.
//! - [`checkout`] locks the git checkout during turns, pulls when clean, and
//!   pauses the bridge if a turn modifies files outside the writable
//!   directories.
//! - [`host`] implements the `jp` plugin protocol over stdin and stdout.
//! - [`options`] parses `plugins.command.serve-chat.options`.

mod answer;
mod bridge;
mod checkout;
mod host;
mod options;
mod signal;
mod state;
mod text;
mod transport;

use std::{
    io::{BufRead, Write},
    sync::Arc,
};

use camino::Utf8PathBuf;
use jp_plugin::message::{DescribeResponse, ExitMessage, HostToPlugin, InitMessage, PluginToHost};
use tokio::sync::{mpsc, watch};

use crate::{
    bridge::Bridge,
    checkout::{Checkout, GitCheckout, Unguarded},
    host::{Host as _, HostClient, Level},
    options::Options,
    signal::{Logger, Signal},
    state::State,
    transport::Outbox,
};

/// Minimum host protocol version required by this plugin.
///
/// Protocol 11 added `expires_in` on `query` requests; protocol 9 added in-turn
/// `interrupt` replies.
const REQUIRED_PROTOCOL: u32 = 11;

/// Embedded plugin manifest read by `jp` for binary discovery.
///
/// `protocol` must match [`REQUIRED_PROTOCOL`].
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 11,
    description: "Relay chat messages to JP turns, and the answers back",
    command: ["serve", "chat"],
);

/// CLI `--help` output, also printed when the binary is invoked directly.
pub const HELP_TEXT: &str = "\
Relay messages from a chat app to JP turns, and the answers back.

Usage: jp serve chat

Runs until stopped. Every message from a configured person becomes a turn,
run by `jp` under the configured persona, and the turn's answer is sent back.

Configuration (`plugins.command.serve-chat.options`):

  persona            A configuration every turn runs under, as --cfg takes it.
  cfg                More configurations, applied after the persona.
  admin              The person told when the bridge pauses.
  rollover           `daily`, or the weekday a new conversation starts on.
  conversation_ttl   How long a conversation is kept. Default: 7d (14d weekly).
  signal.account     The Signal account to run as. Required.
  signal.account_id  The account's id, to recognize mentions of it.
  signal.command     The signal-cli executable. Default: signal-cli.
  people.<key>       Who is answered: `name`, and their `signal` account.
  checkout.enabled   Lock, pull, and check the checkout. Default: true.
  checkout.writable  Directories a turn may change.
                     Default: docs/ticket, docs/rfd, docs/wiki.
  checkout.pull_interval  How often to pull between turns. Default: 5m.

Nothing a turn writes is committed. Take it for review from the checkout, under
the lock at `git rev-parse --git-path jp-serve-chat.lock`; the bridge pulls
again once the tree is clean.";

/// Run one plugin session over `stdin` and `stdout`.
///
/// Answers `describe` and exits, or handles `init` by running the bridge until
/// the host requests shutdown or the transport exits.
pub fn run(
    mut stdin: impl BufRead + Send + 'static,
    mut stdout: impl Write + Send + 'static,
) -> Result<(), String> {
    match read_message(&mut stdin)? {
        HostToPlugin::Describe => send_describe(&mut stdout),
        HostToPlugin::Init(init) => {
            match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
                Ok(ready) => send(&mut stdout, &PluginToHost::Ready(ready))?,
                Err(exit) => return send(&mut stdout, &PluginToHost::Exit(exit)),
            }

            if init.tool.is_some() {
                return send_exit(&mut stdout, 1, "`jp serve chat` serves no tools.");
            }
            match init.args.first().map(String::as_str) {
                None => {}
                Some("-h" | "--help") => {
                    send(
                        &mut stdout,
                        &PluginToHost::Print(jp_plugin::message::PrintMessage {
                            text: format!("{HELP_TEXT}\n"),
                            channel: "content".to_owned(),
                            format: "plain".to_owned(),
                            language: None,
                        }),
                    )?;
                    return send(
                        &mut stdout,
                        &PluginToHost::Exit(ExitMessage {
                            code: 0,
                            reason: None,
                            expected: false,
                        }),
                    );
                }
                Some(other) => {
                    return send_exit(
                        &mut stdout,
                        2,
                        &format!("`jp serve chat` takes no arguments, got `{other}`."),
                    );
                }
            }

            let (client, shutdown) = HostClient::start(stdin, stdout);
            let result = serve(&init, &client, shutdown);
            match &result {
                Ok(()) => client.exit(0, None),
                Err(reason) => client.exit(1, Some(reason.clone())),
            }

            Ok(())
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

fn serve(
    init: &InitMessage,
    client: &HostClient,
    shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let options = Options::read(&init.options)?;
    let state = State::open(&state_dir(init)?)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("jp-serve-chat")
        .build()
        .map_err(|error| format!("failed to start the async runtime: {error}"))?;

    runtime.block_on(async {
        let (inbound_tx, inbound) = mpsc::channel(64);
        let log_client = client.clone();
        let log: Logger = Arc::new(move |level, message| log_client.log(level, &message));
        let (signal, _process) = Signal::spawn(&options.signal, log, inbound_tx)?;
        let outbox: Arc<dyn Outbox> = signal;

        client.print(&format!(
            "Relaying Signal messages to {} for {} {}.",
            options.signal.account,
            options.people.len(),
            if options.people.len() == 1 {
                "person"
            } else {
                "people"
            }
        ));
        client.log(
            Level::Info,
            &format!("Bridge state is kept in {}", state_dir(init)?),
        );

        if options.checkout.enabled {
            let checkout =
                GitCheckout::open(&init.workspace.root, options.checkout.writable.clone())?;
            relay(client, outbox, checkout, options, state, inbound, shutdown).await
        } else {
            relay(client, outbox, Unguarded, options, state, inbound, shutdown).await
        }
    })
}

async fn relay<C: Checkout>(
    client: &HostClient,
    outbox: Arc<dyn Outbox>,
    checkout: C,
    options: Options,
    state: State,
    inbound: mpsc::Receiver<transport::Inbound>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    Bridge::new(client.clone(), outbox, checkout, options, state)
        .run(inbound, shutdown)
        .await
}

/// Directory where the bridge persists its chat-to-conversation mapping.
///
/// Stored in JP's user data directory outside the workspace checkout so state
/// updates never dirty the git working tree.
fn state_dir(init: &InitMessage) -> Result<Utf8PathBuf, String> {
    if let Some(dir) = &init.paths.user_workspace {
        return Ok(dir.join("serve-chat"));
    }
    if let Some(dir) = &init.paths.user_data {
        return Ok(dir.join("serve-chat").join(&init.workspace.id));
    }

    Err("JP reported no user data directory to keep the bridge's state in.".to_owned())
}

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    let mut describe = DescribeResponse::from_manifest(
        MANIFEST,
        "serve-chat",
        env!("CARGO_PKG_VERSION"),
        HELP_TEXT,
    )
    .map_err(|error| format!("invalid embedded manifest: {error}"))?;

    describe.author = Some("Jean Mertz <git@jeanmertz.com>".to_owned());
    describe.repository = Some("https://github.com/dcdpr/jp".to_owned());

    send(stdout, &PluginToHost::Describe(describe))
}

fn send_exit(stdout: &mut impl Write, code: u8, reason: &str) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Exit(ExitMessage {
            code,
            reason: Some(reason.to_owned()),
            expected: false,
        }),
    )
}

fn read_message(stdin: &mut impl BufRead) -> Result<HostToPlugin, String> {
    let mut line = String::new();
    let read = stdin
        .read_line(&mut line)
        .map_err(|error| format!("failed to read from the host: {error}"))?;
    if read == 0 {
        return Err("The host stopped responding before sending init.".to_owned());
    }

    serde_json::from_str(line.trim()).map_err(|error| format!("invalid host message: {error}"))
}

fn send(stdout: &mut impl Write, message: &PluginToHost) -> Result<(), String> {
    let json =
        serde_json::to_string(message).map_err(|error| format!("serialize error: {error}"))?;
    writeln!(stdout, "{json}").map_err(|error| format!("write error: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush error: {error}"))
}
