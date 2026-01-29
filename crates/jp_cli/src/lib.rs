mod cmd;
mod ctx;
mod editor;
mod error;
mod parser;
mod signals;

use std::{
    error::Error as _,
    fmt,
    io::{IsTerminal as _, stdout},
    num::{NonZeroU8, NonZeroUsize},
    path::PathBuf,
    process::ExitCode,
    str::FromStr,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use camino::{FromPathBufError, Utf8PathBuf, absolute_utf8};
use clap::{
    ArgAction, Parser,
    builder::{BoolValueParser, TypedValueParser as _},
};
use cmd::{Commands, Output, Success};
use comfy_table::{Cell, CellAlignment, Row};
use crossterm::style::Stylize as _;
use ctx::{Ctx, IntoPartialAppConfig};
use error::{Error, Result};
use jp_config::{
    PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
    fs::{load_partial, user_global_config_path},
    util::{
        build, find_file_in_load_path, load_envs, load_partial_at_path,
        load_partial_at_path_recursive, load_partials_with_inheritance,
    },
};
use jp_printer::Printer;
use jp_workspace::{Workspace, user_data_dir};
use serde_json::Value;
use tokio::runtime::{self, Runtime};
use tracing::{debug, info, trace, warn};

static WORKER_THREADS: AtomicUsize = AtomicUsize::new(0);

const DEFAULT_STORAGE_DIR: &str = ".jp";

#[expect(dead_code)]
const DEFAULT_VARIABLE_PREFIX: &str = "JP_";

/// The prefix used to parse a CLI argument as a path instead of a string.
const PATH_STRING_PREFIX: char = '@';

// Jean Pierre's LLM Toolkit.
#[derive(Parser)]
#[command(author, version, long_version = env!("LONG_VERSION"), about, long_about = None)]
struct Cli {
    #[command(flatten, next_help_heading = "Global Options")]
    globals: Globals,

    #[command(flatten)]
    root: RootOpts,

    #[command(subcommand, next_help_heading = "Options")]
    command: Commands,
}

/// The root options for the CLI.
///
/// These options are only available at the root level, e.g. `jp --foo` but not
/// `jp query --foo`.
#[derive(Parser)]
pub struct RootOpts {
    /// Number of threads to use for processing (default is number of available
    /// cores)
    #[arg(short = 't', long = "threads")]
    pub threads: Option<NonZeroUsize>,
}

#[derive(Debug, Default, clap::Args)]
struct Globals {
    /// Override a configuration value for the duration of the command.
    #[arg(
        short = 'c',
        long = "cfg",
        global = true,
        action = ArgAction::Append,
        value_name = "KEY=VALUE",
        value_parser = KeyValueOrPath::from_str,
    )]
    config: Vec<KeyValueOrPath>,

    #[arg(
        short = 'I',
        long = "no-inherit",
        global = true,
        value_parser = BoolValueParser::new().map(|v| !v),
        default_value_t = true,
        help = "Disable loading of non-CLI provided config.",
    )]
    load_non_cli_config: bool,

    /// Increase verbosity of logging.
    ///
    /// Can be specified multiple times to increase verbosity.
    ///
    /// Defaults to printing "error" messages. For each increase in verbosity,
    /// the log level is set to "warn", "info", "debug", and "trace"
    /// respectively.
    #[arg(short = 'v', long, global = true, action = ArgAction::Count)]
    verbose: u8,

    /// Suppress all output, including errors.
    #[arg(short = 'q', long, global = true)]
    quiet: bool,

    /// Use OCI-compliant terminal links.
    #[arg(
        short = 'H',
        long = "no-hyperlinks",
        global = true,
        default_value_t = false,
        value_parser = BoolValueParser::new().map(|v| !v),
        help = "Disable OCI-compliant terminal links.",
    )]
    hyperlinks: bool,

    /// Use OCI-compliant terminal links.
    #[arg(
        short = 'C',
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
        visible_short_alias = 'P',
        global = true,
        default_value_t = false,
        value_parser = BoolValueParser::new().map(|v| !v),
        help = "Disable persistence for the duration of the command.",
    )]
    persist: bool,

    /// The workspace to use for the command.
    ///
    /// This can be either a path to a workspace directory, or a workspace ID.
    #[arg(short = 'w', long, global = true)]
    workspace: Option<WorkspaceIdOrPath>,
    // TODO
    // /// The format of the output.
    // #[arg(long, global = true, value_enum, default_value_t = Format::Text)]
    // format: Format,
}

