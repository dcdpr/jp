mod cmd;
mod config_pipeline;
mod ctx;
mod editor;
mod error;
mod format;
mod output;
mod parser;
mod schema;
mod session;
mod signals;

use std::{
    env, fmt,
    io::{self, IsTerminal as _, stderr, stdout},
    num::{self, NonZeroUsize},
    process::ExitCode,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use camino::{FromPathBufError, Utf8PathBuf, absolute_utf8};
use camino_tempfile::NamedUtf8TempFile;
use clap::{
    ArgAction, Parser,
    builder::{BoolValueParser, TypedValueParser as _},
};
use cmd::Commands;
use comfy_table::{Cell, CellAlignment, Row};
use crossterm::style::Stylize as _;
use ctx::{Ctx, IntoPartialAppConfig};
use error::{Error, Result};
use jp_config::{
    PartialAppConfig,
    assignment::KvAssignment,
    fs::user_global_config_path,
    util::{
        build, load_envs, load_partial_at_path, load_partial_at_path_recursive,
        load_partials_with_inheritance,
    },
};
use jp_printer::{OutputFormat, Printer};
use jp_term::table::{details, details_markdown};
use jp_workspace::{Workspace, user_data_dir};
use serde_json::Value;
use tokio::runtime::{self, Runtime};
use tracing::{debug, info, trace, warn};

use crate::{cmd::target::resolve_request, config_pipeline::ConfigPipeline};

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

    /// The output format.
    ///
    /// - `auto`: Automatically detect based on terminal.
    /// - `text`: Plain text, no ANSI colors or unicode decorations.
    /// - `text-pretty`: Rich text with ANSI colors and hyperlinks.
    /// - `json`: Compact JSON output.
    /// - `json-pretty`: Pretty-printed JSON output.
    #[arg(
        short = 'F',
        long = "format",
        global = true,
        value_enum,
        default_value_t = CliFormat::Auto,
    )]
    format: CliFormat,

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

    /// The format of the log output written to stderr.
    ///
    /// Defaults to "text" when stderr is a terminal, and "json" when stderr
    /// is redirected to a file or pipe.
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Auto)]
    log_format: LogFormat,
}

/// The format used for log output on stderr.
#[derive(Debug, Default, Clone, Copy, clap::ValueEnum)]
pub(crate) enum LogFormat {
    /// Automatically detect: use "text" for terminals, "json" otherwise.
    #[default]
    Auto,

    /// Human-readable compact text format with ANSI colors.
    Text,

    /// Machine-readable JSON format, one object per line.
    Json,
}

#[derive(Debug, Clone)]
pub(crate) enum KeyValueOrPath {
    KeyValue(KvAssignment),
    Path(Utf8PathBuf),
}

impl FromStr for KeyValueOrPath {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        // String prefixed with `@` is always a path.
        if let Some(s) = s.strip_prefix(PATH_STRING_PREFIX) {
            return Ok(Self::Path(Utf8PathBuf::from(s.trim())));
        }

        // String without `=` is always a path.
        if !s.contains('=') {
            return Ok(Self::Path(Utf8PathBuf::from(s.trim())));
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
        if Utf8PathBuf::from(s).exists() {
            return Ok(Self::Path(Utf8PathBuf::from(s)));
        }

        Ok(Self::Id(jp_workspace::Id::from_str(s)?))
    }
}

/// The format of the CLI output written to stdout.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum CliFormat {
    /// Automatically detect: use "text-pretty" for terminals,
    /// "text" otherwise.
    #[default]
    Auto,

    /// Plain text output. No ANSI colors or unicode decorations.
    Text,

    /// Pretty-printed text output. Includes ANSI colors, unicode
    /// decorations, and hyperlinks.
    TextPretty,

    /// Compact JSON output.
    Json,

    /// Pretty-printed multi-line JSON output.
    JsonPretty,
}

