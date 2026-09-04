mod access;
mod bootstrap;
mod cmd;
mod config_pipeline;
mod ctx;
mod editor;
#[cfg(test)]
mod env_testing;
mod error;
mod format;
mod output;
mod parser;
mod render;
mod schema;
mod session;
mod shared;
mod signals;
mod timer;

use std::{
    env, fmt, fs,
    io::{self, IsTerminal as _, Write as _, stderr, stdout},
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

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::NamedUtf8TempFile;
use clap::{
    ArgAction, Parser,
    builder::{BoolValueParser, TypedValueParser as _},
};
use cmd::{Commands, workspace::target::WorkspaceTarget};
use crossterm::{style::Stylize as _, terminal};
use ctx::{Ctx, IntoPartialAppConfig};
use error::{Error, Result};
use jp_config::{
    AppConfig, PartialAppConfig,
    assignment::KvAssignment,
    fs::user_global_config_dir,
    util::{
        build, load_envs, load_partial_at_path, load_partial_at_path_recursive,
        load_partials_with_inheritance, log_load_diagnostics,
    },
};
use jp_printer::{OutputFormat, OutputWidth, Printer};
use jp_storage::backend::{
    FsStorageBackend, NullLockBackend, NullPersistBackend, ReadOnlySessionBackend,
};
use jp_term::table::{DetailRow, Details, details, details_markdown};
use jp_workspace::{
    DEFAULT_STORAGE_DIR, Workspace, roots, session_store::WorkspaceSessionStore, user_data_dir,
};
use relative_path::RelativePath;
use serde_json::Value;
use tokio::runtime::{self, Runtime};
use tracing::{debug, info, trace, warn};

use crate::{
    bootstrap::WorkspaceRequirement,
    cmd::{
        plugin::dispatch::{describe_plugin, discover_plugins},
        target::resolve_request,
    },
    config_pipeline::{ConfigPipeline, ConfigReset, ConfigResetEvents},
    timer::{LineTimer, spawn_line_timer},
};

static WORKER_THREADS: AtomicUsize = AtomicUsize::new(0);

/// The per-user data subdirectory holding one directory per known workspace
/// (`<slug>-<id>`), each with its roots registry (RFD 087).
const USER_WORKSPACES_DIR: &str = "workspace";

#[expect(dead_code)]
const DEFAULT_VARIABLE_PREFIX: &str = "JP_";

/// The prefix used to parse a CLI argument as a path instead of a string.
const PATH_STRING_PREFIX: char = '@';

// Jean Pierre's LLM Toolkit.
#[derive(Parser)]
#[command(name = "jp", author, version, long_version = env!("LONG_VERSION"), about, long_about = None)]
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

    /// Shorthand for `--cfg=NONE`: skip implicit config loading and start from
    /// program defaults.
    ///
    /// Subsequent `--cfg` values layer on top of the defaults.
    #[arg(long = "no-cfg", global = true, default_value_t = false)]
    no_config: bool,

    /// Increase verbosity of logging.
    ///
    /// Can be specified multiple times to increase verbosity.
    ///
    /// Defaults to printing "error" messages.
    /// For each increase in verbosity, the log level is set to "warn", "info",
    /// "debug", and "trace" respectively.
    #[arg(short = 'v', long, global = true, action = ArgAction::Count)]
    verbose: u8,

    /// Suppress all output, including errors.
    #[arg(short = 'q', long, global = true)]
    quiet: bool,

    /// Assume no user is available to answer prompts.
    ///
    /// Everything JP would ask about resolves the way it does when JP runs
    /// without a terminal: the workspace and conversation pickers, the
    /// lock-timeout prompt, and a third-party plugin's approval all fail, and a
    /// tool set to ask before running runs unconfirmed.
    ///
    /// A command with no answer to fall back on fails too: `jp init`, and any
    /// removal or archive it would have to confirm.
    ///
    /// Set `JP_NONINTERACTIVE=1` to apply this to every invocation in an
    /// environment, such as a script or a CI job.
    ///
    /// This does not change output formatting; use `--format` for that.
    #[arg(long, global = true, visible_alias = "non-interactive")]
    no_interactive: bool,

    /// The output format.
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
    /// actions.
    /// It is also useful to send a query to the assistant, without adding that
    /// query to the conversation history.
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
    /// Accepts the workspace targeting grammar (see `jp w use help`): a
    /// workspace ID, a path, `cwd` / `.`, or `-` to read an ID from stdin.
    /// Interactive runs can also use the session keywords (`s`, `?s`), the
    /// pickers (`?`), and free-text matching.
    ///
    /// Selects the workspace for this invocation only; it does not change the
    /// session's active workspace (that is `jp w use`).
    ///
    /// On `jp workspace use` and `jp workspace show` it names the workspace the
    /// subcommand acts on, as an alternative to their positional target.
    #[arg(short = 'w', long, global = true)]
    workspace: Option<WorkspaceTarget>,

    /// Lay output out against this many columns.
    ///
    /// Detected automatically when stdout is a terminal.
    /// Set it when stdout is a pipe that still renders for a human at a known
    /// width, such as a preview pane laid out by another program.
    /// `0` means unknown, which is the default when piped: content keeps its
    /// natural width instead of being fitted to a guess.
    #[arg(long, global = true, value_name = "COLUMNS")]
    width: Option<u16>,

    /// The format of the log output written to stderr.
    ///
    /// Defaults to "text" when stderr is a terminal, and "json" when stderr is
    /// redirected to a file or pipe.
    /// Only takes effect when tracing is written to stderr (via `-v` or
    /// `--log-file=-`).
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Auto)]
    log_format: LogFormat,

    /// Write the full tracing log to the given file.
    ///
    /// Use `-` to stream logs to stderr instead.
    /// When unset, the log is written to a temporary file, which is kept and
    /// its path printed when a run fails, or when `JP_DEBUG=1` is set and
    /// stdout is a terminal.
    #[arg(long, global = true, value_name = "PATH")]
    log_file: Option<String>,

    /// Filter log output by target and level.
    ///
    /// Accepts a comma-separated list of `target=level` pairs.
    /// Levels are one of: off, error, warn, info, debug, trace.
    /// Targets match path prefixes.
    ///
    /// Examples: --log=tool::stderr=trace Show stderr output from local tools.
    /// --log=mcp::stderr=debug Show stderr from MCP servers.
    /// --log='jp\_llm=trace,plugin=off' Trace jp\_llm internals, silence
    /// plugins.
    ///
    /// Composes with `-v`: verbosity sets the baseline for jp's own modules;
    /// `--log` adds or overrides specific targets.
    /// Passing `--log` without `-v` still enables stderr log output.
    #[allow(clippy::doc_markdown)]
    #[arg(long, global = true, value_name = "DIRECTIVE")]
    log: Option<String>,
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

