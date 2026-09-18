//! `jp-serve-web`: web UI plugin for JP.
//!
//! Communicates with the `jp` host over the JSON-lines plugin protocol
//! (stdin/stdout) and serves a conversation browser over HTTP.
//! Turns composed in the browser are delegated to the host, which owns the
//! agent loop.
//!
//! See: `docs/rfd/072-command-plugin-system.md`

mod client;
mod log_layer;
mod render;
mod routes;
mod style;
mod views;

use std::{
    io::{self, BufRead, BufReader, IsTerminal as _, Write},
    net::{IpAddr, SocketAddr, TcpListener},
    sync::{Arc, Mutex},
};

use jp_plugin::message::{DescribeResponse, HostToPlugin, InitMessage, PluginToHost, PrintMessage};
use tracing::{Level, info, warn};

use crate::{
    client::SharedWriter,
    log_layer::{ProtocolLogHandle, ProtocolLogLayer},
};

/// The protocol version this plugin needs from the host.
///
/// It archives and renames conversations (3), syncs what is being typed through
/// the draft messages (4), offers the configurations a turn can name (5), posts
/// turns with `query` and learns their id from `created` (6), stops them with
/// `interrupt` (7), and reads whether a turn is already running from `lock` on
/// `events` (8).
///
/// The last is what makes 8 the floor rather than 7: defaulting `lock` to free
/// would draw a send button for a conversation that is busy, and the request
/// behind it would be refused as already-locked.
const REQUIRED_PROTOCOL: u32 = 8;

const HELP_TEXT: &str = "\
Start the web interface for browsing JP conversations and continuing them.

Usage: jp serve-web [OPTIONS]

Options:
  --bind <ADDR>    Address to bind to [default: 127.0.0.1]
  --port <PORT>    Port to listen on [default: 3000]

Configuration (in .jp/config.toml):
  [plugins.command.serve-web.options]
  bind = \"127.0.0.1\"
  port = 8080

The server has no authentication and exposes every conversation in the
workspace, and anyone who reaches it can start a turn, which spends tokens and
runs whatever tools the conversation allows. Binding to a non-loopback address
(e.g. 0.0.0.0) hands that to the network.";

fn main() {
    let log_handle = init_tracing();

    // If stdin is a TTY, the binary was invoked directly (not via the plugin
    // protocol). Print help and exit.
    if io::stdin().is_terminal() {
        let mut err = io::stderr().lock();
        drop(writeln!(err, "{HELP_TEXT}"));
        drop(writeln!(err));
        drop(writeln!(
            err,
            "Note: this binary is a JP plugin. Run it via `jp serve-web`."
        ));
        std::process::exit(0);
    }

    let stdin = BufReader::new(io::stdin());
    let stdout = io::stdout();

    let code = match run(stdin, stdout, &log_handle) {
        Ok(()) => 0,
        Err(e) => {
            let mut err = io::stderr().lock();
            drop(writeln!(err, "Fatal: {e}"));
            1
        }
    };

    std::process::exit(code);
}

