//! Plugin-side helpers for the command plugin JSON-lines protocol over `stdin`
//! and `stdout`.
//!
//! [`start`] handles the initial `describe` or `init` handshake, [`main`]
//! provides the standard `main()` entry point for plugin binaries, and the
//! remaining functions send or receive individual protocol messages.

use std::{
    io::{self, BufRead, BufReader, IsTerminal as _, Stdin, Stdout, Write},
    process::ExitCode,
};

use serde::Serialize;

use crate::{
    message::{
        ComposeRequest, DescribeResponse, ExitMessage, HostToPlugin, InitMessage, PluginToHost,
        PrintMessage, ToolOutcomeMessage,
    },
    ready,
};

/// Cargo package metadata for a plugin binary, constructed via [`package!`].
///
/// [`package!`]: crate::package!
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Package {
    pub version: &'static str,

    /// Colon-separated author list from `CARGO_PKG_AUTHORS`.
    pub authors: &'static str,

    /// Repository URL from `CARGO_PKG_REPOSITORY` (empty if unset).
    pub repository: &'static str,
}

/// Capture the calling crate's [`Package`] metadata at compile time.
#[macro_export]
macro_rules! package {
    () => {
        $crate::client::Package {
            version: env!("CARGO_PKG_VERSION"),
            authors: env!("CARGO_PKG_AUTHORS"),
            repository: env!("CARGO_PKG_REPOSITORY"),
        }
    };
}

/// Build a [`DescribeResponse`] for plugin `name` from its embedded `manifest`,
/// `help` text, and [`Package`] metadata.
///
/// # Errors
///
/// Returns an error if `manifest` cannot be parsed.
pub fn describe(
    manifest: &str,
    name: &str,
    help: String,
    package: Package,
) -> Result<DescribeResponse, String> {
    let mut describe = DescribeResponse::from_manifest(manifest, name, package.version, help)
        .map_err(|error| format!("invalid embedded manifest: {error}"))?;
    describe.author = (!package.authors.is_empty())
        .then(|| package.authors.split(':').collect::<Vec<_>>().join(", "));
    describe.repository = (!package.repository.is_empty()).then(|| package.repository.to_owned());

    Ok(describe)
}

/// Handle the host's initial message (`describe` or `init`).
///
/// - On [`HostToPlugin::Describe`], sends the response produced by `describe`
///   and returns `Ok(None)`.
/// - On [`HostToPlugin::Init`], verifies that `init.version >= required`.
///   If compatible, sends `ready` and returns `Ok(Some(init))`; otherwise sends
///   an `exit` error message and returns `Ok(None)`.
///
/// # Errors
///
/// Returns an error if reading or writing fails, or if the first message is
/// neither `describe` nor `init`.
pub fn start(
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
    binary: &str,
    required: u32,
    describe: impl FnOnce() -> Result<DescribeResponse, String>,
) -> Result<Option<InitMessage>, String> {
    let init = match read(stdin, binary)? {
        HostToPlugin::Describe => {
            send(stdout, &PluginToHost::Describe(describe()?))?;
            return Ok(None);
        }
        HostToPlugin::Init(init) => init,
        other => return Err(format!("expected init or describe, got: {other:?}")),
    };

    match ready(required, init.version) {
        Ok(ready) => send(stdout, &PluginToHost::Ready(ready))?,
        Err(exit) => {
            send(stdout, &PluginToHost::Exit(exit))?;
            return Ok(None);
        }
    }

    Ok(Some(*init))
}

/// Serialize and write `message` as a single JSON line to `stdout`, then flush.
///
/// # Errors
///
/// Returns an error if serialization, writing, or flushing fails.
pub fn send(stdout: &mut impl Write, message: &PluginToHost) -> Result<(), String> {
    let json =
        serde_json::to_string(message).map_err(|error| format!("serialize error: {error}"))?;
    writeln!(stdout, "{json}").map_err(|error| format!("write error: {error}"))?;

    stdout
        .flush()
        .map_err(|error| format!("flush error: {error}"))
}