/// A reserved UPPERCASE `--cfg` keyword naming a config reset point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CfgKeyword {
    /// `NONE`: reset to program defaults, and skip implicit config loading for
    /// the whole invocation.
    None,
    /// `WORKSPACE`: reset to the workspace's resolved config.
    Workspace,
}

#[derive(Debug, Clone)]
pub(crate) enum KeyValueOrPath {
    KeyValue(KvAssignment),
    Path(Utf8PathBuf),
    Keyword(CfgKeyword),
}

impl FromStr for KeyValueOrPath {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        // String prefixed with `@` is always a path.
        if let Some(s) = s.strip_prefix(PATH_STRING_PREFIX) {
            return Ok(Self::Path(Utf8PathBuf::from(s.trim())));
        }

        // Reserved UPPERCASE keywords are matched exactly, before any other
        // resolution.
        // A file literally named `NONE` or `WORKSPACE` is reachable through
        // the `@` prefix above or a path-style prefix such as `./NONE`.
        if s == "NONE" {
            return Ok(Self::Keyword(CfgKeyword::None));
        }
        if s == "WORKSPACE" {
            return Ok(Self::Keyword(CfgKeyword::Workspace));
        }

        // A JSON object is treated as a root-level config assignment that
        // merges each top-level key individually.
        if s.starts_with('{') {
            let value: serde_json::Value =
                serde_json::from_str(s).map_err(|e| Error::CliConfig(e.to_string()))?;
            if !value.is_object() {
                return Err(Error::CliConfig(
                    "--cfg JSON value must be an object".into(),
                ));
            }
            return Ok(Self::KeyValue(KvAssignment::root_json(value)));
        }

        // String without `=` is always a path.
        if !s.contains('=') {
            return Ok(Self::Path(Utf8PathBuf::from(s.trim())));
        }

        // Anything else is parsed as a key-value pair.
        s.parse().map(Self::KeyValue).map_err(Into::into)
    }
}

/// The format of the CLI output written to stdout.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum CliFormat {
    /// Automatically detect: use "text-pretty" for terminals, "text" otherwise.
    #[default]
    Auto,

    /// Plain text output.
    /// No ANSI colors or unicode decorations.
    Text,

    /// Pretty-printed text output.
    /// Includes ANSI colors, unicode decorations, and hyperlinks.
    TextPretty,

    /// Compact JSON output.
    Json,

    /// Pretty-printed multi-line JSON output.
    JsonPretty,
}

/// Whether a user is present to answer prompts.
///
/// A terminal is the evidence that someone is watching; `no_interactive` is the
/// user overriding that evidence.
/// Deliberately independent of whether output can carry ANSI escapes ([RFD
/// 048]) — a piped `jp c ls` still has a user behind it.
///
/// [RFD 048]: https://jp.computer/rfd/048
const fn interactive(no_interactive: bool, terminal: bool) -> bool {
    !no_interactive && terminal
}

/// Whether a user is present to answer a workspace or conversation picker.
///
/// Both resolve before a [`Ctx`] exists, so they cannot read
/// `Term::interactive` and take the terminal signal from stdin themselves.
pub(crate) fn stdin_interactive(no_interactive: bool) -> bool {
    interactive(no_interactive, io::stdin().is_terminal())
}