#[derive(Debug, Clone)]
pub(crate) enum KeyValueOrPath {
    KeyValue(KvAssignment),
    Path(PathBuf),
}

impl FromStr for KeyValueOrPath {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        // String prefixed with `@` is always a path.
        if let Some(s) = s.strip_prefix(PATH_STRING_PREFIX) {
            return Ok(Self::Path(PathBuf::from(s.trim())));
        }

        // String without `=` is always a path.
        if !s.contains('=') {
            return Ok(Self::Path(PathBuf::from(s.trim())));
        }

        // Anything else is parsed as a key-value pair.
        s.parse().map(Self::KeyValue).map_err(Into::into)
    }
}

#[derive(Debug, Clone)]
pub(crate) enum WorkspaceIdOrPath {
    Id(jp_workspace::Id),
    Path(Utf8PathBuf),
}

impl FromStr for WorkspaceIdOrPath {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        if PathBuf::from(s).exists() {
            return Ok(Self::Path(Utf8PathBuf::from(s)));
        }

        Ok(Self::Id(jp_workspace::Id::from_str(s)?))
    }
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

pub fn run() -> ExitCode {
    #[cfg(feature = "dhat")]
    let _profiler = run_dhat();

    let cli = Cli::parse();
    let is_tty = stdout().is_terminal();

    configure_logging(cli.globals.verbose, cli.globals.quiet);
    trace!(command = cli.command.name(), arguments = %cli, "Starting CLI run.");

    let (code, output) = match run_inner(cli) {
        Ok(output) if is_tty => (0, output_to_string(output)),
        Ok(output) => (0, parse_json_output(output)),
        Err(error) => parse_error(error, is_tty),
    };

    #[expect(clippy::print_stdout, clippy::print_stderr)]
    if code == 0 {
        println!("{output}");
    } else {
        eprintln!("{output}");
    }

    #[cfg(feature = "dhat")]
    #[expect(clippy::print_stderr)]
    {
        eprintln!("You can view the heap profile at https://profiler.firefox.com");
    }

    ExitCode::from(code)
}

