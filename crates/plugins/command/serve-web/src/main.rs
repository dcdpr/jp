//! `jp-serve-web`: read-only web UI plugin for JP.
//!
//! Communicates with the `jp` host over the JSON-lines plugin protocol
//! (stdin/stdout) and serves a read-only conversation browser over HTTP.
//!
//! See: `docs/rfd/D17-command-plugin-system.md`

mod client;
mod log_layer;
mod render;
mod routes;
mod style;
mod views;

use std::{
    io::{self, BufRead, BufReader, IsTerminal as _, Write},
    sync::{Arc, Mutex},
};

use jp_plugin::message::{DescribeResponse, HostToPlugin, InitMessage, PluginToHost, PrintMessage};
use tracing::{Level, info};

use crate::{
    client::SharedWriter,
    log_layer::{ProtocolLogHandle, ProtocolLogLayer},
};

const HELP_TEXT: &str = "\
Start the read-only web interface for browsing JP conversations.

Usage: jp serve web [OPTIONS]

Options:
  --bind <ADDR>    Address to bind to [default: 127.0.0.1]
  --port <PORT>    Port to listen on [default: 3000]

Configuration (in .jp/config.toml):
  [plugins.command.serve.options]
  bind = \"0.0.0.0\"
  port = 8080";

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
            "Note: this binary is a JP plugin. Run it via `jp serve web`."
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

    // Send early protocol messages before sharing stdout.
    send(&mut stdout, &PluginToHost::Ready)?;
    send(
        &mut stdout,
        &PluginToHost::Print(PrintMessage {
            text: format!("Serving at http://{bind}:{port}\n"),
            channel: "content".into(),
            format: "plain".into(),
            language: None,
        }),
    )?;

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
        let addr = format!("{bind}:{port}");
        info!(%addr, "Starting web server");

        let mut shutdown = shutdown_rx;
        let shutdown_signal = async move {
            let _ = shutdown.changed().await;
        };

        routes::serve(client, &addr, shutdown_signal)
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
            description: "Read-only web UI for browsing conversations".to_owned(),
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
/// Events are buffered until the protocol writer is available. Returns a
/// handle that must be activated once the writer and log level are known.
fn init_tracing() -> ProtocolLogHandle {
    use tracing_subscriber::prelude::*;

    let (layer, handle) = ProtocolLogLayer::new();

    tracing_subscriber::registry().with(layer).init();

    handle
}