fn run(
    mut stdin: impl BufRead + Send + 'static,
    mut stdout: impl Write + Send + 'static,
    log_handle: &ProtocolLogHandle,
) -> Result<(), String> {
    let first_msg = read_message(&mut stdin)?;

    match first_msg {
        HostToPlugin::Describe => {
            send_describe(&mut stdout)?;
            Ok(())
        }
        HostToPlugin::Init(ref init) => run_server(init, stdin, stdout, log_handle),
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

fn run_server(
    init: &InitMessage,
    stdin: impl BufRead + Send + 'static,
    mut stdout: impl Write + Send + 'static,
    log_handle: &ProtocolLogHandle,
) -> Result<(), String> {
    let args = parse_args(init);

    let bind = args
        .bind
        .or_else(|| {
            init.options
                .get("bind")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| "127.0.0.1".into());
    let port = args
        .port
        .or_else(|| {
            init.options
                .get("port")
                .and_then(serde_json::Value::as_u64)
                .and_then(|v| u16::try_from(v).ok())
        })
        .unwrap_or(3000);

    let ip: IpAddr = bind
        .parse()
        .map_err(|e| format!("invalid bind address `{bind}`: {e}"))?;
    let socket_addr = SocketAddr::new(ip, port);

    // Bind before announcing, so a bind failure is reported instead of a false
    // "Serving at" message. The listener is handed to axum below.
    let listener =
        TcpListener::bind(socket_addr).map_err(|e| format!("failed to bind {socket_addr}: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("failed to configure listener: {e}"))?;

    let is_loopback = ip.is_loopback();
    if !is_loopback {
        warn!(
            %socket_addr,
            "Binding to a non-loopback address exposes all conversations, and lets anyone who \
             reaches it start a turn, without authentication"
        );
    }

    // Send early protocol messages before sharing stdout.
    match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
        Ok(ready) => send(&mut stdout, &PluginToHost::Ready(ready))?,
        Err(exit) => return send(&mut stdout, &PluginToHost::Exit(exit)),
    }
    send(
        &mut stdout,
        &PluginToHost::Print(PrintMessage {
            text: format!("Serving at http://{socket_addr}\n"),
            channel: "content".into(),
            format: "plain".into(),
            language: None,
        }),
    )?;

    // The `warn!` above is invisible at the default log level, so surface the
    // exposure through a `Print` that always reaches the terminal.
    if !is_loopback {
        send(
            &mut stdout,
            &PluginToHost::Print(PrintMessage {
                text: "Warning: bound to a non-loopback address; every conversation in this \
                       workspace is readable over the network without authentication, and anyone \
                       who reaches it can start a turn.\n"
                    .into(),
                channel: "content".into(),
                format: "plain".into(),
                language: None,
            }),
        )?;
    }

    // Wrap stdout for shared access between the protocol client and log layer.
    let writer: SharedWriter = Arc::new(Mutex::new(Box::new(stdout)));

    // Activate the log layer now that we have the writer and know the level.
    let min_level = match init.log_level {
        0 => Level::ERROR,
        1 => Level::WARN,
        2 => Level::INFO,
        3 => Level::DEBUG,
        _ => Level::TRACE,
    };
    log_handle.activate(&writer, min_level);

    let (client, shutdown_rx) = client::PluginClient::start(stdin, writer);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("jp-serve-web")
        .build()
        .map_err(|e| format!("failed to build tokio runtime: {e}"))?;

    let exit_client = client.clone();

    let result = rt.block_on(async {
        info!(%socket_addr, "Starting web server");

        let mut shutdown = shutdown_rx;
        let shutdown_signal = async move {
            let _ = shutdown.changed().await;
        };

        routes::serve(client, listener, shutdown_signal)
            .await
            .map_err(|e| format!("server error: {e}"))
    });

    let code = u8::from(result.is_err());
    exit_client.send_exit(code);

    result
}

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Describe(DescribeResponse {
            name: "serve-web".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Web UI for browsing conversations and continuing them".to_owned(),
            command: vec!["serve".to_owned(), "web".to_owned()],
            author: Some("Jean Mertz <git@jeanmertz.com>".to_owned()),
            help: Some(HELP_TEXT.to_owned()),
            repository: Some("https://github.com/dcdpr/jp".to_owned()),
        }),
    )
}

fn read_message(stdin: &mut impl BufRead) -> Result<HostToPlugin, String> {
    let mut line = String::new();
    stdin
        .read_line(&mut line)
        .map_err(|e| format!("failed to read from host: {e}"))?;

    serde_json::from_str(line.trim()).map_err(|e| format!("invalid host message: {e}"))
}

fn send(stdout: &mut impl Write, msg: &PluginToHost) -> Result<(), String> {
    let json = serde_json::to_string(msg).map_err(|e| format!("serialize error: {e}"))?;
    writeln!(stdout, "{json}").map_err(|e| format!("write error: {e}"))?;
    stdout.flush().map_err(|e| format!("flush error: {e}"))
}

struct Args {
    bind: Option<String>,
    port: Option<u16>,
}

fn parse_args(init: &InitMessage) -> Args {
    let mut bind = None;
    let mut port = None;
    let mut iter = init.args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--bind" => bind = iter.next().map(String::from),
            "--port" => {
                port = iter.next().and_then(|v| v.parse().ok());
            }
            s if s.starts_with("--bind=") => bind = s.strip_prefix("--bind=").map(String::from),
            s if s.starts_with("--port=") => {
                port = s.strip_prefix("--port=").and_then(|v| v.parse().ok());
            }
            _ => {}
        }
    }

    Args { bind, port }
}

/// Install the tracing subscriber with the protocol log layer.
///
/// Events are buffered until the protocol writer is available.
/// Returns a handle that must be activated once the writer and log level are
/// known.
fn init_tracing() -> ProtocolLogHandle {
    use tracing_subscriber::prelude::*;

    let (layer, handle) = ProtocolLogLayer::new();

    tracing_subscriber::registry().with(layer).init();

    handle
}