/// Whether `name` names an environment variable set to an opt-in value.
///
/// Only `1` and `true` count.
/// Every other value, including `0` and the empty string, reads as unset, so
/// exporting `NAME=0` turns the behaviour off rather than on.
fn env_opt_out(name: &str) -> bool {
    env::var(name)
        .as_deref()
        .is_ok_and(|v| v == "1" || v == "true")
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

    let mut cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            if e.kind() == clap::error::ErrorKind::DisplayHelp && is_root_help_request() {
                drop(e.print());
                print_plugin_help_section();
                return ExitCode::from(0);
            }
            // All other cases (subcommand help, version, errors): let clap handle it.
            e.exit();
        }
    };
    let is_tty = stdout().is_terminal();

    // Folded into the flag once here, so every consumer downstream reads one
    // field instead of re-reading the environment.
    cli.globals.no_interactive |= env_opt_out("JP_NONINTERACTIVE");

    let format = cli.globals.format.resolve(is_tty);

    let guard = configure_logging(
        cli.globals.verbose,
        cli.globals.quiet,
        cli.globals.log_format,
        format,
        cli.globals.log_file.as_deref(),
        cli.globals.log.as_deref(),
    );

    trace!(command = cli.command.name(), arguments = %cli, "Starting CLI run.");
    let (code, outcome, output) = match run_inner(cli, format) {
        Ok(()) => (0, RunOutcome::AsExpected, None),
        Err(error) => {
            let error = cmd::Error::from(error);
            let outcome = if error.expected {
                RunOutcome::AsExpected
            } else {
                RunOutcome::Failed
            };
            let (code, msg) = parse_error(error, format);
            (code, outcome, Some(msg))
        }
    };

    if let Some(output) = output
        && !output.trim().is_empty()
    {
        if code == 0 {
            println!("{output}");
        } else {
            eprintln!("{output}");
        }
    }

    // Read here rather than inside the policy, which stays a pure function of
    // its inputs.
    let debug_enabled = env_opt_out("JP_DEBUG");

    if should_report_trace_log(outcome, is_tty, debug_enabled)
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

/// How a run ended, as far as reporting its trace log goes.
#[derive(Debug, Clone, Copy)]
enum RunOutcome {
    /// The run did what was asked: it either succeeded, or exited non-zero to
    /// report a result.
    /// `jp conversation grep` exits 1 when it finds nothing.
    AsExpected,

    /// The run failed.
    Failed,
}

/// Whether to tell the user where the run's trace log was written.
///
/// A failed run always reports it: diagnosing the failure matters more than
/// keeping the output stream clean.
/// The exit status alone doesn't answer this, since a command can exit non-zero
/// to report a result rather than a failure.
///
/// Every other run makes the report opt-in via `JP_DEBUG`, and only when stdout
/// is a terminal.
/// A piped stdout means `jp` is a component in someone else's pipeline, and a
/// program consuming it may own the screen: an `fzf` list or preview, for
/// instance, where two uninvited lines corrupt the layout.
/// Note that stderr's own tty-ness is the wrong test: in `jp … | fzf`, stderr
/// *is* the terminal, which is exactly how the corruption happens.
///
/// Set `--log-file` to choose the path when a piped run needs to be traced;
/// nothing has to be announced when the caller picked the destination.
fn should_report_trace_log(outcome: RunOutcome, stdout_is_tty: bool, debug_enabled: bool) -> bool {
    match outcome {
        RunOutcome::Failed => true,
        RunOutcome::AsExpected => stdout_is_tty && debug_enabled,
    }
}

/// The width to lay output out against.
///
/// A `--width` given on the command line is [`OutputWidth::Declared`], with `0`
/// meaning unknown.
/// Otherwise the controlling terminal is measured when stdout is a TTY.
///
/// [`OutputWidth::Unknown`] when stdout is piped or redirected and no width was
/// given, so output keeps its full width for machine consumption rather than
/// being laid out against a guessed size.
fn detect_output_width(declared: Option<u16>) -> OutputWidth {
    if let Some(width) = declared {
        return if width > 0 {
            OutputWidth::Declared(width)
        } else {
            OutputWidth::Unknown
        };
    }

    if !stdout().is_terminal() {
        return OutputWidth::Unknown;
    }

    terminal::size()
        .ok()
        .map_or(OutputWidth::Unknown, |(cols, _)| {
            OutputWidth::Terminal(cols)
        })
}