fn run_inner(cli: Cli) -> Result<Success> {
    let printer = Printer::terminal(jp_printer::Format::Text);

    match cli.command {
        Commands::Init(ref args) => args.run(&printer).map_err(Into::into),
        cmd => {
            let mut workspace = load_workspace(cli.globals.workspace.as_ref())?;
            if !cli.globals.persist {
                workspace.disable_persistence();
            }

            let runtime = build_runtime(cli.root.threads, "jp-worker")?;
            workspace.load()?;

            let partial = load_partial_config(&cmd, Some(&workspace), &cli.globals.config)?;
            let config = build(partial.clone())?;

            let mut ctx = Ctx::new(workspace, runtime, cli.globals, config, printer);
            let handle = ctx.handle().clone();

            let output = handle.block_on(cmd.run(&mut ctx));
            if let Err(err) = output.as_ref()
                && err.disable_persistence
            {
                tracing::info!("Error running command. Disabling workspace persistence.");
                ctx.workspace.disable_persistence();
            }

            // Wait for background tasks to complete and sync their results to
            // the workspace.
            handle
                .block_on(
                    ctx.task_handler
                        .sync(&mut ctx.workspace, Duration::from_secs(10)),
                )
                .map_err(Error::Task)?;

            // Remove ephemeral conversations that are no longer needed.
            ctx.workspace.remove_ephemeral_conversations();

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
        Success::Json(value) => format!("{value:#}"),
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

fn parse_error(error: error::Error, is_tty: bool) -> (u8, String) {
    let (code, message, mut metadata) = match error {
        error::Error::Command(error) => (error.code, error.message, error.metadata),
        _ => (
            NonZeroU8::new(1).unwrap(),
            Some(strip_ansi_escapes::strip_str(error.to_string())),
            {
                let mut metadata = vec![];
                let mut source = error.source();
                while let Some(error) = source {
                    metadata.push((String::new(), error.to_string().into()));
                    source = error.source();
                }

                metadata
            },
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
                            .add_cell(
                                Cell::new(match v {
                                    Value::String(s) => s,
                                    v => format!("{v:#}"),
                                })
                                .set_alignment(CellAlignment::Left),
                            );
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
        metadata.push(("source".to_owned(), Value::String(error.to_string())));

        let error = serde_json::json!({
            "message": err.to_string(),
            "metadata": metadata,
            "code": 127,
        });

        format!("{error}")
    });

    (code.into(), error)
}

/// Load the static partial workspace configuration.
///
/// This uses all configuration sources known at the start of the CLI run.
///
/// See: <https://jp.computer/configuration>
fn load_partial_config(
    cmd: &Commands,
    workspace: Option<&Workspace>,
    overrides: &[KeyValueOrPath],
) -> Result<PartialAppConfig> {
    // Load all partials in different file locations, the first loaded file
    // having the lowest precedence.
    let partials = load_partial_configs_from_files(workspace, absolute_utf8(".").ok())?;

    // Load all partials, merging later partials over earlier ones, unless one
    // of the partials set `inherit = false`, then later partials are ignored.
    let mut partial = load_partials_with_inheritance(partials)?;

    // Load environment variables.
    partial = load_envs(partial).map_err(|e| Error::CliConfig(e.to_string()))?;

    // Apply conversation-specific config, if needed.
    if let Some(workspace) = workspace {
        partial = cmd
            .apply_conversation_config(Some(workspace), partial, None)
            .map_err(|e| Error::CliConfig(e.to_string()))?;
    }

    // Load CLI-provided `--cfg` arguments. These are different from
    // command-specific CLI arguments, in that they are global, and allow you to
    // change any field in the [`Config`] struct.
    partial = load_cli_cfg_args(partial, overrides, workspace)?;

    // Load command-specific CLI arguments last (e.g. `jp query --model`).
    partial = cmd
        .apply_cli_config(workspace, partial, None)
        .map_err(|e| Error::CliConfig(e.to_string()))?;

    Ok(partial)
}

fn load_cli_cfg_args(
    mut partial: PartialAppConfig,
    overrides: &[KeyValueOrPath],
    workspace: Option<&Workspace>,
) -> Result<PartialAppConfig> {
    for field in overrides {
        match field {
            KeyValueOrPath::Path(path) if path.exists() => {
                if let Some(p) = load_partial_at_path(path)? {
                    partial = load_partial(partial, p)?;
                }
            }
            KeyValueOrPath::Path(path) => {
                // Get the list of `config_load_paths`
                //
                // We do this on every iteration of `overrides`, to allow
                // additional load paths to be added using `--cfg`.
                let config_load_paths = workspace.iter().flat_map(|w| {
                    partial
                        .config_load_paths
                        .iter()
                        .flatten()
                        .map(|p| p.to_path(w.root()))
                });

                let mut found = false;
                for load_path in config_load_paths {
                    debug!(
                        path = %path.display(),
                        load_path = %load_path.display(),
                        "Trying to load partial from config load path"
                    );

                    if let Some(path) = find_file_in_load_path(path, &load_path) {
                        if let Some(p) = load_partial_at_path(path)? {
                            partial = load_partial(partial, p)?;
                        }
                        found = true;
                        break;
                    }
                }

                if !found {
                    return Err(Error::MissingConfigFile(path.clone()));
                }
            }
            KeyValueOrPath::KeyValue(kv) => partial
                .assign(kv.clone())
                .map_err(|e| Error::CliConfig(e.to_string()))?,
        }
    }

    Ok(partial)
}

fn load_partial_configs_from_files(
    workspace: Option<&Workspace>,
    cwd: Option<Utf8PathBuf>,
) -> Result<Vec<PartialAppConfig>> {
    let mut partials = vec![];

    // Load `$XDG_CONFIG_HOME/jp/config.{toml,json,yaml}`.
    if let Some(user_global_config) = user_global_config_path(std::env::home_dir().as_deref())
        .and_then(|p| load_partial_at_path(p.join("config.toml")).transpose())
        .transpose()?
    {
        partials.push(user_global_config);
    }

    // Load `$WORKSPACE_ROOT/.jp/config.{toml,json,yaml}`.
    if let Some(workspace_config) = workspace
        .and_then(Workspace::storage_path)
        .and_then(|p| load_partial_at_path(p.join("config.toml")).transpose())
        .transpose()?
    {
        partials.push(workspace_config);
    }

    // Load `$CWD/.jp.{toml,json,yaml}`, recursing up the directory tree until
    // either the root of the workspace, or filesystem is reached.
    if let Some(cwd_config) = cwd
        .and_then(|cwd| {
            load_partial_at_path_recursive(
                cwd.join(".jp.toml"),
                Workspace::find_root(cwd, DEFAULT_STORAGE_DIR).as_deref(),
            )
            .transpose()
        })
        .transpose()?
    {
        partials.push(cwd_config);
    }

    // Load `$XDG_DATA_HOME/jp/<workspace_the id>config.{toml,json,yaml}`.
    if let Some(user_workspace_config) = workspace
        .and_then(Workspace::user_storage_path)
        .and_then(|p| load_partial_at_path(p.join("config.toml")).transpose())
        .transpose()?
    {
        partials.push(user_workspace_config);
    }

    Ok(partials)
}

/// Find the workspace for the current directory.
fn load_workspace(workspace: Option<&WorkspaceIdOrPath>) -> Result<Workspace> {
    let cwd = match workspace {
        None => absolute_utf8(".")?,
        Some(WorkspaceIdOrPath::Path(path)) => path.clone(),

        // TODO: Centralize this in a new `UserStorage` struct.
        Some(WorkspaceIdOrPath::Id(id)) => user_data_dir()?
            .read_dir()?
            .map(|dir| dir.ok().map(|dir| dir.path().clone()))
            .find_map(|path| {
                path.filter(|dir| {
                    dir.file_name()
                        .and_then(|v| v.to_str())
                        .is_some_and(|v| v.ends_with(&id.to_string()))
                })
            })
            .ok_or(jp_workspace::Error::MissingStorage)?
            .join("storage")
            .canonicalize()?
            .try_into()
            .map_err(FromPathBufError::into_io_error)?,
    };
    trace!(cwd = %cwd, "Finding workspace.");

    let root = Workspace::find_root(cwd, DEFAULT_STORAGE_DIR).ok_or(cmd::Error::from(format!(
        "Could not locate workspace. Use `{}` to create a new workspace.",
        "jp init".bold().yellow()
    )))?;
    trace!(root = %root, "Found workspace root.");

    let storage = root.join(DEFAULT_STORAGE_DIR);
    trace!(storage = %storage, "Initializing workspace storage.");

    let id = jp_workspace::Id::load(&storage)
        .transpose()
        .ok()
        .flatten()
        .unwrap_or_default();

    jp_id::global::set(id.to_string());
    trace!(%id, "Loaded unique workspace ID.");

    let workspace = Workspace::new_with_id(root, id)
        .persisted_at(&storage)
        .inspect(|ws| info!(workspace = %ws.root(), "Using existing workspace."))?;

    workspace.id().store(&storage)?;

    workspace.with_local_storage().map_err(Into::into)
}

fn configure_logging(verbose: u8, quiet: bool) {
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::fmt;

    let (mut level, more) = match verbose {
        0 => (LevelFilter::ERROR, 0),
        1 => (LevelFilter::WARN, 0),
        2 => (LevelFilter::INFO, 0),
        3 => (LevelFilter::DEBUG, 0),
        4 => (LevelFilter::TRACE, 0),
        5 => (LevelFilter::TRACE, 1),
        _ => (LevelFilter::TRACE, 2),
    };

    if quiet {
        level = LevelFilter::OFF;
    }

    let mut filter: Vec<_> = match more {
        0 => vec!["off".to_owned()],
        1 => vec![
            [
                "trace",
                "h2=off",
                "hyper_util=off",
                "ignore=off",
                "mio=off",
                "reqwest=off",
                "rustls=off",
                "tokio=off",
            ]
            .to_vec()
            .join(","),
        ],
        _ => vec!["trace".to_owned()],
    };

    for krate in [
        "attachment",
        "attachment_bear_note",
        "attachment_cmd_output",
        "attachment_file_content",
        "attachment_mcp_resources",
        "cli",
        "config",
        "conversation",
        "format",
        "id",
        "llm",
        "mcp",
        "openrouter",
        "query",
        "storage",
        "task",
        "term",
        "test",
        "tombmap",
        "workspace",
    ] {
        filter.push(format!("jp_{krate}={level}"));
    }

    let format = fmt::format().with_target(more > 0).compact();

    if level < LevelFilter::DEBUG {
        tracing_subscriber::fmt()
            .event_format(format)
            .without_time()
            .with_ansi(true)
            .with_writer(std::io::stderr)
            .with_env_filter(filter.join(","))
            .init();
    } else {
        tracing_subscriber::fmt()
            .event_format(format)
            .with_ansi(true)
            .with_writer(std::io::stderr)
            .with_env_filter(filter.join(","))
            .init();
    }
}

/// Get the number of worker threads to use.
pub fn worker_threads() -> Option<NonZeroUsize> {
    NonZeroUsize::new(WORKER_THREADS.load(Ordering::Relaxed))
}

/// Build an async runtime.
///
/// # Panics
///
/// Panics if called twice.
pub(crate) fn build_runtime(threads: Option<NonZeroUsize>, thread_name: &str) -> Result<Runtime> {
    let mut rt_builder = runtime::Builder::new_multi_thread();
    rt_builder.max_blocking_threads(1024);
    rt_builder.enable_all().thread_name(thread_name);

    let worker_threads = threads.unwrap_or_else(num_threads).get();
    WORKER_THREADS
        .compare_exchange(0, worker_threads, Ordering::Acquire, Ordering::Relaxed)
        .expect("double thread initialization");
    rt_builder.worker_threads(worker_threads);

    debug!(worker_threads, "Building runtime.");
    rt_builder.build().map_err(Into::into)
}

/// Returns an estimate of the number of recommended threads that JP should
/// spawn.
pub fn num_threads() -> NonZeroUsize {
    match std::thread::available_parallelism() {
        Ok(count) => count,
        Err(error) => {
            warn!(%error, "Failed to determine available parallelism for thread count, defaulting to 1.");
            std::num::NonZeroUsize::MIN
        }
    }
}

#[cfg(feature = "dhat")]
fn run_dhat() -> dhat::Profiler {
    std::process::Command::new(env!("CARGO"))
        .arg("locate-project")
        .arg("--workspace")
        .arg("--message-format=plain")
        .output()
        .ok()
        .and_then(|v| String::from_utf8(v.stdout).ok())
        .and_then(|v| PathBuf::from(v).parent().map(|v| v.join("tmp/profiling")))
        .and_then(|v| std::fs::create_dir_all(&v).ok().map(|()| v))
        .map(|v| {
            v.join(format!(
                "heap-{}.json",
                time::UtcDateTime::now().unix_timestamp()
            ))
        })
        .map_or_else(dhat::Profiler::new_heap, |v| {
            dhat::Profiler::builder().file_name(v).build()
        })
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use test_log::test;

    use super::*;

    #[test]
    fn test_cli() {
        Cli::command().debug_assert();
    }
}
