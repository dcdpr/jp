mod cmd;
mod ctx;
mod editor;
pub mod error;
mod parser;

use std::{
    fmt,
    io::{stdout, IsTerminal as _},
    num::NonZeroI32,
    time::Duration,
};

use clap::{
    builder::{BoolValueParser, TypedValueParser as _},
    ArgAction, Parser,
};
use cmd::{Commands, Output, Success};
use comfy_table::{Cell, CellAlignment, Row};
use crossterm::style::Stylize as _;
use ctx::Ctx;
use error::{Error, Result};
use jp_config::Config;
use jp_workspace::Workspace;
use serde_json::{Map, Value};
use tracing::{info, trace};

const DEFAULT_STORAGE_DIR: &str = ".jp";
const DEFAULT_VARIABLE_PREFIX: &str = "JP_";

// Jean Pierre's LLM Toolkit.
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(flatten, next_help_heading = "Global Options")]
    globals: Globals,

    #[command(subcommand, next_help_heading = "Options")]
    command: Commands,
}

#[derive(Debug, clap::Args)]
pub struct Globals {
    /// Override a configuration value for the duration of the command.
    #[arg(short, long, value_name = "KEY=VALUE", global = true, action = ArgAction::Append)]
    config: Vec<String>,

    /// Increase verbosity of logging.
    ///
    /// Can be specified multiple times to increase verbosity.
    ///
    /// Defaults to printing "error" messages. For each increase in verbosity,
    /// the log level is set to "warn", "info", "debug", and "trace"
    /// respectively.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    verbose: u8,

    /// Suppress all output, including errors.
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Use OCI-compliant terminal links.
    #[arg(
        long = "no-hyperlinks",
        global = true,
        default_value_t = false,
        value_parser = BoolValueParser::new().map(|v| !v),
        help = "Disable OCI-compliant terminal links."
    )]
    hyperlinks: bool,

    /// Use OCI-compliant terminal links.
    #[arg(
        long = "no-color",
        alias = "no-colors",
        global = true,
        default_value_t = false,
        value_parser = BoolValueParser::new().map(|v| !v),
        help = "Disable color in the output.",
    )]
    colors: bool,

    /// Persist modified state to disk.
    ///
    /// This is enabled by default, but can be disabled to debug certain
    /// actions. It is also useful to send a query to the assistant, without
    /// adding that query to the conversation history.
    #[arg(
        short = '!',
        long = "no-persist",
        alias = "no-persist",
        global = true,
        default_value_t = false,
        value_parser = BoolValueParser::new().map(|v| !v),
        help = "Disable persistence for the duration of the command."
    )]
    pub persist: bool,
    // TODO
    // /// The format of the output.
    // #[arg(long, global = true, value_enum, default_value_t = Format::Text)]
    // format: Format,
}

// TODO
// #[derive(Debug, Default, Clone, Copy, clap::ValueEnum)]
// enum Format {
//     /// Plain text output. No coloring or other formatting.
//     Text,
//
//     /// Pretty-printed text output. Includes coloring and hyperlinks.
//     #[default]
//     TextPretty
//
//     /// Compact JSON output.
//     Json,
//
//     /// Pretty-printed multi-line JSON output.
//     JsonPretty,
// }

impl fmt::Display for Cli {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entry(&"config", &self.globals.config)
            .entry(&"verbose", &self.globals.verbose)
            .entry(&"quiet", &self.globals.quiet)
            .finish()
    }
}

pub async fn run() {
    let cli = Cli::parse();
    let is_tty = stdout().is_terminal();

    configure_logging(cli.globals.verbose, cli.globals.quiet);
    trace!(command = cli.command.name(), arguments = %cli, "Starting CLI run.");

    let (code, output) = match run_inner(cli).await {
        Ok(output) if is_tty => (0, output_to_string(output)),
        Ok(output) => (0, parse_json_output(output)),
        Err(error) => parse_error(error, is_tty),
    };

    println!("{output}");
    std::process::exit(code);
}

async fn run_inner(cli: Cli) -> Result<Success> {
    match cli.command {
        Commands::Init(args) => args.run().map_err(Into::into),
        cmd => {
            let mut workspace = load_workspace()?;
            if !cli.globals.persist {
                workspace.disable_persistence();
            }

            let mut config = load_config(&workspace)?;
            apply_cli_configs(&cli.globals.config, &mut config)?;

            workspace.load()?;

            let mut ctx = Ctx::new(workspace, cli.globals, config);
            let output = cmd.run(&mut ctx).await;
            if output.is_err() {
                tracing::info!("Error running command. Disabling workspace persistence.");
                ctx.workspace.disable_persistence();
            }

            // Wait for background tasks to complete and sync their results to
            // the workspace.
            ctx.task_handler
                .sync(&mut ctx.workspace, Duration::from_secs(10))
                .await
                .map_err(Error::Task)?;

            output.map_err(Into::into)
        }
    }
}

fn output_to_string(output: Success) -> String {
    match output {
        Success::Ok => String::new(),
        Success::Message(msg) => msg,
        Success::Table { header, rows } => jp_term::table::list(header, rows),
        Success::Details { title, rows } => jp_term::table::details(title.as_deref(), rows),
        Success::Json(value) => value.to_string(),
    }
}