#[expect(clippy::too_many_lines)]
fn run_inner(cli: Cli, format: OutputFormat) -> Result<()> {
    let printer =
        Printer::terminal(format).with_output_width(detect_output_width(cli.globals.width));

    // `jp workspace` runs on a dedicated pre-workspace path: selecting or
    // inspecting a workspace must work from outside every workspace —
    // including resolving to *no* workspace — so its subcommands never
    // construct a `Ctx`. Each declares what it pays for through
    // `workspace_requirement` (`ls`: registries only; `use`: resolve and
    // validate a target root; `show`: additionally loads conversation
    // indexes).
    if let Commands::Workspace(args) = cli.command {
        trace!("Resolving session identity.");
        let session = session::resolve();

        // The global `--workspace` flag names the workspace `use` and `show`
        // act on here, rather than the one the run operates from.
        let output = args
            .run(
                &printer,
                session.as_ref(),
                cli.globals.persist,
                cli.globals.workspace.as_ref(),
                cli.globals.no_interactive,
            )
            .map_err(Into::into);

        // `jp w use` and friends mutate the user-global records, so they get
        // the same hygiene pass as a workspace-consuming run.
        cleanup_workspace_session_records();

        return output;
    }

    // The per-command workspace bootstrap requirement (RFD 087): commands
    // declaring `None` run without any workspace resolution or construction,
    // so the downstream consumers that assume a root do not run.
    let requirement = cli.command.workspace_requirement();
    if requirement == WorkspaceRequirement::None {
        let Commands::Init(args) = &cli.command else {
            unreachable!("every workspace-free command has a dedicated run path");
        };

        return args
            .run(&printer, cli.globals.no_interactive)
            .map_err(Into::into);
    }

    // The pre-workspace bootstrap (RFD 087): session identity and the
    // execution context — launch cwd, selected checkout root, child cwd —
    // are resolved once, before any `Workspace` is constructed, and passed
    // explicitly to their consumers below.
    trace!("Resolving session identity.");
    let session = session::resolve();

    let exec = bootstrap::resolve(
        cli.globals.workspace.as_ref(),
        session.as_ref(),
        cli.globals.no_interactive,
    )?;
    trace!(
        root = %exec.root,
        source = ?exec.source,
        child_cwd = ?exec.child_cwd(),
        "Bootstrapped workspace selection."
    );

    let (mut workspace, fs_backend) =
        load_workspace(&exec.root, cli.globals.persist, LoadIntent::Run)?;

    // `Resolve` commands stop at a validated root; only `Load` commands pay
    // for sanitization and the conversation index.
    if requirement == WorkspaceRequirement::Load {
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

        // Populate the conversation index. This does NOT load the contents of
        // individual conversations, this is done lazily as needed.
        workspace.load_conversation_index();
    }

    // `--no-cfg` is shorthand for a leading `--cfg=NONE`, applied to config
    // resolution only. `Globals.config` stays as the user typed it: commands
    // re-consume the raw `--cfg` args (e.g. `config set` persists them), and
    // must not see a synthetic reset keyword they'd have to reject
    // ([RFD 038]).
    //
    // [RFD 038]: https://jp.computer/rfd/038
    let cfg_overrides = effective_cfg_overrides(&cli.globals);

    let (config, handles, start_new, config_reset) = resolve_config(
        &cli.command,
        || load_base_partial(fs_backend.as_deref(), exec.config_cwd().to_owned()),
        &cfg_overrides,
        &mut workspace,
        session.as_ref(),
        fs_backend.as_deref(),
        stdin_interactive(cli.globals.no_interactive),
    )?;
    let config = Arc::new(config);
    let runtime = build_runtime(cli.root.threads, "jp-worker")?;
    let mut ctx = Ctx::new(
        exec,
        workspace,
        fs_backend,
        runtime,
        cli.globals,
        config,
        session,
        printer,
    );
    ctx.config_reset = config_reset;
    let rt = ctx.handle().clone();

    // Run the requested command, racing it against the shutdown token.
    // `start_new` carries the interactive picker's "start a new conversation"
    // choice through to the query command, which honors it at lock time.
    //
    // When a graceful shutdown is requested (an unhandled or escalated
    // Ctrl-C, or SIGTERM), the command future is dropped and the run falls
    // through to the normal teardown below. Dropping the future releases
    // conversation locks and persists dirty conversations (guard-scoped
    // persistence); the teardown then drains background tasks and cleans up
    // stale files.
    let shutdown = ctx.signals.shutdown_token();
    let output = rt.block_on(async {
        tokio::select! {
            biased;
            output = cli.command.run(&mut ctx, handles, start_new) => output,
            () = shutdown.cancelled() => Err(cmd::Error::interrupted()),
        }
    });

    // The shutdown arm above drops the command future, so a conversation scope
    // that was dirty persists from its `Drop` and records any failure on the
    // workspace — with the command's own drain gone along with the future.
    // Draining here, before the `disable_persistence` check below, is what lets
    // an interrupted unsaved run still say so.
    // Commands that reported already left nothing behind: the record yields
    // each failure once.
    let output = cmd::fold_persist_failure(output, ctx.workspace.take_persist_failure());

    if let Err(error) = output.as_ref()
        && error.disable_persistence
    {
        tracing::info!(
            error = error.to_string(),
            "Error running command. Disabling workspace persistence."
        );
        ctx.workspace.disable_persistence();

        // The state this run produced is being discarded, so background work
        // derived from it (e.g. generating a title for a conversation that
        // won't survive) is wasted. Fire the soft-cancellation token now:
        // the drain below then skips its soft wait and goes straight to the
        // 2s grace pass instead of waiting up to 10s for doomed tasks.
        ctx.task_handler.cancel_token().cancel();
    }

    // Flush the printer to ensure all queued typewriter output is fully written
    // before background tasks log any errors.
    ctx.printer.flush();

    // Drain background tasks. Shows a timer line while waiting. A graceful
    // shutdown request (Ctrl-C, an interrupt earlier in the run, or SIGTERM)
    // switches to a 2s cancellation countdown; any further Ctrl-C exits the
    // process immediately via the signal router's escalation ladder.
    let drained = rt.block_on(drain_background_tasks(&mut ctx));

    // Task sync takes conversation locks of its own — the title generator
    // persists its result — so this is the run's last write, after the drain
    // above. Held separately from the drain's own error so a failing task and an
    // unsaved conversation are not reported as the same thing.
    let output = cmd::fold_persist_failure(output, ctx.workspace.take_persist_failure());
    drained.map_err(Error::Task)?;

    // Remove ephemeral conversations that are no longer needed, but protect
    // any conversation that is active in a terminal session.
    let active_ids = ctx.workspace.all_active_conversation_ids();
    ctx.workspace.remove_ephemeral_conversations(&active_ids);

    // Remove orphaned lock files and stale session mappings.
    ctx.workspace.cleanup_stale_files(ctx.fs_backend.as_deref());

    // Bootstrap cleanup (RFD 087): the user-global session → workspace
    // records are owned by this layer, not `Workspace` — they exist before
    // any workspace is selected and can reference workspaces this run never
    // touched. The source-split rules live in
    // `WorkspaceSessionStore::cleanup`.
    cleanup_workspace_session_records();

    output.map_err(Into::into)
}

