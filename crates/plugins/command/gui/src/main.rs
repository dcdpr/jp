//! `jp-gui`: open the current workspace in the JP macOS app.
//!
//! A command plugin that hands the host's already-resolved workspace root to
//! the app and exits.
//! It does no path resolution of its own: `InitMessage` carries the root, so
//! `jp gui`, `jp -w ../other gui`, and `jp gui .` all arrive here already
//! answered.
//!
//! See: `docs/rfd/072-command-plugin-system.md`

use std::io::{self, BufRead, BufReader, IsTerminal as _, Write};

use jp_plugin::message::{DescribeResponse, ExitMessage, HostToPlugin, InitMessage, PluginToHost};

mod launch;

use launch::{Launcher, SystemLauncher};

const HELP_TEXT: &str = "\
Open the current workspace in the JP macOS app.

Usage: jp gui [PATH]

Arguments:
  [PATH]  A directory inside the workspace to open. Defaults to the workspace
          the rest of `jp` is using; pass `-w/--workspace` to target another.";

/// The app's bundle identifier, which is how macOS finds it without a path.
const BUNDLE_ID: &str = "computer.jp.jean-pierre";

/// The protocol version this plugin needs from the host.
///
/// It reads the workspace root out of `init` and launches an app, which the
/// first version carries, so anything able to spawn it will do.
const REQUIRED_PROTOCOL: u32 = 1;

fn main() {
    if io::stdin().is_terminal() {
        let mut err = io::stderr().lock();
        drop(writeln!(err, "{HELP_TEXT}"));
        drop(writeln!(err));
        drop(writeln!(
            err,
            "Note: this binary is a JP plugin. Run it via `jp gui`."
        ));
        std::process::exit(0);
    }

    let stdin = BufReader::new(io::stdin());
    let stdout = io::stdout();

    let code = match run(stdin, stdout, &SystemLauncher) {
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
    mut stdin: impl BufRead,
    mut stdout: impl Write,
    launcher: &impl Launcher,
) -> Result<(), String> {
    match read_message(&mut stdin)? {
        HostToPlugin::Describe => send_describe(&mut stdout),
        HostToPlugin::Init(init) => {
            match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
                Ok(ready) => send(&mut stdout, &PluginToHost::Ready(ready))?,
                Err(exit) => return send(&mut stdout, &PluginToHost::Exit(exit)),
            }

            open(&init, &mut stdout, launcher)
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

/// Launch the app on the workspace the host resolved.
fn open(
    init: &InitMessage,
    stdout: &mut impl Write,
    launcher: &impl Launcher,
) -> Result<(), String> {
    let root = init.workspace.root.as_str();

    match validate(init, root) {
        Err(reason) => send_exit(stdout, 1, Some(&reason)),
        Ok(()) => match launcher.launch(BUNDLE_ID, root) {
            Ok(()) => send_exit(stdout, 0, None),
            Err(reason) => send_exit(stdout, 1, Some(&reason)),
        },
    }
}

/// Check a trailing path argument against the workspace the host resolved.
///
/// The host has already picked the workspace, so a path that names a different
/// one is a mistake worth reporting rather than silently ignoring: the user
/// would otherwise get a window onto a workspace they did not ask for.
fn validate(init: &InitMessage, root: &str) -> Result<(), String> {
    let Some(argument) = init.args.first() else {
        return Ok(());
    };

    if argument.starts_with('-') {
        return Err(format!("unknown option: {argument}\n\n{HELP_TEXT}"));
    }

    if init.args.len() > 1 {
        return Err(format!("expected at most one path\n\n{HELP_TEXT}"));
    }

    // The host resolves `jp gui .` against the same workspace, so a relative
    // argument that is inside the resolved root is what "." and "src/" look like
    // by the time they reach here.
    let absolute = std::path::Path::new(argument)
        .canonicalize()
        .map_err(|e| format!("cannot resolve path '{argument}': {e}"))?;

    let root_path = std::path::Path::new(root)
        .canonicalize()
        .map_err(|e| format!("cannot resolve workspace root '{root}': {e}"))?;

    if absolute.starts_with(&root_path) {
        Ok(())
    } else {
        Err(format!(
            "'{argument}' is not inside the workspace at '{root}'. Use `jp -w {argument} gui` to \
             open it instead."
        ))
    }
}

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Describe(DescribeResponse {
            name: "gui".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Open the current workspace in the JP macOS app".to_owned(),
            command: vec!["gui".to_owned()],
            author: Some("Jean Mertz <git@jeanmertz.com>".to_owned()),
            help: Some(HELP_TEXT.to_owned()),
            repository: Some("https://github.com/dcdpr/jp".to_owned()),
        }),
    )
}

fn send_exit(stdout: &mut impl Write, code: u8, reason: Option<&str>) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Exit(ExitMessage {
            code,
            reason: reason.map(String::from),
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

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
