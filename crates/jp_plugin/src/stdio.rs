//! Synchronous JSON-lines protocol helpers for command plugins.
//!
//! [`read`] and [`send`] exchange single protocol messages over stdin/stdout,
//! [`describe`] responds to `describe` requests for in-tree plugins,
//! [`print_to`] and [`print_json`] emit output through the host printer, and
//! [`send_exit`] / [`send_result`] terminate the session.
//! [`exit_code`] wraps a plugin binary's `main` entrypoint:
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

/// Default plugin author reported by [`describe`].
const AUTHOR: &str = "Jean Mertz <git@jeanmertz.com>";

/// Default repository URL reported by [`describe`].
const REPOSITORY: &str = "https://github.com/dcdpr/jp";

/// Read and deserialize the next [`HostToPlugin`] message from `stdin`.
///
/// `binary` is the plugin executable name (e.g. `"jp-ticket"`), included in the
/// error message if the host closes stdin unexpectedly (for example, when an
/// older `jp` rejects an unrecognized plugin message).
///
/// # Errors
///
/// Returns an error if reading fails, `stdin` reaches EOF, or the line is not a
/// valid [`HostToPlugin`] message.
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

/// Serialize `message` as a JSON line, write it to `stdout`, and flush.
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

/// Send a [`PluginToHost::Describe`] response for an in-tree JP plugin.
///
/// `manifest` is the constant produced by [`manifest!`], and `name`, `version`,
/// and `help` describe the plugin.
///
/// # Errors
///
/// Returns an error if `manifest` cannot be parsed or writing to `stdout`
/// fails.
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

/// Build a [`DescribeResponse`] for an in-tree JP plugin, populated with JP's
/// author and repository metadata.
///
/// # Errors
///
/// Returns an error if `manifest` is not a valid manifest line.
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

/// Send a plain-text `print` message on `channel` (`"content"` for stdout or
/// `"chrome"` for stderr output suppressed by `--quiet`).
///
/// # Errors
///
/// Returns an error if writing to `stdout` fails.
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

/// Send a JSON `print` message on the `"content"` channel.
///
/// # Errors
///
/// Returns an error if writing to `stdout` fails.
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

/// Send an `exit` message with `code` and an optional failure `reason`.
///
/// # Errors
///
/// Returns an error if writing to `stdout` fails.
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

/// Send an `exit` message for a command whose non-zero exit code reports a
/// check result (`expected = true`) rather than an unexpected failure.
///
/// # Errors
///
/// Returns an error if writing to `stdout` fails.
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

/// Run a plugin's `session` and return the process exit code, writing
/// diagnostics to `stderr`.
///
/// When `terminal` is `true` (the user invoked the `jp-<name>` binary directly
/// in a terminal rather than via `jp <command>`), prints `help` and a usage
/// note to `stderr` and returns `0` without waiting on stdin.
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
