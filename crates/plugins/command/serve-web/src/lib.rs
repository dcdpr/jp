//! `jp-serve-web`: web UI plugin for JP.
//!
//! Communicates with the `jp` host over the JSON-lines plugin protocol
//! (stdin/stdout) and serves a conversation browser over HTTP.
//! Turns composed in the browser are delegated to the host, which owns the
//! agent loop.
//!
//! See: `docs/rfd/072-command-plugin-system.md`

mod client;
mod render;
mod routes;
mod style;
mod views;

use std::{
    io::{BufRead, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
};

use jp_plugin::{
    client::Writer,
    log::LogHandle,
    message::{HostToPlugin, InitMessage, PluginToHost},
    origin,
    server::{self, Listen},
    stdio::{self, print_to, send, send_exit},
};
use tracing::info;

const BINARY: &str = "jp-serve-web";

const DEFAULT_PORT: u16 = 3000;

const SERVING: &str = "conversations";

/// Warning text shown when bound to a non-loopback address.
const REACH: &str = "read every conversation in this workspace, and start turns that spend tokens \
                     and run whatever tools the conversation allows";

/// The protocol version this plugin needs from the host.
///
/// It archives and renames conversations (3), syncs what is being typed through
/// the draft messages (4), offers the configurations a turn can name (5), posts
/// turns with `query` and learns their id from `created` (6), stops them with
/// `interrupt` (7), reads whether a turn is already running from `lock` on
/// `events` (8), and answers a running turn with `interrupt`'s `reply` action
/// (9).
///
/// The last is what makes 9 the floor rather than 8: an older host reads the
/// action as the stop it defaults to, so sending a message to a busy
/// conversation would end its turn and discard what was typed.
const REQUIRED_PROTOCOL: u32 = 9;

/// Where this plugin attaches to `jp`, readable from the binary without running
/// it.
///
/// `protocol` has to match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 9,
    description: "Web UI for browsing conversations and continuing them",
    command: ["serve", "web"],
);

/// What a human running the binary directly is shown instead of a hung read on
/// a protocol that is never going to speak.
pub const HELP_TEXT: &str = "\
Start the web interface for browsing JP conversations and continuing them.

Usage: jp serve web [OPTIONS]

Options:
  --bind <ADDR>           Address to listen on: an IP address, or `localhost`
                          [default: 127.0.0.1]
  --port <PORT>           Port to listen on [default: 3000]
  --allowed-host <NAME>   A name to answer to on loopback, besides addresses
                          and `localhost`, such as the one a proxy on this
                          machine forwards; repeat for more than one