/// Source-split cleanup of the user-global session → active-workspace store.
///
/// A selection stays live while its own recorded checkout still holds the
/// workspace, or while any registered checkout of that workspace ID does;
/// expanding an ID also prunes its dead registry entries opportunistically (RFD
/// 087).
///
/// Checking the recorded checkout directly is what keeps a selection of a
/// checkout the roots registry has not seen yet from being pruned in the same
/// invocation that wrote it.
fn cleanup_workspace_session_records() {
    let Ok(data_dir) = user_data_dir() else {
        return;
    };

    let workspaces_dir = data_dir.join(USER_WORKSPACES_DIR);
    WorkspaceSessionStore::at_user_data_dir(&data_dir).cleanup(&|entry| {
        entry.id().is_some_and(|id| {
            roots::is_live(&entry.root, &id, DEFAULT_STORAGE_DIR)
                || !roots::resolve_live_roots(&workspaces_dir, &id, DEFAULT_STORAGE_DIR).is_empty()
        })
    });
}

/// Drain background tasks at end of run, with interrupt-aware cancellation.
///
/// While [`TaskHandler::sync`] runs, prints a `⏱ Finishing background tasks…
/// Ns` line on stderr after a 1s delay.
/// A graceful shutdown request — a Ctrl-C during the drain, an interrupt
/// earlier in the run, or SIGTERM — switches the line to a 2s countdown and
/// signals cancellation.
/// Any Ctrl-C after shutdown has begun exits the process immediately (the
/// signal router's escalation ladder).
///
/// [`TaskHandler::sync`]: jp_task::TaskHandler::sync
async fn drain_background_tasks(
    ctx: &mut Ctx,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if ctx.task_handler.is_empty() {
        return Ok(());
    }

    let cancel = ctx.task_handler.cancel_token();
    let printer = ctx.printer.clone();
    let shutdown = ctx.signals.shutdown_token();
    let show_chrome = ctx.term.is_tty;
    // The shutdown token only cancels once; after acting on it, stop
    // selecting on it so the loop doesn't spin on a completed future.
    let mut shutdown_watched = true;

    let mut timer = if show_chrome {
        spawn_line_timer(
            printer.clone(),
            true,
            Duration::from_secs(1),
            Duration::from_millis(100),
            |secs, _status| format!("\r\x1b[K⏱ Finishing background tasks… {secs:.1}s"),
        )
    } else {
        None
    };

    let sync_fut = ctx
        .task_handler
        .sync(&mut ctx.workspace, Duration::from_secs(10));
    tokio::pin!(sync_fut);

    let result = loop {
        tokio::select! {
            biased;
            // A graceful shutdown request (pending from an interrupted
            // command, or arriving mid-drain) cancels background tasks.
            () = shutdown.cancelled(), if shutdown_watched => {
                shutdown_watched = false;
                stop_drain_timer(timer.take()).await;

                cancel.cancel();
                if show_chrome {
                    timer = spawn_line_timer(
                        printer.clone(),
                        true,
                        Duration::ZERO,
                        Duration::from_millis(100),
                        |secs, _status| {
                            format!(
                                "\r\x1b[K⏱ Cancelling background tasks… {:.1}s",
                                (2.0 - secs).max(0.0),
                            )
                        },
                    );
                }
            }
            result = &mut sync_fut => break result,
        }
    };

    stop_drain_timer(timer).await;
    result
}

async fn stop_drain_timer(timer: Option<LineTimer>) {
    if let Some(timer) = timer {
        timer.finish().await;
    }
}

/// Check if the current invocation is a root-level help request (`jp -h`).
///
/// We only inject the "Plugins:" section for root help, not for subcommand help
/// like `jp query -h`.
fn is_root_help_request() -> bool {
    let args: Vec<String> = env::args().collect();
    args.len() == 2 && (args[1] == "-h" || args[1] == "--help")
}

/// Discover plugins on `$PATH`, describe them, and print a "Plugins:" section.
fn print_plugin_help_section() {
    let plugins = discover_plugins();
    if plugins.is_empty() {
        return;
    }

    let mut descriptions: Vec<(String, String)> = Vec::new();
    for (name, binary) in &plugins {
        let desc = describe_plugin(binary);
        let display_name = desc
            .as_ref()
            .filter(|d| !d.command.is_empty())
            .map_or_else(|| name.clone(), |d| d.command.join(" "));
        let description = desc.map_or_else(|| "(no description)".into(), |d| d.description);
        descriptions.push((display_name, description));
    }

    let mut out = io::stdout().lock();
    drop(writeln!(out, "\nPlugins:"));
    for (name, desc) in &descriptions {
        drop(writeln!(out, "  {name:<16}{desc}"));
    }
}

