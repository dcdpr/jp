//! `jp-rfd`: read and write a workspace's RFDs.
//!
//! A command plugin providing `jp rfd` and its subcommands, and the tools the
//! assistant reaches through `source = "plugin.command.rfd.<tool>"`.
//! An `init` carrying a tool call is answered by [`tool`]; any other runs the
//! command its arguments name.
//!
//! Settings come from `plugins.command.rfd.options` ([`options`]).
//! The RFD directory is resolved against the workspace root from the host's
//! `init` message, so the commands work from any subdirectory.
//!
//! [`board`] reads the RFDs, the priority board, and the tracking tickets into
//! one view; [`render`] prints it.
//!
//! See: `docs/rfd/001-jp-rfd-process.md`,
//! `docs/rfd/116-shareable-rfd-ticket-and-site-tooling.md`

mod board;
mod definitions;
mod document;
mod id;
mod options;
mod render;
mod tool;

use std::{
    io::{BufRead, Write},
    iter,
};

use camino::{Utf8Path, Utf8PathBuf};
use clap::{CommandFactory, Parser, Subcommand};
use jp_plugin::message::{
    DescribeResponse, ExitMessage, HostToPlugin, InitMessage, LogMessage, PluginToHost,
    PrintMessage, ToolCall, ToolOutcomeMessage,
};
use serde_json::Value;

use crate::{
    board::Loaded,
    id::{RfdId, resolve},
    options::{Options, ticket_dir},
    render::Sections,
};

#[derive(Debug, Parser)]
#[command(
    name = "jp rfd",
    about = "Write and track RFDs, design documents with a lifecycle."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List RFDs in priority order, optionally of one category.
    ///
    /// Only the planned RFDs are listed unless a section is named.
    List {
        /// Only RFDs of this category, such as `design`.
        category: Option<String>,

        /// The planned RFDs, in priority order.
        #[arg(long)]
        planned: bool,

        /// The RFDs on the board below the planned ones.
        #[arg(long)]
        backlog: bool,

        /// Every RFD: planned, backlog, and those the board doesn't place.
        #[arg(long)]
        all: bool,

        /// Add each RFD's summary and the RFDs it depends on.
        #[arg(long)]
        full: bool,

        /// Print every RFD as JSON, tagged with its section and rank.
        #[arg(long)]
        json: bool,
    },

    /// Show one RFD's metadata and where it stands on the priority board.
    Show {
        /// The RFD: a number (`42`, `042`) or a draft id (`D05`).
        id: RfdId,

        /// Print it as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Write the tool definitions that offer the RFDs to the assistant.
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ToolsCommand {
    /// Write a starting set of tool definitions, one file per tool.
    ///
    /// Refuses to overwrite a file that exists.
    /// The workspace configuration has to load the directory for the tools to
    /// appear.
    Init {
        /// Directory to write them into, relative to the workspace root.
        path: Utf8PathBuf,
    },

    /// Regenerate what the tool definitions mirror from configuration.
    ///
    /// Writes the configured categories into the schemas of the tools that take
    /// one, found by their `source`, as a file of its own that is rewritten
    /// whole.
    Sync {
        /// Directory holding the definitions, relative to the workspace root.
        path: Utf8PathBuf,
    },
}

/// What a subcommand produced: text for the user, warnings for the log.
///
/// Warnings stay out of the text so `--json` output survives a pipe into `jq`.
#[derive(Debug, Default)]
struct Output {
    text: String,
    warnings: Vec<String>,
}

impl From<String> for Output {
    fn from(text: String) -> Self {
        Self {
            text,
            warnings: vec![],
        }
    }
}

/// What a command runs against.
struct Context<'a> {
    /// The workspace root, which every relative path resolves against.
    root: &'a Utf8Path,

    /// The RFD directory.
    dir: Utf8PathBuf,

    /// The ticket directory, which says what is in development.
    tickets: Utf8PathBuf,

    /// The configuration the host resolved.
    config: &'a Value,

    /// The plugin's own settings.
    options: &'a Options,
}

impl Context<'_> {
    /// Read the board, the way every reading command starts.
    fn board(&self) -> Result<Loaded, String> {
        board::load(self.root, &self.dir, &self.tickets)
    }
}