/// Read and deserialize the next [`HostToPlugin`] message from `stdin`.
///
/// # Errors
///
/// Returns an error if `stdin` reaches EOF (naming `binary` to suggest checking
/// for a version mismatch with `jp`), if an I/O error occurs, or if the JSON is
/// malformed.
pub fn read(stdin: &mut impl BufRead, binary: &str) -> Result<HostToPlugin, String> {
    let mut line = String::new();
    let count = stdin
        .read_line(&mut line)
        .map_err(|error| format!("failed to read from host: {error}"))?;
    if count == 0 {
        return Err(format!(
            "The host stopped responding. Are `jp` and `{binary}` the same build?"
        ));
    }

    serde_json::from_str(line.trim()).map_err(|error| format!("invalid host message: {error}"))
}

/// Send a [`PrintMessage`] to the host.
///
/// # Errors
///
/// Returns an error if the message cannot be written to `stdout`.
pub fn print(
    stdout: &mut impl Write,
    text: &str,
    channel: &str,
    format: &str,
) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: channel.to_owned(),
            format: format.to_owned(),
            language: None,
        }),
    )
}

/// Send an [`ExitMessage`] with `code` and optional failure `reason`.
///
/// # Errors
///
/// Returns an error if the message cannot be written to `stdout`.
pub fn exit(stdout: &mut impl Write, code: u8, reason: Option<&str>) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Exit(ExitMessage {
            code,
            reason: reason.map(ToOwned::to_owned),
            expected: false,
        }),
    )
}

/// Send an [`ExitMessage`] with `code`, marking non-zero exit codes as
/// `expected: true` (for commands like `check` or `due` that use exit code 1 to
/// report a normal status result rather than a runtime error).
///
/// # Errors
///
/// Returns an error if the message cannot be written to `stdout`.
pub fn result(stdout: &mut impl Write, code: u8) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Exit(ExitMessage {
            code,
            reason: None,
            expected: code != 0,
        }),
    )
}

/// Send a [`ToolOutcomeMessage`] with `outcome` followed by a `0` exit message.
///
/// # Errors
///
/// Returns an error if `outcome` cannot be serialized or written to `stdout`.
pub fn answer_tool(stdout: &mut impl Write, outcome: &impl Serialize) -> Result<(), String> {
    let outcome = serde_json::to_value(outcome)
        .map_err(|error| format!("failed to serialize the tool outcome: {error}"))?;
    send(
        stdout,
        &PluginToHost::ToolOutcome(ToolOutcomeMessage { outcome }),
    )?;

    exit(stdout, 0, None)
}

/// Send a [`ComposeRequest`] to the host and wait for the user's response.
///
/// # Errors
///
/// Returns an error if the prompt was cancelled, the host returned an error or
/// shut down, or reading/writing failed.
pub fn compose(
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
    binary: &str,
    request: ComposeRequest,
) -> Result<String, String> {
    send(stdout, &PluginToHost::Compose(request))?;

    match read(stdin, binary)? {
        HostToPlugin::Composed(response) => response.text.ok_or_else(|| {
            "Nothing composed; run it again with what it asked for given as flags.".to_owned()
        }),
        HostToPlugin::Error(error) => Err(error.message),
        HostToPlugin::Shutdown => Err("Interrupted.".to_owned()),
        other => Err(format!("expected a composed response, got: {other:?}")),
    }
}

/// Entry point helper for command plugin binaries.
///
/// When invoked directly in a terminal (`stdin` is a TTY), prints `help()` and
/// a note to run `jp <command>` instead of blocking on `stdin`.
/// Otherwise runs `run` over `stdin` and `stdout`.
#[must_use]
pub fn main(
    command: &str,
    help: impl FnOnce() -> String,
    run: impl FnOnce(BufReader<Stdin>, Stdout) -> Result<(), String>,
) -> ExitCode {
    if io::stdin().is_terminal() {
        let mut stderr = io::stderr().lock();
        drop(writeln!(
            stderr,
            "{}\n\nNote: this binary is a JP plugin. Run it via `jp {command}`.",
            help()
        ));
        return ExitCode::SUCCESS;
    }

    match run(BufReader::new(io::stdin()), io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            drop(writeln!(io::stderr().lock(), "Fatal: {error}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