Configuration (in .jp/config.toml):
  [plugins.command.serve-web.options]
  bind = \"127.0.0.1\"
  port = 8080
  allowed_hosts = [\"my-machine.local\"]

The server has no authentication and exposes every conversation in the
workspace, and anyone who reaches it can start a turn, which spends tokens and
runs whatever tools the conversation allows. Binding to a non-loopback address
(e.g. 0.0.0.0) hands that to the network.";

/// Serve one plugin session: read the host's first message and answer it.
///
/// A `describe` is answered and ends the session; an `init` starts the web
/// server and blocks until the host asks it to stop.
pub fn run(
    mut stdin: impl BufRead + Send + 'static,
    mut stdout: impl Write + Send + 'static,
    log_handle: &LogHandle,
) -> Result<(), String> {
    match stdio::read(&mut stdin, BINARY)? {
        HostToPlugin::Describe => stdio::describe(
            &mut stdout,
            MANIFEST,
            "serve-web",
            env!("CARGO_PKG_VERSION"),
            HELP_TEXT,
        ),
        HostToPlugin::Init(ref init) => run_server(init, stdin, stdout, log_handle),
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

fn run_server(
    init: &InitMessage,
    stdin: impl BufRead + Send + 'static,
    mut stdout: impl Write + Send + 'static,
    log_handle: &LogHandle,
) -> Result<(), String> {
    match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
        Ok(ready) => send(&mut stdout, &PluginToHost::Ready(ready))?,
        Err(exit) => return send(&mut stdout, &PluginToHost::Exit(exit)),
    }

    // Bind the socket before announcing the URL so a port conflict fails
    // before printing "Serving".
    let (listener, allowed_hosts) =
        match parse_args(&init.args).and_then(|request| listen(init, request)) {
            Ok(Some(listening)) => listening,
            Ok(None) => {
                print_to(&mut stdout, &format!("{HELP_TEXT}\n"), "content")?;
                return send_exit(&mut stdout, 0, None);
            }
            Err(reason) => return send_exit(&mut stdout, 1, Some(&reason)),
        };
    let address = listener
        .local_addr()
        .map_err(|error| format!("Failed to read the address listened on: {error}"))?;
    server::announce(&mut stdout, SERVING, address, REACH)?;

    // Shared by the protocol client and the log layer, each writing whole
    // lines.
    let writer: Writer = Arc::new(Mutex::new(Box::new(stdout)));
    log_handle.activate(&writer);

    let (client, shutdown_rx) = client::PluginClient::start(stdin, writer)
        .map_err(|e| format!("failed to start the protocol client: {e}"))?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("jp-serve-web")
        .build()
        .map_err(|e| format!("failed to build tokio runtime: {e}"))?;

    let exit_client = client.clone();

    let result = rt.block_on(async {
        info!(%address, "Starting web server");

        let mut shutdown = shutdown_rx;
        let shutdown_signal = async move {
            let _ = shutdown.changed().await;
        };

        routes::serve(client, listener, allowed_hosts, shutdown_signal)
            .await
            .map_err(|e| format!("server error: {e}"))
    });

    let code = u8::from(result.is_err());
    exit_client.send_exit(code);

    result
}

/// Bind the TCP listener configured by `request` and `init.options`, returning
/// the listener and allowed hostnames, or `None` if `--help` was requested.
fn listen(
    init: &InitMessage,
    request: Request,
) -> Result<Option<(TcpListener, Vec<String>)>, String> {
    let Request::Serve(flags) = request else {
        return Ok(None);
    };
    let options = Listen::from_options(&init.options, "serve-web")?;
    let listener = server::listen(options.address(&flags, DEFAULT_PORT))?;

    Ok(Some((listener, options.allowed_hosts(&flags).to_vec())))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    Help,
    Serve(Listen),
}

/// Parse `jp serve web` CLI arguments (`--bind`, `--port`, `--allowed-host`, or
/// `-h`/`--help`).
fn parse_args(args: &[String]) -> Result<Request, String> {
    let mut flags = Listen::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (arg.as_str(), None),
        };
        if matches!(name, "-h" | "--help") {
            return Ok(Request::Help);
        }
        if !matches!(name, "--bind" | "--port" | "--allowed-host") {
            return Err(format!(
                "`jp serve web` takes `--bind`, `--port`, and `--allowed-host`, not `{arg}`."
            ));
        }

        let value = inline
            .or_else(|| args.next().cloned())
            .ok_or_else(|| format!("`{name}` needs a value."))?;
        set_flag(&mut flags, name, &value)?;
    }

    Ok(Request::Serve(flags))
}

/// Apply a single CLI flag (`--bind`, `--port`, or `--allowed-host`) to
/// `flags`.
fn set_flag(flags: &mut Listen, name: &str, value: &str) -> Result<(), String> {
    if name == "--bind" {
        flags.bind = Some(server::parse_bind(value).map_err(|why| format!("`--bind`: {why}."))?);
        return Ok(());
    }
    if name == "--allowed-host" {
        let host = origin::parse_host(value).map_err(|why| format!("`--allowed-host`: {why}."))?;
        flags.allowed_hosts.get_or_insert_default().push(host);
        return Ok(());
    }

    flags.port = Some(
        value
            .trim()
            .parse()
            .map_err(|_| format!("`--port` must be a port, from 0 to 65535, not `{value}`."))?,
    );
    Ok(())
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