impl CliFormat {
    /// Resolve `Auto` based on TTY detection, returning the concrete
    /// [`OutputFormat`].
    #[must_use]
    pub(crate) fn resolve(self, is_tty: bool) -> OutputFormat {
        match self {
            Self::Auto if is_tty => OutputFormat::TextPretty,
            Self::Auto | Self::Text => OutputFormat::Text,
            Self::TextPretty => OutputFormat::TextPretty,
            Self::Json => OutputFormat::Json,
            Self::JsonPretty => OutputFormat::JsonPretty,
        }
    }
}

impl fmt::Display for Cli {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entry(&"config", &self.globals.config)
            .entry(&"verbose", &self.globals.verbose)
            .entry(&"quiet", &self.globals.quiet)
            .finish()
    }
}

#[expect(clippy::print_stdout, clippy::print_stderr)]
pub fn run() -> ExitCode {
    #[cfg(feature = "dhat")]
    let _profiler = run_dhat();

    let cli = Cli::parse();
    let is_tty = stdout().is_terminal();

    let format = cli.globals.format.resolve(is_tty);

    let guard = configure_logging(
        cli.globals.verbose,
        cli.globals.quiet,
        cli.globals.log_format,
        format,
    );

    trace!(command = cli.command.name(), arguments = %cli, "Starting CLI run.");
    let (code, output) = match run_inner(cli, format) {
        Ok(()) => (0, None),
        Err(error) => {
            let (code, msg) = parse_error(error.into(), format);
            (code, Some(msg))
        }
    };

    if let Some(output) = output {
        if code == 0 {
            println!("{output}");
        } else {
            eprintln!("{output}");
        }
    }

    if (code != 0
        || env::var("JP_DEBUG")
            .as_deref()
            .is_ok_and(|v| v == "1" || v == "true"))
        && let Some(path) = guard.and_then(TracingGuard::persist)
    {
        if format.is_json() {
            let msg = serde_json::json!({ "trace_log": path.as_str() });
            eprintln!("{msg}");
        } else {
            eprintln!("\nFull trace log written to: {path}");
        }
    }

    #[cfg(feature = "dhat")]
    eprintln!("You can view the heap profile at https://profiler.firefox.com");

    ExitCode::from(code)
}

fn run_inner(cli: Cli, format: OutputFormat) -> Result<()> {
    let printer = Printer::terminal(format);

    // `jp init` is a special case that doesn't need the full startup pipeline.
    if let Commands::Init(args) = &cli.command {
        return args.run(&printer).map_err(Into::into);
    }

    let mut workspace = load_workspace(cli.globals.workspace.as_ref())?;
    if !cli.globals.persist {
        workspace.disable_persistence();
    }

    trace!("Sanitizing workspace.");
    let report = workspace.sanitize()?;
    if report.has_repairs() {
        for trashed in &report.trashed {
            warn!(
                dirname = trashed.dirname,
                error = %trashed.error,
                "Trashed corrupt conversation"
            );
        }
    }

    trace!("Resolving session identity.");
    let session = session::resolve();

    // Populate the conversation index. This does NOT load the contents of
    // individual conversations, this is done lazily as needed.
    workspace.load_conversation_index();

    // Config Loading Phase 1: Load static sources (files + env + --cfg) once.
    let base = load_base_partial(&workspace)?;
    let pipeline = ConfigPipeline::new(base, &cli.globals.config, Some(&workspace))?;

    // Extract default_id for conversation resolution. This builds a temporary
    // partial (base + --cfg + command-CLI) without the per-conversation layer.
    let default_id = {
        let mut cfg = pipeline.partial_without_conversation()?;
        cfg = cli
            .command
            .apply_cli_config(Some(&workspace), cfg, None)
            .map_err(|error| Error::CliConfig(error.to_string()))?;

        cfg.conversation.default_id.take().unwrap_or_default()
    };

    let request = cli.command.conversation_load_request();
    let handles = resolve_request(&request, &workspace, session.as_ref(), default_id)?;

    // Config Loading Phase 2: Build final config with per-conversation layer.
    let config_handle = request.config_conversation.and_then(|idx| handles.get(idx));
    if let Some(handle) = config_handle
        && let Err(error) = workspace.eager_load_conversation(handle)
    {
        tracing::warn!(error = ?error, "Failed to eager-load conversation.");
    }

    let conversation_partial = config_handle
        .map(|handle| {
            cli.command
                .apply_conversation_config(&workspace, PartialAppConfig::default(), None, handle)
                .map_err(|error| Error::CliConfig(error.to_string()))
        })
        .transpose()?;

    let mut partial = match conversation_partial {
        Some(conversation_config) => pipeline.partial_with_conversation(conversation_config)?,
        None => pipeline.partial_without_conversation()?,
    };

    partial = cli
        .command
        .apply_cli_config(Some(&workspace), partial, None)
        .map_err(|error| Error::CliConfig(error.to_string()))?;

    let config = Arc::new(build(partial)?);
    let runtime = build_runtime(cli.root.threads, "jp-worker")?;
    let mut ctx = Ctx::new(workspace, runtime, cli.globals, config, session, printer);
    let rt = ctx.handle().clone();

    // Run the requested command.
    let output = rt.block_on(cli.command.run(&mut ctx, handles));

    if let Err(error) = output.as_ref()
        && error.disable_persistence
    {
        tracing::info!(
            error = error.to_string(),
            "Error running command. Disabling workspace persistence."
        );
        ctx.workspace.disable_persistence();
    }

    // Flush the printer to ensure all queued typewriter output is fully written
    // before background tasks log any errors.
    ctx.printer.flush();

    // Wait for background tasks to complete and sync their results to the
    // workspace.
    rt.block_on(
        ctx.task_handler
            .sync(&mut ctx.workspace, Duration::from_secs(10)),
    )
    .map_err(Error::Task)?;

    // Remove ephemeral conversations that are no longer needed.
    ctx.workspace.remove_ephemeral_conversations(&[]);

    // Remove orphaned lock files and stale session mappings.
    ctx.workspace.cleanup_stale_files();

    output.map_err(Into::into)
}

