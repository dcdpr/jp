//! `jp-path`: print JP directory paths.
//!
//! A command plugin that prints well-known JP directory paths for use in shell
//! scripts and automation.
//! All paths come from the host's `init` message, so this plugin has no
//! platform-specific logic.
//!
//! See: `docs/rfd/072-command-plugin-system.md`

use std::{
    io::{self, BufRead, IsTerminal as _, Write},
    process,
};

use jp_plugin::{
    message::{HostToPlugin, InitMessage, PluginToHost},
    stdio::{self, print_to, send, send_exit},
};

const BINARY: &str = "jp-path";

/// The protocol version this plugin needs from the host.
///
/// It only reads paths out of `init`, so anything that can spawn it will do.
const REQUIRED_PROTOCOL: u32 = 1;

/// Where this plugin attaches to `jp`, readable from the binary without running
/// it.
///
/// `protocol` has to match [`REQUIRED_PROTOCOL`].
static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 1,
    description: "Print JP directory paths",
    command: ["path"],
);

const HELP_TEXT: &str = "\
Print JP directory paths.

Usage: jp path <COMMAND> [OPTIONS]

Commands:
  user-local       User-local data directory
  user-config      User-global config directory
  workspace        Workspace storage directory (.jp)
  user-workspace   User-local workspace storage directory

Options for user-local:
  --plugins=command  Print the command plugin install directory";

fn main() {
    let code = stdio::exit_code(
        "path",
        io::stdin().is_terminal(),
        || HELP_TEXT.to_owned(),
        || run(io::stdin().lock(), io::stdout()),
        &mut io::stderr(),
    );

    process::exit(code);
}

fn run(mut stdin: impl BufRead, mut stdout: impl Write) -> Result<(), String> {
    let first_msg = stdio::read(&mut stdin, BINARY)?;

    match first_msg {
        HostToPlugin::Describe => stdio::describe(
            &mut stdout,
            MANIFEST,
            "path",
            env!("CARGO_PKG_VERSION"),
            HELP_TEXT,
        ),
        HostToPlugin::Init(init) => {
            match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
                Ok(ready) => send(&mut stdout, &PluginToHost::Ready(ready))?,
                Err(exit) => return send(&mut stdout, &PluginToHost::Exit(exit)),
            }
            handle_command(&init, &mut stdout)
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

fn handle_command(init: &InitMessage, stdout: &mut impl Write) -> Result<(), String> {
    let args = &init.args;
    let subcommand = args.first().map(String::as_str);

    let result = match subcommand {
        Some("user-local") => handle_user_local(init, args),
        Some("user-config") => handle_user_config(init),
        Some("workspace") => Ok(init.workspace.storage.to_string()),
        Some("user-workspace") => handle_user_workspace(init),
        Some(other) => Err(format!("unknown subcommand: {other}\n\n{HELP_TEXT}")),
        None => Err(format!("missing subcommand\n\n{HELP_TEXT}")),
    };

    match result {
        Ok(path) => {
            print_to(stdout, &format!("{path}\n"), "content")?;
            send_exit(stdout, 0, None)
        }
        Err(msg) => send_exit(stdout, 1, Some(&msg)),
    }
}

fn handle_user_local(init: &InitMessage, args: &[String]) -> Result<String, String> {
    let base = init
        .paths
        .user_data
        .as_ref()
        .ok_or("host did not provide user data path")?;

    // Check for --plugins=command flag.
    let plugins_value = args.iter().find_map(|a| a.strip_prefix("--plugins="));

    match plugins_value {
        Some("command") => Ok(format!("{base}/plugins/command")),
        Some(other) => Err(format!("unknown --plugins value: {other}")),
        None => Ok(base.to_string()),
    }
}

fn handle_user_config(init: &InitMessage) -> Result<String, String> {
    init.paths
        .user_config
        .as_ref()
        .map(ToString::to_string)
        .ok_or_else(|| "host did not provide user config path".to_owned())
}

fn handle_user_workspace(init: &InitMessage) -> Result<String, String> {
    init.paths
        .user_workspace
        .as_ref()
        .map(ToString::to_string)
        .ok_or_else(|| "no user-workspace storage configured for this workspace".to_owned())
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
