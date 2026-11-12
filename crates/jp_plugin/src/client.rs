//! The plugin's side of the protocol: reading what the host sends on the
//! plugin's stdin, and sending the plugin's messages on its stdout.
//!
//! [`start`] answers the host's first message, [`main`] is the body of a plugin
//! binary's `main`, and the rest send or wait for one message each.
//! Every function reports a failure as a message for the user, the way a
//! plugin's `run` returns it.

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

/// The package a plugin binary is built from, as Cargo describes it.
///
/// Build it with [`package!`], in the plugin's crate.
///
/// [`package!`]: crate::package!
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Package {
    pub version: &'static str,

    /// The package's authors, separated by `:`, as `CARGO_PKG_AUTHORS` holds
    /// them.
    pub authors: &'static str,

    /// The package's repository address; empty when it names none.
    pub repository: &'static str,
}

/// The [`Package`] of the crate the macro is used in.
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

/// The answer to `describe` for the plugin `name`, from its embedded
/// `manifest`, its `help` text, and the `package` it is built from.
///
/// # Errors
///
/// Returns why `manifest` can't be read.
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

/// Answer the host's first message.
///
/// A `describe` is answered with what `describe` returns, which ends the
/// session.
/// An `init` from a host speaking at least protocol `required` is answered with
/// `ready` and returned; one from an older host is answered with `exit`, which
/// ends the session.
/// `binary` names the plugin's executable in the error for a host that stopped
/// responding.
///
/// # Errors
///
/// Returns why the first message can't be read or answered, or that it was
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

/// Send `message` to the host, as one line of JSON.
///
/// # Errors
///
/// Returns why the message can't be written.
pub fn send(stdout: &mut impl Write, message: &PluginToHost) -> Result<(), String> {
    let json =
        serde_json::to_string(message).map_err(|error| format!("serialize error: {error}"))?;
    writeln!(stdout, "{json}").map_err(|error| format!("write error: {error}"))?;

    stdout
        .flush()
        .map_err(|error| format!("flush error: {error}"))
}

/// Wait for the host's next message.
///
/// # Errors
///
/// Returns why the message can't be read.
/// A host that closed the pipe without answering is reported as one that may be
/// another build than `binary`, the plugin's executable: an older `jp` does
/// that when it gives up on a message it couldn't read.
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

/// Print `text`, written in `format`, on the host's `channel`.
///
/// # Errors
///
/// Returns why the message can't be written.
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

/// End the session with exit code `code`: success with 0, and otherwise a
/// failure, which the host reports with `reason`.
///
/// # Errors
///
/// Returns why the message can't be written.
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

/// End the session with exit code `code`, which reports the command's result
/// rather than a failure, as a check that found problems does.
///
/// # Errors
///
/// Returns why the message can't be written.
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

/// Answer the tool call `init` carried with `outcome`, and end the session.
///
/// `outcome` is serialized as a local tool prints its outcome.
///
/// # Errors
///
/// Returns why `outcome` can't be serialized or the messages written.
pub fn answer_tool(stdout: &mut impl Write, outcome: &impl Serialize) -> Result<(), String> {
    let outcome = serde_json::to_value(outcome)
        .map_err(|error| format!("failed to serialize the tool outcome: {error}"))?;
    send(
        stdout,
        &PluginToHost::ToolOutcome(ToolOutcomeMessage { outcome }),
    )?;

    exit(stdout, 0, None)
}

/// Ask the host to collect text from the user, as `request` says, and wait for
/// it.
///
/// # Errors
///
/// Returns that nothing was composed, the host's error, that the host shut
/// down, or why the exchange failed; `binary` is as [`read`] takes it.
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

/// The body of a plugin binary's `main`: serve the host with `run` over stdin
/// and stdout, and exit with a failure when `run` fails.
///
/// Run from a terminal rather than by `jp`, the binary prints `help` and says
/// to run it as `jp <command>` instead of waiting for a host that isn't there.
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
