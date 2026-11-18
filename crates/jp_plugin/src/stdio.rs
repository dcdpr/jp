//! A plugin's end of the protocol: one JSON message per line, read from the
//! host on the plugin's stdin and written to it on its stdout.
//!
//! [`read`] and [`send`] exchange one message each, and [`describe`] answers
//! the host's `describe` for a plugin in JP's repository.
//! [`print_to`] and [`print_json`] print through the host, and [`send_exit`]
//! and [`send_result`] end the session.
//! [`exit_code`] is what a plugin executable's `main` runs around its session:
//!
//! ```ignore
//! fn main() {
//!     let code = jp_plugin::stdio::exit_code(
//!         "path",
//!         io::stdin().is_terminal(),
//!         help_text,
//!         || run(BufReader::new(io::stdin()), io::stdout()),
//!         &mut io::stderr(),
//!     );
//!
//!     process::exit(code);
//! }
//! ```

use std::io::{BufRead, Write};

use crate::message::{DescribeResponse, ExitMessage, HostToPlugin, PluginToHost, PrintMessage};

/// Who [`describe`] credits a plugin to.
const AUTHOR: &str = "Jean Mertz <git@jeanmertz.com>";

/// Where [`describe`] says a plugin is published.
const REPOSITORY: &str = "https://github.com/dcdpr/jp";

/// Read the host's next message from `stdin`.
///
/// `binary` names the plugin's executable, as in `jp-ticket`, for the error
/// when the host closes the pipe without answering: an older `jp` does that
/// when it can't read the message the plugin sent.
///
/// # Errors
///
/// When the line can't be read, the pipe is closed, or the line isn't a
/// message.
pub fn read(stdin: &mut impl BufRead, binary: &str) -> Result<HostToPlugin, String> {
    let mut line = String::new();
    let read = stdin
        .read_line(&mut line)
        .map_err(|error| format!("failed to read from host: {error}"))?;

    if read == 0 {
        return Err(format!(
            "The host stopped responding. Are `jp` and `{binary}` the same build?"
        ));
    }

    serde_json::from_str(line.trim()).map_err(|error| format!("invalid host message: {error}"))
}

/// Write `message` to the host on `stdout`, as one line, and flush it.
///
/// # Errors
///
/// When `stdout` refuses the line.
pub fn send(stdout: &mut impl Write, message: &PluginToHost) -> Result<(), String> {
    let json =
        serde_json::to_string(message).map_err(|error| format!("serialize error: {error}"))?;
    writeln!(stdout, "{json}").map_err(|error| format!("write error: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush error: {error}"))
}

/// Answer the host's `describe` for a plugin in JP's repository, credited to
/// JP's author.
///
/// `manifest` is the line [`manifest!`] built, and `name`, `version`, and
/// `help` are the plugin's.
///
/// # Errors
///
/// When `manifest` isn't a manifest line, or `stdout` refuses the answer.
///
/// [`manifest!`]: crate::manifest!
pub fn describe(
    stdout: &mut impl Write,
    manifest: &str,
    name: &str,
    version: &str,
    help: impl Into<String>,
) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Describe(describe_response(manifest, name, version, help)?),
    )
}

/// The answer to the host's `describe` for a plugin in JP's repository, for a
/// plugin that sends it some other way than [`describe`].
///
/// # Errors
///
/// When `manifest` isn't a manifest line.
pub fn describe_response(
    manifest: &str,
    name: &str,
    version: &str,
    help: impl Into<String>,
) -> Result<DescribeResponse, String> {
    let mut response = DescribeResponse::from_manifest(manifest, name, version, help)
        .map_err(|error| format!("invalid embedded manifest: {error}"))?;
    response.author = Some(AUTHOR.to_owned());
    response.repository = Some(REPOSITORY.to_owned());

    Ok(response)
}

/// Print `text` on the host's output channel `channel`: `content`, what a
/// script reads on stdout, or `chrome`, notes on stderr that `--quiet` hides.
///
/// # Errors
///
/// When `stdout` refuses the message.
pub fn print_to(stdout: &mut impl Write, text: &str, channel: &str) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: channel.to_owned(),
            format: "plain".into(),
            language: None,
        }),
    )
}

/// Print `text`, one JSON value, for the host to emit as a record of its own.
///
/// # Errors
///
/// When `stdout` refuses the message.
pub fn print_json(stdout: &mut impl Write, text: &str) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: "content".to_owned(),
            format: "json".into(),
            language: None,
        }),
    )
}

/// End the session with `code`: success when it is 0, and otherwise a failure
/// the host reports with `reason`.
///
/// # Errors
///
/// When `stdout` refuses the message.
pub fn send_exit(stdout: &mut impl Write, code: u8, reason: Option<&str>) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Exit(ExitMessage {
            code,
            reason: reason.map(String::from),
            expected: false,
        }),
    )
}

/// End the session with `code`, the result a command reports by its exit
/// status, as a check that found problems does: `jp` exits with it without
/// treating it as a failure.
///
/// # Errors
///
/// When `stdout` refuses the message.
pub fn send_result(stdout: &mut impl Write, code: u8) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Exit(ExitMessage {
            code,
            reason: None,
            expected: code != 0,
        }),
    )
}

/// The exit status of a plugin executable, whose messages go to `stderr`.
///
/// From a terminal (`terminal`), no host is talking to the executable, so it
/// prints `help` and points at `jp {command}` instead of waiting for a message
/// that never comes.
/// Otherwise it runs `session`, reporting the error that ends it.
pub fn exit_code(
    command: &str,
    terminal: bool,
    help: impl FnOnce() -> String,
    session: impl FnOnce() -> Result<(), String>,
    stderr: &mut impl Write,
) -> i32 {
    if terminal {
        drop(writeln!(stderr, "{}\n", help()));
        drop(writeln!(
            stderr,
            "Note: this binary is a JP plugin. Run it via `jp {command}`."
        ));
        return 0;
    }

    match session() {
        Ok(()) => 0,
        Err(error) => {
            drop(writeln!(stderr, "Fatal: {error}"));
            1
        }
    }
}

#[cfg(test)]
#[path = "stdio_tests.rs"]
mod tests;
