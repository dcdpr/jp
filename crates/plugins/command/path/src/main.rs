//! `jp-path`: print JP directory paths.
//!
//! A command plugin that prints well-known JP directory paths for use in
//! shell scripts and automation. All paths come from the host's `init`
//! message, so this plugin has no platform-specific logic.
//!
//! See: `docs/rfd/D17-command-plugin-system.md`

use std::io::{self, BufRead, BufReader, IsTerminal as _, Write};

use jp_plugin::message::{
    DescribeResponse, ExitMessage, HostToPlugin, InitMessage, PluginToHost, PrintMessage,
};

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
    if io::stdin().is_terminal() {
        let mut err = io::stderr().lock();
        drop(writeln!(err, "{HELP_TEXT}"));
        drop(writeln!(err));
        drop(writeln!(
            err,
            "Note: this binary is a JP plugin. Run it via `jp path`."
        ));
        std::process::exit(0);
    }

    let stdin = BufReader::new(io::stdin());
    let stdout = io::stdout();

    let code = match run(stdin, stdout) {
        Ok(()) => 0,
        Err(e) => {
            let mut err = io::stderr().lock();
            drop(writeln!(err, "Fatal: {e}"));
            1
        }
    };

    std::process::exit(code);
}

fn run(mut stdin: impl BufRead, mut stdout: impl Write) -> Result<(), String> {
    let first_msg = read_message(&mut stdin)?;

    match first_msg {
        HostToPlugin::Describe => send_describe(&mut stdout),
        HostToPlugin::Init(init) => {
            send(&mut stdout, &PluginToHost::Ready)?;
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
            send(
                stdout,
                &PluginToHost::Print(PrintMessage {
                    text: format!("{path}\n"),
                    channel: "content".into(),
                    format: "plain".into(),
                    language: None,
                }),
            )?;
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

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Describe(DescribeResponse {
            name: "path".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Print JP directory paths".to_owned(),
            command: vec!["path".to_owned()],
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