fn parse_error(error: cmd::Error, format: OutputFormat) -> (u8, String) {
    let cmd::Error {
        code,
        message,
        mut metadata,
        ..
    } = error;

    if !format.is_json() {
        let rows: Vec<DetailRow> = metadata
            .into_iter()
            .map(|(k, v)| {
                let value = match v {
                    Value::String(s) => s,
                    v => format!("{v:#}"),
                };
                DetailRow::scalar(k, value)
            })
            .collect();

        let rows = Details::Fields(rows);
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

/// Resolve the final [`AppConfig`] and conversation handles.
///
/// Takes a loader for the base partial (the `files + env` layer) and runs the
/// full config pipeline:
///
/// 1. Build the [`ConfigPipeline`], which invokes `load_base` unless a
///    `--cfg=NONE` keyword skips implicit loading ([RFD 038]).
/// 2. Extract `default_id` for conversation resolution (loading-time only).
/// 3. Resolve conversation handles from the command's load request.
/// 4. Merge per-conversation config layer.
/// 5. Apply CLI flag overrides via [`IntoPartialAppConfig`].
/// 6. Consume `default_id` so it doesn't leak into the runtime config.
/// 7. Build the final [`AppConfig`].
///
/// Step 3 can open the conversation picker, which happens here rather than in
/// the command because the conversation it selects supplies a config layer.
/// `interactive` decides whether that prompt is available; without it, a target
/// that resolves to nothing fails with keyword help.
///
/// [RFD 038]: https://jp.computer/rfd/038
pub(crate) fn resolve_config(
    command: &Commands,
    load_base: impl FnOnce() -> Result<PartialAppConfig>,
    cfg_overrides: &[KeyValueOrPath],
    workspace: &mut Workspace,
    session: Option<&jp_workspace::session::Session>,
    fs: Option<&FsStorageBackend>,
    interactive: bool,
) -> Result<(
    AppConfig,
    Vec<jp_workspace::ConversationHandle>,
    bool,
    Option<ConfigResetEvents>,
)> {
    let pipeline = ConfigPipeline::new(cfg_overrides, Some(workspace), fs, load_base)?;

    // The effective reset point of this invocation, if any ([RFD 038]).
    let config_reset = pipeline.config_reset();

    // Extract default_id — a loading-time concern consumed here, not
    // propagated to the runtime config.
    let default_id = pipeline
        .partial_without_conversation()?
        .conversation
        .default_id
        .unwrap_or_default();

    let request = command.conversation_load_request();
    let outcome = resolve_request(
        &request,
        workspace,
        session,
        default_id,
        command.allows_new_from_picker(),
        interactive,
    )?;
    let handles = outcome.handles;

    // Phase 2: per-conversation layer.
    //
    // Skipped when this invocation contains a reset point: the reset discards
    // everything accumulated before it — including this layer — and resolving
    // the stream's current config can itself fail, which must not block the
    // reset (recovering from broken conversation config is a reset use case,
    // [RFD 038]).
    let config_handle = request.config_conversation.and_then(|idx| handles.get(idx));
    let conversation_partial = match config_handle {
        Some(handle) if config_reset.is_none() => {
            if let Err(error) = workspace.eager_load_conversation(handle) {
                tracing::warn!(error = ?error, "Failed to eager-load conversation.");
            }

            Some(
                command
                    .apply_conversation_config(workspace, PartialAppConfig::default(), None, handle)
                    .map_err(|error| Error::CliConfig(error.to_string()))?,
            )
        }
        _ => None,
    };

    let mut partial = match conversation_partial {
        Some(conversation_config) => pipeline.partial_with_conversation(conversation_config)?,
        None => pipeline.partial_without_conversation()?,
    };

    // Phase 3: CLI flag overrides.
    partial = command
        .apply_cli_config(Some(workspace), partial, None)
        .map_err(|error| Error::CliConfig(error.to_string()))?;

    // Consume default_id so it doesn't appear in the runtime config.
    partial.conversation.default_id.take();

    // Capture this invocation's final partial for the reset persistence
    // payload, before `build` consumes it.
    let post_partial = config_reset.as_ref().map(|_| partial.clone());

    log_load_diagnostics(&partial);
    let config = build(partial)?;

    // Assemble the reset point for conversation persistence ([RFD 038]): a
    // continuing conversation records the reset, and whatever this invocation
    // layered on top of it, into its event stream.
    //
    // Both layers resolve model aliases against the final config's flattened
    // alias map (built by `build` above) before capture: partials stored as
    // conversation config deltas must contain resolved model IDs (see
    // [`PartialAppConfig::resolve_model_aliases`]), because the stream's own
    // config resolution never resolves aliases.
    let config_reset = config_reset.map(|mut reset| {
        let aliases = &config.providers.llm.aliases;
        if let ConfigReset::Workspace(workspace) = &mut reset {
            workspace.resolve_model_aliases(aliases);
        }

        let mut post = post_partial.expect("captured when a reset point is present");
        post.resolve_model_aliases(aliases);

        ConfigResetEvents {
            post: Box::new(reset.state().delta(post)),
            reset,
        }
    });

    Ok((config, handles, outcome.start_new, config_reset))
}

/// The `--cfg` directive list used for config resolution.
///
/// Prepends the `NONE` keyword when `--no-cfg` is set, without mutating
/// [`Globals::config`]: the raw `--cfg` args are re-consumed by commands (e.g.
/// `config set` persisting them), which reject reset keywords ([RFD 038]).
///
/// [RFD 038]: https://jp.computer/rfd/038
fn effective_cfg_overrides(globals: &Globals) -> Vec<KeyValueOrPath> {
    let mut overrides = Vec::with_capacity(globals.config.len() + 1);
    if globals.no_config {
        overrides.push(KeyValueOrPath::Keyword(CfgKeyword::None));
    }
    overrides.extend(globals.config.iter().cloned());
    overrides
}

/// Load the base partial config from files and environment variables.
///
/// This produces the `files + inheritance + env` layer that serves as input to
/// [`ConfigPipeline`].
/// No `--cfg` args or per-conversation config.
///
/// `cwd` is the bootstrap-resolved invocation directory for the `.jp.toml`
/// chain ([`bootstrap::ExecutionContext::config_cwd`]): the launch cwd, or the
/// workspace root when JP operates on a workspace other than the launch cwd's
/// own (RFD 087).
///
/// See: <https://jp.computer/configuration>
fn load_base_partial(fs: Option<&FsStorageBackend>, cwd: Utf8PathBuf) -> Result<PartialAppConfig> {
    let partials = load_partial_configs_from_files(fs, Some(cwd))?;
    let partial = load_partials_with_inheritance(partials)?;

    load_envs(partial).map_err(|error| Error::CliConfig(error.to_string()))
}

fn load_partial_configs_from_files(
    fs: Option<&FsStorageBackend>,
    cwd: Option<Utf8PathBuf>,
) -> Result<Vec<PartialAppConfig>> {
    let config_path = RelativePath::new("config.toml");
    let mut partials = vec![];

    // Load the user-global config file (see RFD D20).
    let home = env::home_dir().and_then(|p| Utf8PathBuf::from_path_buf(p).ok());
    if let Some(user_global_config) = user_global_config_dir(home.as_deref())
        .and_then(|p| load_partial_at_path(p.join("config.toml")).transpose())
        .transpose()?
    {
        partials.push(user_global_config);
    }

    // Load `$WORKSPACE_ROOT/.jp/config.{toml,json,yaml}`.
    if let Some(workspace_config) = fs
        .map(|f| f.root_with_path(config_path))
        .and_then(|p| load_partial_at_path(p).transpose())
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

    // Load `$XDG_DATA_HOME/jp/workspace/<name>-<id>/config.{toml,json,yaml}`.
    if let Some(user_workspace_config) = fs
        .and_then(|f| f.user_storage_with_path(config_path))
        .and_then(|p| load_partial_at_path(p).transpose())
        .transpose()?
    {
        partials.push(user_workspace_config);
    }

    Ok(partials)
}

/// What a workspace load is allowed to change about the workspace it opens.
///
/// Opening a workspace is not free of side effects: user-local storage is
/// materialized on first setup, and the checkout announces itself to the roots
/// registry.
/// Both are correct for the workspace a command *runs against*, and wrong for
/// one it merely *reports on*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadIntent {
    /// The workspace the command runs against.
    ///
    /// Materializes user-local storage (creating the user-workspace directory,
    /// merging legacy siblings, importing conversations on first setup),
    /// records the checkout in the roots registry, and repairs the stored
    /// workspace ID.
    Run,

    /// A workspace the command only reads.
    ///
    /// Reuses an existing user-workspace directory read-only and writes
    /// nothing: no directory creation, no migration, no import, no registry
    /// entry, no ID write.
    /// Inspecting a workspace therefore cannot change which checkout `latest`
    /// resolves to, nor mint user-local state for a workspace the user never
    /// ran a command in.
    Inspect,
}

/// Construct the workspace at the given, bootstrap-selected checkout root.
///
/// Root selection lives in [`bootstrap::resolve`]; this only builds the storage
/// backend and [`Workspace`] on top of it.
/// `intent` decides what the load may write — see [`LoadIntent`].
///
/// When `persist` is `false` (`--no-persist`), the persist backend is swapped
/// to [`NullPersistBackend`] and the lock backend to [`NullLockBackend`] so
/// that ephemeral queries never write to disk and never block on lock
/// contention.
/// The session backend is wrapped in [`ReadOnlySessionBackend`] for the same
/// reason: the run still needs to read which conversation the session is on,
/// but must not record one that it never persisted.
fn load_workspace(
    root: &Utf8Path,
    persist: bool,
    intent: LoadIntent,
) -> Result<(Workspace, Option<Arc<FsStorageBackend>>)> {
    trace!(root = %root, ?intent, "Opening workspace.");

    // The intent picks the access mode: a command that merely reports on a
    // workspace must not create its user-local storage or repair its stored ID.
    let mut workspace = match intent {
        LoadIntent::Run => Workspace::open(root),
        LoadIntent::Inspect => Workspace::open_read_only(root),
    }
    .map_err(|error| match error {
        jp_workspace::Error::WorkspaceNotFound(_) => Error::Command(cmd::Error::from(format!(
            "Could not locate workspace. Use `{}` to create a new workspace.",
            "jp init".bold().yellow()
        ))),
        error => Error::Workspace(error),
    })?;

    let fs = workspace.fs_storage().cloned();

    if intent == LoadIntent::Run
        && let Some(dir) = fs.as_ref().and_then(|fs| fs.user_storage_path())
    {
        register_checkout(dir, root, workspace.id());
    }

    if !persist {
        let sessions = Arc::new(ReadOnlySessionBackend::new(workspace.sessions().clone()));
        workspace = workspace
            .with_persist(Arc::new(NullPersistBackend))
            .with_locker(Arc::new(NullLockBackend))
            .with_sessions(sessions);
    }
    info!(workspace = %workspace.root(), "Using existing workspace.");

    Ok((workspace, fs))
}

/// Announce a checkout in the workspace's roots registry.
///
/// Folds in any pre-registry `storage` symlink and records the checkout so `-w
/// <id>` and `jp w ls` can reach it from anywhere (RFD 087).
/// `user_dir` is the workspace's user-workspace directory, which the caller has
/// already materialized.
fn register_checkout(user_dir: &Utf8Path, root: &Utf8Path, id: &jp_workspace::Id) {
    roots::migrate_legacy_symlink(user_dir, id, DEFAULT_STORAGE_DIR);
    if let Err(error) = roots::upsert_root(user_dir, root) {
        warn!(%error, "Failed to record the checkout in the workspace roots registry.");
    }
}

/// Register a checkout without constructing a [`Workspace`].
///
/// `jp w use` records a selection that later runs resolve by ID from anywhere,
/// which only works once the checkout is in the registry.
/// Selecting a checkout no workspace-loading command has run inside yet is
/// exactly the case that needs it.
pub(crate) fn register_workspace_checkout(
    workspaces_dir: &Utf8Path,
    root: &Utf8Path,
    id: &jp_workspace::Id,
) -> Result<()> {
    let storage = root.join(DEFAULT_STORAGE_DIR);
    let fs = FsStorageBackend::new(&storage)
        .map_err(jp_workspace::Error::from)?
        .with_user_storage(workspaces_dir, root.file_name(), id.to_string())
        .map_err(jp_workspace::Error::from)?;

    if let Some(dir) = fs.user_storage_path() {
        register_checkout(dir, root, id);
    }
    Ok(())
}

const JP_CRATES: &[&str] = &[
    "attachment",
    "attachment_bear_note",
    "attachment_cmd_output",
    "attachment_file_content",
    "attachment_http_content",
    "attachment_internal",
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
    sink: Option<TraceSink>,
}

/// Where the full trace log is written.
enum TraceSink {
    /// A delete-on-drop temp file, kept only when [`TracingGuard::persist`] is
    /// called (a failed run, or `JP_DEBUG=1` with stdout on a terminal).
    Temp(NamedUtf8TempFile),
    /// A caller-chosen path (`--log-file <path>`).
    /// The file always persists.
    Path(Utf8PathBuf),
}

impl TracingGuard {
    fn persist(mut self) -> Option<Utf8PathBuf> {
        match self.sink.take()? {
            TraceSink::Temp(file) => file.keep().ok().map(|(_file, path)| path),
            TraceSink::Path(path) => Some(path),
        }
    }
}

fn configure_logging(
    verbose: u8,
    quiet: bool,
    log_format: LogFormat,
    output_format: OutputFormat,
    log_file: Option<&str>,
    log_filter: Option<&str>,
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

    // File layer: always captures full trace for post-mortem debugging.
    let mut file_filter = vec![reasonable_more.to_vec().join(",")];
    for krate in JP_CRATES {
        file_filter.push(format!("jp_{krate}=trace"));
    }
    // Plugin stderr and protocol log messages.
    file_filter.push("plugin=trace".to_owned());
    let file_env_filter = tracing_subscriber::EnvFilter::new(file_filter.join(","));

    // An explicit `--log-file <path>` pins the trace log to that path;
    // otherwise it goes to a delete-on-drop temp file that is only kept when the
    // run fails, or when `JP_DEBUG=1` is set and stdout is a terminal. (`-`
    // selects the stderr layer below, not a file path.)
    let (file_writer, sink) = match log_file {
        Some(path) if path != "-" => {
            let file = fs::File::create(path).ok()?;
            (file, TraceSink::Path(Utf8PathBuf::from(path)))
        }
        _ => {
            let file = NamedUtf8TempFile::new().ok()?;
            let writer = file.as_file().try_clone().ok()?;
            (writer, TraceSink::Temp(file))
        }
    };

    let file_layer = fmt::layer()
        .json()
        .with_ansi(false)
        .with_writer(Mutex::new(file_writer))
        .with_filter(file_env_filter);

    let registry = tracing_subscriber::registry().with(file_layer);

    // Stderr layer: only enabled when the user asks for it.
    //
    // By default, tracing goes only to the log file. Stderr is reserved for
    // chrome (progress indicators, tool headers). This keeps `2> chrome.log`
    // clean of tracing noise.
    //
    // `-v` implies stderr output (the user wants to see logs).
    // `--log-file=-` is an explicit opt-in.
    // `--log` is an explicit opt-in (otherwise the flag would silently do
    // nothing without also passing `-v`).
    // `--quiet` suppresses stderr output regardless.
    let log_to_stderr =
        !quiet && (verbose > 0 || log_filter.is_some() || log_file.is_some_and(|f| f == "-"));

    if log_to_stderr {
        let mut term_filter: Vec<_> = match more {
            0 => vec!["off".to_owned()],
            1 => vec![reasonable_more.to_vec().join(",")],
            _ => vec!["trace".to_owned()],
        };
        for krate in JP_CRATES {
            term_filter.push(format!("jp_{krate}={level}"));
        }
        // Plugin stderr and protocol log messages.
        term_filter.push(format!("plugin={level}"));
        // User-supplied directives come last so they override the baseline
        // for any targets they mention.
        if let Some(directive) = log_filter {
            term_filter.push(directive.to_owned());
        }
        let term_env_filter = tracing_subscriber::EnvFilter::new(term_filter.join(","));

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
    } else {
        // No stderr layer. Logs go only to the file.
        registry.init();
    }

    Some(TracingGuard { sink: Some(sink) })
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