fn parse_error(error: cmd::Error, format: OutputFormat) -> (u8, String) {
    let cmd::Error {
        code,
        message,
        mut metadata,
        ..
    } = error;

    if !format.is_json() {
        let rows: Vec<Row> = metadata
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
            .collect();

        let rendered = if format.is_pretty() {
            details(message.as_deref(), rows)
        } else {
            details_markdown(message.as_deref(), rows)
        };

        return (code.into(), rendered);
    }

    let error = serde_json::json!({
        "message": message,
        "metadata": metadata,
        "code": code,
    });

    let error = if format.is_json_pretty() {
        serde_json::to_string_pretty(&error)
    } else {
        serde_json::to_string(&error)
    }
    .unwrap_or_else(|err| {
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

/// Load the base partial config from files and environment variables.
///
/// This produces the `files + inheritance + env` layer that serves as input to
/// [`ConfigPipeline`]. No `--cfg` args or per-conversation config.
///
/// See: <https://jp.computer/configuration>
fn load_base_partial(workspace: &Workspace) -> Result<PartialAppConfig> {
    let partials = load_partial_configs_from_files(Some(workspace), absolute_utf8(".").ok())?;
    let partial = load_partials_with_inheritance(partials)?;

    load_envs(partial).map_err(|error| Error::CliConfig(error.to_string()))
}

fn load_partial_configs_from_files(
    workspace: Option<&Workspace>,
    cwd: Option<Utf8PathBuf>,
) -> Result<Vec<PartialAppConfig>> {
    let mut partials = vec![];

    // Load `$XDG_CONFIG_HOME/jp/config.{toml,json,yaml}`.
    let home = env::home_dir().and_then(|p| Utf8PathBuf::from_path_buf(p).ok());
    if let Some(user_global_config) = user_global_config_path(home.as_deref())
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

    trace!(%id, "Loaded unique workspace ID.");

    let workspace = Workspace::new_with_id(root, id)
        .persisted_at(&storage)
        .inspect(|ws| info!(workspace = %ws.root(), "Using existing workspace."))?;

    workspace.id().store(&storage)?;

    workspace.with_local_storage().map_err(Into::into)
}

const JP_CRATES: &[&str] = &[
    "attachment",
    "attachment_bear_note",
    "attachment_cmd_output",
    "attachment_file_content",
    "attachment_http_content",
    "attachment_mcp_resources",
    "cli",
    "config",
    "conversation",
    "format",
    "id",
    "inquire",
    "llm",
    "macro",
    "mcp",
    "md",
    "openrouter",
    "printer",
    "serde",
    "storage",
    "task",
    "term",
    "test",
    "tombmap",
    "tool",
    "workspace",
];

pub struct TracingGuard {
    file: Option<NamedUtf8TempFile>,
}

impl TracingGuard {
    fn persist(mut self) -> Option<Utf8PathBuf> {
        self.file
            .take()
            .and_then(|file| file.keep().ok().map(|(_file, path)| path))
    }
}

fn configure_logging(
    verbose: u8,
    quiet: bool,
    log_format: LogFormat,
    output_format: OutputFormat,
) -> Option<TracingGuard> {
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::{fmt, prelude::*};

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

    let reasonable_more = [
        "trace",
        "h2=off",
        "hyper_util=off",
        "ignore=off",
        "mio=off",
        "reqwest=off",
        "rustls=off",
        "tokio=off",
    ];

    let mut term_filter: Vec<_> = match more {
        0 => vec!["off".to_owned()],
        1 => vec![reasonable_more.to_vec().join(",")],
        _ => vec!["trace".to_owned()],
    };

    for krate in JP_CRATES {
        term_filter.push(format!("jp_{krate}={level}"));
    }

    let term_env_filter = tracing_subscriber::EnvFilter::new(term_filter.join(","));

    let mut file_filter = vec![reasonable_more.to_vec().join(",")];

    for krate in JP_CRATES {
        file_filter.push(format!("jp_{krate}=trace"));
    }

    let file_env_filter = tracing_subscriber::EnvFilter::new(file_filter.join(","));

    let file = NamedUtf8TempFile::new().ok()?;
    let file_writer = file.as_file().try_clone().ok()?;

    let file_layer = fmt::layer()
        .json()
        .with_ansi(false)
        .with_writer(Mutex::new(file_writer))
        .with_filter(file_env_filter);

    let registry = tracing_subscriber::registry().with(file_layer);

    let use_json = match log_format {
        LogFormat::Json => true,
        LogFormat::Text => false,
        // When stdout is JSON, force stderr logging to JSON too so
        // consumers can parse both streams reliably.
        LogFormat::Auto => output_format.is_json() || !stderr().is_terminal(),
    };

    if use_json {
        let layer = fmt::layer().json().with_ansi(false).with_writer(io::stderr);

        let layer = if level < LevelFilter::DEBUG {
            layer.without_time().boxed()
        } else {
            layer.boxed()
        };

        registry.with(layer.with_filter(term_env_filter)).init();
    } else {
        let format = fmt::format().with_target(more > 0).compact();
        let layer = fmt::layer()
            .event_format(format)
            .with_ansi(true)
            .with_writer(io::stderr);

        if level < LevelFilter::DEBUG {
            registry
                .with(layer.without_time().with_filter(term_env_filter))
                .init();
        } else {
            registry.with(layer.with_filter(term_env_filter)).init();
        }
    }

    Some(TracingGuard { file: Some(file) })
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
    match thread::available_parallelism() {
        Ok(count) => count,
        Err(error) => {
            warn!(%error, "Failed to determine available parallelism for thread count, defaulting to 1.");
            num::NonZeroUsize::MIN
        }
    }
}

#[cfg(feature = "dhat")]
fn run_dhat() -> dhat::Profiler {
    use std::path::PathBuf;

    std::process::Command::new(env!("CARGO"))
        .arg("locate-project")
        .arg("--workspace")
        .arg("--message-format=plain")
        .output()
        .ok()
        .and_then(|v| String::from_utf8(v.stdout).ok())
        .and_then(|v| PathBuf::from(v).parent().map(|v| v.join("tmp/profiling")))
        .and_then(|v| std::fs::create_dir_all(&v).ok().map(|()| v))
        .map(|v| v.join(format!("heap-{}.json", chrono::Utc::now().timestamp())))
        .map_or_else(dhat::Profiler::new_heap, |v| {
            dhat::Profiler::builder().file_name(v).build()
        })
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
