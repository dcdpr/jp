//! `jp-serve-chat`: a command plugin (`jp serve chat`) that relays messages
//! between a chat app and JP turns.
//!
//! The plugin does not call models or execute tools itself; the `jp` host runs
//! each turn under the configured persona and configuration, just like `jp
//! query`.
//!
//! - `transport` defines the chat transport interface; `signal` implements it
//!   over `signal-cli` in JSON-RPC mode.
//! - `bridge` routes messages from allowed senders to conversations and
//!   schedules turns, with `inbox` buffering messages awaiting a `decide`
//!   verdict and `queue` batching messages awaiting a turn.
//! - `checkout` locks the git checkout during turns, pulls when clean, and
//!   pauses the bridge if a turn modifies files outside the writable
//!   directories.
//! - `host` implements the `jp` plugin protocol over stdin and stdout.
//! - `options` parses `plugins.command.serve-chat.options`.

mod answer;
mod bridge;
mod checkout;
mod decide;
mod host;
mod inbox;
mod options;
mod queue;
mod signal;
mod state;
mod text;
mod transport;

use std::{
    env,
    io::{BufRead, Write},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use camino::Utf8PathBuf;
use jp_plugin::message::{DescribeResponse, HostToPlugin, InitMessage, PluginToHost};
use tokio::sync::{mpsc, watch};

use crate::{
    bridge::Bridge,
    checkout::{GitCheckout, Unguarded},
    decide::{Decide, SystemOne},
    host::{Host as _, HostClient, Level},
    options::Options,
    signal::{Logger, Signal},
    state::State,
    transport::Outbox,
};

/// Minimum host protocol version required by this plugin.
///
/// Protocol 14 added `compact`; 11 added `expires_in` on `query`, and 9 in-turn
/// `interrupt` replies.
const REQUIRED_PROTOCOL: u32 = 14;

/// Embedded plugin manifest read by `jp` for binary discovery.
///
/// `protocol` must match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 14,
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
  signal.attachments_dir  Where signal-cli saves attachments.
  decide             Ask a model what to do with each message: answer it,
                     ignore it, or, when it starts by mentioning the bot,
                     start a new conversation or compact the current one.
                     `url`, `model`,
                     `api_key_env` (default JEV_API_KEY; empty for no token),
                     `threshold` (default 0.5), `instructions`.
  compact            The compactions \"@bot compact\" asks for: `from`, `to`,
                     and `reasoning`, `tool_calls`, or `summary`. Default:
                     reasoning and tool calls stripped from turns 2 to -2.
  people.<key>       Who is answered: `name`, and their `signal` account.
  checkout.enabled   Lock, pull, and check the checkout. Default: true.
  checkout.writable  Directories a turn may change.
                     Default: docs/ticket, docs/rfd, docs/wiki.
  checkout.pull_interval  How often to pull between turns. Default: 5m.

Nothing a turn writes is committed. Take it for review from the checkout, under
the lock at `git rev-parse --git-path jp-serve-chat.lock`; the bridge pulls
again once the tree is clean.";

/// How many received chat messages can wait for the bridge before the
/// transport's reader blocks.
const INBOUND_CAPACITY: usize = 64;

/// Run one plugin session over `stdin` and `stdout`.
///
/// Answers `describe` and exits, or handles `init` by running the bridge until
/// the host requests shutdown or the transport exits.
pub fn run(
    mut stdin: impl BufRead + Send + 'static,
    stdout: impl Write + Send + 'static,
) -> Result<(), String> {
    let client = HostClient::new(stdout);

    let init = match first_message(&mut stdin)? {
        HostToPlugin::Describe => return client.send(&PluginToHost::Describe(describe()?)),
        HostToPlugin::Init(init) => init,
        other => return Err(format!("expected init or describe, got: {other:?}")),
    };

    match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
        Ok(ready) => client.send(&PluginToHost::Ready(ready))?,
        Err(exit) => return client.send(&PluginToHost::Exit(exit)),
    }
    if init.tool.is_some() {
        return client.exit(1, Some("`jp serve chat` serves no tools.".to_owned()));
    }
    match init.args.first().map(String::as_str) {
        None => {}
        Some("-h" | "--help") => {
            client.print(HELP_TEXT);
            return client.exit(0, None);
        }
        Some(other) => {
            return client.exit(
                2,
                Some(format!(
                    "`jp serve chat` takes no arguments, got `{other}`."
                )),
            );
        }
    }

    let shutdown = client.listen(stdin)?;
    match serve(&init, &client, shutdown) {
        Ok(()) => client.exit(0, None),
        Err(reason) => client.exit(1, Some(reason)),
    }
}

/// Read the host's first message: `describe`, or `init` to start a run.
fn first_message(stdin: &mut impl BufRead) -> Result<HostToPlugin, String> {
    let line = stdin
        .lines()
        .next()
        .ok_or("The host stopped responding before sending init.")?
        .map_err(|error| format!("failed to read from the host: {error}"))?;

    serde_json::from_str(line.trim()).map_err(|error| format!("invalid host message: {error}"))
}

fn serve(
    init: &InitMessage,
    client: &HostClient,
    shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let options = Options::read(&init.options)?;
    let decider = decider(&options)?;
    let state = State::open(&state_dir(init)?)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("jp-serve-chat")
        .build()
        .map_err(|error| format!("failed to start the async runtime: {error}"))?;

    runtime.block_on(async {
        let (inbound_tx, inbound) = mpsc::channel(INBOUND_CAPACITY);
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
            Level::INFO,
            &format!("Bridge state is kept in {}", state_dir(init)?),
        );

        if !options.checkout.enabled {
            let bridge = Bridge::new(client.clone(), outbox, Unguarded, options, state);
            return bridge.with_decider(decider).run(inbound, shutdown).await;
        }

        let checkout = GitCheckout::open(&init.workspace.root, options.checkout.writable.clone())?;
        let bridge = Bridge::new(client.clone(), outbox, checkout, options, state);
        bridge.with_decider(decider).run(inbound, shutdown).await
    })
}

/// Build the [`Decide`] client from `options.decide`, reading its bearer token
/// from the environment variable named by `decide.api_key_env` (if set).
fn decider(options: &Options) -> Result<Option<Arc<dyn Decide>>, String> {
    let Some(decide) = &options.decide else {
        return Ok(None);
    };
    let token = match &decide.api_key_env {
        None => None,
        Some(name) => Some(
            env::var(name)
                .ok()
                .filter(|token| !token.trim().is_empty())
                .ok_or_else(|| {
                    format!(
                        "`decide` is configured, but the environment variable `{name}` holds no \
                         API token. Set `decide.api_key_env = \"\"` for a server that needs none."
                    )
                })?,
        ),
    };

    Ok(Some(Arc::new(SystemOne::new(decide.clone(), token)?)))
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

fn describe() -> Result<DescribeResponse, String> {
    let mut describe = DescribeResponse::from_manifest(
        MANIFEST,
        "serve-chat",
        env!("CARGO_PKG_VERSION"),
        HELP_TEXT,
    )
    .map_err(|error| format!("invalid embedded manifest: {error}"))?;

    describe.author = Some("Jean Mertz <git@jeanmertz.com>".to_owned());
    describe.repository = Some("https://github.com/dcdpr/jp".to_owned());

    Ok(describe)
}

/// Acquire `mutex`, recovering the guard if a previous holder panicked.
pub(crate) fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