/// Serve one plugin session: read the host's first message and answer it.
///
/// A `describe` is answered and ends the session.
/// An `init` carrying a tool call is answered with the call's outcome; any
/// other `init` runs the command its arguments name.
pub fn run(mut stdin: impl BufRead, mut stdout: impl Write) -> Result<(), String> {
    match read_message(&mut stdin)? {
        HostToPlugin::Describe => send_describe(&mut stdout),
        HostToPlugin::Init(init) => {
            match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
                Ok(ready) => send(&mut stdout, &PluginToHost::Ready(ready))?,
                Err(exit) => return send(&mut stdout, &PluginToHost::Exit(exit)),
            }

            match &init.tool {
                Some(call) => answer_tool_call(&init, call, &mut stdout),
                None => handle_command(&init, &mut stdout),
            }
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

/// The protocol version this plugin needs from the host.
///
/// 12 is the first to carry tool calls (`tool` on `init`, answered with
/// `tool_outcome`), which is how the `rfd_*` tools reach this plugin.
const REQUIRED_PROTOCOL: u32 = 12;

/// Where this plugin attaches to `jp`, readable from the binary without running
/// it.
///
/// `protocol` has to match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 12,
    description: "Write and track RFDs",
    command: ["rfd"],
);

/// Answer the tool call `init` carried, then exit.
///
/// The host closed stdin after `init`, so nothing here may ask it anything.
/// Options the plugin can't read end the call as a failed plugin run rather
/// than as an outcome: the assistant can't fix the configuration.
fn answer_tool_call(
    init: &InitMessage,
    call: &ToolCall,
    stdout: &mut impl Write,
) -> Result<(), String> {
    let options = match Options::read(&init.options) {
        Ok(options) => options,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };

    let outcome = serde_json::to_value(tool::run(init, call, &options))
        .map_err(|error| format!("failed to serialize the tool outcome: {error}"))?;
    send(
        stdout,
        &PluginToHost::ToolOutcome(ToolOutcomeMessage { outcome }),
    )?;

    send_exit(stdout, 0, None)
}

fn handle_command(init: &InitMessage, stdout: &mut impl Write) -> Result<(), String> {
    let parsed = Args::try_parse_from(iter::once("jp rfd".to_owned()).chain(init.args.clone()));

    let args = match parsed {
        Ok(args) => args,
        // `--help` and `--version` surface as errors that aren't failures.
        Err(error) if !error.use_stderr() => {
            print(stdout, &error.to_string())?;
            return send_exit(stdout, 0, None);
        }
        Err(error) => return send_exit(stdout, 1, Some(&error.to_string())),
    };

    let options = match Options::read(&init.options) {
        Ok(options) => options,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };
    let tickets = match ticket_dir(&init.config) {
        Ok(tickets) => tickets,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };

    let root = init.workspace.root.as_path();
    let ctx = Context {
        root,
        dir: resolve_dir(root, &options.dir),
        tickets: resolve_dir(root, &tickets),
        config: &init.config,
        options: &options,
    };

    match execute(&ctx, args.command) {
        Ok(output) => {
            for warning in &output.warnings {
                send(
                    stdout,
                    &PluginToHost::Log(LogMessage {
                        level: "warn".to_owned(),
                        message: warning.clone(),
                        fields: serde_json::Map::new(),
                    }),
                )?;
            }
            if !output.text.is_empty() {
                print(stdout, &output.text)?;
            }
            send_exit(stdout, 0, None)
        }
        Err(message) => send_exit(stdout, 1, Some(&message)),
    }
}

/// Resolve a directory against the workspace root.
///
/// An absolute path is taken as given; a relative one hangs off the root, so
/// the commands behave the same from any subdirectory.
fn resolve_dir(root: &Utf8Path, dir: &Utf8Path) -> Utf8PathBuf {
    if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        root.join(dir)
    }
}

fn execute(ctx: &Context<'_>, command: Command) -> Result<Output, String> {
    match command {
        Command::List {
            category,
            planned,
            backlog,
            all,
            full,
            json,
        } => {
            let Loaded { board, warnings } = ctx.board()?;
            let text = if json {
                render::list_json(&board, category.as_deref())?
            } else {
                let sections = Sections::from_flags(planned, backlog, all);
                render::list_text(&board, category.as_deref(), sections, full)
            };

            Ok(Output { text, warnings })
        }

        Command::Show { id, json } => {
            let Loaded { board, warnings } = ctx.board()?;
            let file = resolve(&ctx.dir, id).map_err(|error| error.to_string())?;
            let file = board::relative(ctx.root, &file);
            let entry = board
                .entry(&file)
                .ok_or_else(|| format!("{file} is not an RFD: its name has no slug."))?;

            let text = if json {
                render::show_json(&board, entry)?
            } else {
                render::show_text(&board, entry)
            };

            Ok(Output { text, warnings })
        }

        Command::Tools {
            command: ToolsCommand::Init { path },
        } => definitions::init(&resolve_dir(ctx.root, &path)),

        Command::Tools {
            command: ToolsCommand::Sync { path },
        } => definitions::sync(
            &resolve_dir(ctx.root, &path),
            ctx.config,
            &ctx.options.categories,
        ),
    }
}

/// The usage text, rendered from the same clap definition the commands parse.
#[must_use]
pub fn help_text() -> String {
    Args::command().render_help().to_string()
}

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    let mut describe =
        DescribeResponse::from_manifest(MANIFEST, "rfd", env!("CARGO_PKG_VERSION"), help_text())
            .map_err(|e| format!("invalid embedded manifest: {e}"))?;

    describe.author = Some("Jean Mertz <git@jeanmertz.com>".to_owned());
    describe.repository = Some("https://github.com/dcdpr/jp".to_owned());

    send(stdout, &PluginToHost::Describe(describe))
}

fn print(stdout: &mut impl Write, text: &str) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: "content".into(),
            format: "plain".into(),
            language: None,
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
    let read = stdin
        .read_line(&mut line)
        .map_err(|error| format!("failed to read from host: {error}"))?;

    // EOF: the host closed the pipe without answering, which is what an older
    // `jp` does when it gives up on a message it couldn't read.
    if read == 0 {
        return Err(
            "The host stopped responding. Are `jp` and `jp-rfd` the same build?".to_owned(),
        );
    }

    serde_json::from_str(line.trim()).map_err(|error| format!("invalid host message: {error}"))
}

fn send(stdout: &mut impl Write, msg: &PluginToHost) -> Result<(), String> {
    let json = serde_json::to_string(msg).map_err(|error| format!("serialize error: {error}"))?;
    writeln!(stdout, "{json}").map_err(|error| format!("write error: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush error: {error}"))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