fn parse_json_output(output: Success) -> String {
    let value = match output {
        Success::Ok => serde_json::json!({}),
        Success::Message(msg) => serde_json::json!({ "message": msg }),
        Success::Table { header, rows } => jp_term::table::list_json(header, rows),
        Success::Details { title, rows } => jp_term::table::details_json(title.as_deref(), rows),
        Success::Json(value) => value,
    };

    serde_json::to_string(&value).unwrap_or_else(|_| value.to_string())
}

fn parse_error(error: error::Error, is_tty: bool) -> (i32, String) {
    let (code, message, mut metadata) = match error {
        error::Error::Command(error) => (error.code, error.message, error.metadata),
        _ => (
            NonZeroI32::new(1).unwrap(),
            Some(strip_ansi_escapes::strip_str(error.to_string())),
            Map::new(),
        ),
    };

    if is_tty {
        return (
            code.into(),
            jp_term::table::details(
                message.as_deref(),
                metadata
                    .into_iter()
                    .map(|(k, v)| {
                        let mut row = Row::new();
                        row.add_cell(Cell::new(k).set_alignment(CellAlignment::Right))
                            .add_cell(Cell::new(v).set_alignment(CellAlignment::Left));
                        row
                    })
                    .collect::<Vec<_>>(),
            ),
        );
    }

    let error = serde_json::json!({
        "message": message,
        "metadata": metadata,
        "code": code,
    });

    let error = serde_json::to_string(&error).unwrap_or_else(|err| {
        metadata.insert("source".to_owned(), Value::String(error.to_string()));

        let error = serde_json::json!({
            "message": err.to_string(),
            "metadata": metadata,
            "code": 127,
        });

        format!("{error}")
    });

    (code.into(), error)
}

/// Load the workspace configuration.
fn load_config(workspace: &Workspace) -> Result<Config> {
    let partial = if let Some(storage) = workspace.storage_path() {
        // First look for `config.toml` in the storage directory.
        let partial = jp_config::load_partial(&storage.join("config.toml"), false, None)?;

        // Then search for a config file, starting from the workspace root.
        jp_config::load_partial(&workspace.root, true, Some(partial))
    } else {
        // Search for a config file, starting from the workspace root.
        jp_config::load_partial(&workspace.root, true, None)
    }?;

    // Load environment variables.
    let partial = jp_config::load_envs(partial)?;

    // Build the final config.
    jp_config::build(partial).map_err(Into::into)
}

/// Find the workspace for the current directory.
fn load_workspace() -> Result<Workspace> {
    let cwd = std::env::current_dir()?;
    trace!(cwd = %cwd.display(), "Finding workspace.");

    let root = Workspace::find_root(cwd, DEFAULT_STORAGE_DIR).ok_or(cmd::Error::from(format!(
        "Could not locate workspace. Use `{}` to create a new workspace.",
        "jp init".bold().yellow()
    )))?;
    trace!(root = %root.display(), "Found workspace root.");

    let storage = root.join(DEFAULT_STORAGE_DIR);
    trace!(storage = %storage.display(), "Initializing workspace storage.");

    let id = jp_workspace::id::load(&storage).unwrap_or_else(jp_workspace::id::new);
    jp_id::global::set(id.clone());
    trace!(id, "Loaded unique workspace ID.");

    let workspace = Workspace::new_with_id(root, id)
        .persisted_at(&storage)
        .inspect(|ws| info!(workspace = %ws.root.display(), "Using existing workspace."))?;

    jp_workspace::id::store(workspace.id(), &storage)?;

    workspace.with_local_storage().map_err(Into::into)
}

/// Apply CLI config overrides to the [`Config`].
fn apply_cli_configs(overrides: &[String], config: &mut Config) -> Result<()> {
    trace!(overrides = ?overrides, "Applying CLI config overrides.");

    for field in overrides {
        let (key, value) = field.split_once('=').unwrap_or((field, ""));
        config.set(key, key, value)?;
    }

    Ok(())
}

fn configure_logging(verbose: u8, quiet: bool) {
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::fmt;

    let mut level = match verbose {
        0 => LevelFilter::ERROR,
        1 => LevelFilter::WARN,
        2 => LevelFilter::INFO,
        3 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    if quiet {
        level = LevelFilter::OFF;
    }

    let mut filter = vec!["off".to_owned()];
    for krate in [
        "attachment",
        "attachment_bear_note",
        "attachment_file_content",
        "cli",
        "config",
        "conversation",
        "format",
        "id",
        "llm",
        "mcp",
        "openrouter",
        "query",
        "task",
        "term",
        "test",
        "workspace",
    ] {
        filter.push(format!("jp_{krate}={level}"));
    }

    let format = fmt::format().with_target(false).compact();

    if level < LevelFilter::DEBUG {
        tracing_subscriber::fmt()
            .event_format(format)
            .without_time()
            .with_ansi(true)
            .with_target(false)
            .with_writer(std::io::stderr)
            .with_env_filter(filter.join(","))
            .init();
    } else {
        tracing_subscriber::fmt()
            .event_format(format)
            .with_ansi(true)
            .with_target(false)
            .with_writer(std::io::stderr)
            .with_env_filter(filter.join(","))
            .init();
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn test_cli() {
        Cli::command().debug_assert();
    }
}
