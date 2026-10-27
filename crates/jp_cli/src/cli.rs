//! The `jp` command line: its root command, the global options, and parsing.
//!
//! [`Cli::try_parse_from`] is the entry point.
//! It builds the root [`Command`], lets clap match the arguments against it,
//! records how each option's value was written ([`Spelling`]), and reads the
//! matches back into a [`Cli`].
//! Each subcommand declares and reads its own arguments; see [`Commands`].
//!
//! A flag whose value is optional is declared through [`flag`], which decides
//! whether a word after it is its value or the start of the free text.
//!
//! [`Commands`]: crate::cmd::Commands

pub(crate) mod flag;
mod spelling;

use std::{ffi::OsString, num::NonZeroUsize, str::FromStr as _};

use clap::{
    Arg, ArgAction, ArgMatches, Command,
    builder::{BoolValueParser, TypedValueParser as _},
    value_parser,
};

use self::flag::Parsed;
pub(crate) use self::spelling::Spelling;
use crate::{
    CliFormat, KeyValueOrPath, LogFormat,
    cmd::{Commands, workspace::target::WorkspaceTarget},
};

/// The heading the options accepted by every command are listed under.
const GLOBAL_HEADING: &str = "Global Options";

/// A command's arguments: declared on its clap [`Command`], and read back from
/// what clap matched.
///
/// A command with subcommands declares them in [`augment`] and reads the one
/// given in [`from_matches`].
///
/// [`augment`]: Self::augment
/// [`from_matches`]: Self::from_matches
pub(crate) trait CommandArgs: Sized {
    /// Declare the arguments, and any subcommands, on `cmd`.
    fn augment(cmd: Command) -> Command;

    /// Read the arguments back from `matches`.
    ///
    /// # Errors
    ///
    /// Returns an error when a value cannot be used, or no subcommand was given
    /// where one is required.
    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error>;
}

/// The subcommand `matches` selected, or the error for a missing one.
///
/// # Errors
///
/// Returns an error when no subcommand was given.
pub(crate) fn subcommand_of(matches: &ArgMatches) -> Result<(&str, &ArgMatches), clap::Error> {
    matches.subcommand().ok_or_else(|| {
        clap::Error::raw(
            clap::error::ErrorKind::MissingSubcommand,
            "A subcommand is required but one was not provided.\n",
        )
    })
}

/// The error for a subcommand name a command declares but does not read.
pub(crate) fn unread_subcommand(name: &str) -> clap::Error {
    clap::Error::raw(
        clap::error::ErrorKind::InvalidSubcommand,
        format!("`{name}` is a built-in command `jp` cannot read\n"),
    )
}

/// A parsed `jp` invocation.
pub(crate) struct Cli {
    pub(crate) globals: Globals,
    pub(crate) root: RootOpts,
    pub(crate) command: Commands<Parsed>,
}

impl Cli {
    /// The root `jp` command, with every subcommand mounted on it.
    pub(crate) fn command() -> Command {
        let cmd = Command::new("jp")
            .author(env!("CARGO_PKG_AUTHORS"))
            .version(env!("CARGO_PKG_VERSION"))
            .long_version(env!("LONG_VERSION"))
            .about(env!("CARGO_PKG_DESCRIPTION"))
            .subcommand_required(true)
            .arg_required_else_help(true);

        let cmd = Globals::augment(cmd);
        let cmd = RootOpts::augment(cmd);
        Commands::augment(cmd)
    }

    /// Parse `argv`, whose first item is the program name.
    ///
    /// # Errors
    ///
    /// Returns clap's error for arguments that do not match the command line,
    /// including the help and version requests clap reports as errors.
    pub(crate) fn try_parse_from<I, T>(argv: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let argv: Vec<OsString> = argv.into_iter().map(Into::into).collect();

        let mut cmd = Self::command();
        let matches = cmd.try_get_matches_from_mut(&argv)?;
        cmd.build();
        let spelling = Spelling::of(&cmd, &matches, &argv);

        // An error from reading the matches back carries no usage line of its
        // own; formatting it against the subcommand gives it the same chrome as
        // an error clap raised while matching.
        Self::from_matches(&matches, &spelling)
            .map_err(|error| error.format(matched_command(&mut cmd, &matches)))
    }

    fn from_matches(matches: &ArgMatches, spelling: &Spelling) -> Result<Self, clap::Error> {
        Ok(Self {
            globals: Globals::from_matches(matches),
            root: RootOpts::from_matches(matches),
            command: Commands::from_matches(matches, spelling)?,
        })
    }
}

/// The innermost declared subcommand of `cmd` that `matches` selected.
fn matched_command<'a>(cmd: &'a mut Command, matches: &ArgMatches) -> &'a mut Command {
    let mut cmd = cmd;
    let mut matches = matches;
    while let Some((name, sub)) = matches.subcommand() {
        // A plugin's command is not declared, so its parent is the innermost.
        if cmd.find_subcommand(name).is_none() {
            break;
        }
        cmd = cmd
            .find_subcommand_mut(name)
            .expect("found by the same name above");
        matches = sub;
    }

    cmd
}

/// The options accepted only by `jp` itself, before any subcommand.
pub(crate) struct RootOpts {
    /// The number of worker threads, or one per available core when unset.
    pub(crate) threads: Option<NonZeroUsize>,
}

impl RootOpts {
    fn augment(cmd: Command) -> Command {
        cmd.arg(
            Arg::new("threads")
                .short('t')
                .long("threads")
                .value_name("THREADS")
                .value_parser(value_parser!(NonZeroUsize))
                .help_heading(GLOBAL_HEADING)
                .help(
                    "Number of threads to use for processing (default is number of available \
                     cores)",
                ),
        )
    }

    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            threads: matches.get_one("threads").copied(),
        }
    }
}

/// The options every `jp` command accepts, before or after its name.
#[derive(Debug, Default)]
pub(crate) struct Globals {
    /// `--cfg` values, in the order given.
    pub(crate) config: Vec<KeyValueOrPath>,

    /// `--no-cfg`.
    pub(crate) no_config: bool,

    /// How many times `-v` was given.
    pub(crate) verbose: u8,

    /// `--quiet`.
    pub(crate) quiet: bool,

    /// `--no-interactive`.
    pub(crate) no_interactive: bool,

    /// `--format`.
    pub(crate) format: CliFormat,

    /// Whether to persist changes; `--no-persist` turns it off.
    pub(crate) persist: bool,

    /// `--workspace`.
    pub(crate) workspace: Option<WorkspaceTarget>,

    /// `--width`.
    pub(crate) width: Option<u16>,

    /// `--log-format`.
    pub(crate) log_format: LogFormat,

    /// `--log-file`.
    pub(crate) log_file: Option<String>,

    /// `--log`.
    pub(crate) log: Option<String>,
}

impl Globals {
    #[expect(clippy::too_many_lines)]
    fn augment(cmd: Command) -> Command {
        let global = |arg: Arg| arg.global(true).help_heading(GLOBAL_HEADING);

        cmd.arg(global(
            Arg::new("config")
                .short('c')
                .long("cfg")
                .action(ArgAction::Append)
                .value_name("KEY=VALUE")
                .value_parser(KeyValueOrPath::from_str)
                .help("Override a configuration value for the duration of the command"),
        ))
        .arg(global(
            Arg::new("no_config")
                .long("no-cfg")
                .action(ArgAction::SetTrue)
                .help(
                    "Shorthand for `--cfg=NONE`: skip implicit config loading and start from \
                     program defaults",
                )
                .long_help(
                    "Shorthand for `--cfg=NONE`: skip implicit config loading and start from \
                     program defaults.\n\nSubsequent `--cfg` values layer on top of the defaults.",
                ),
        ))
        .arg(global(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .action(ArgAction::Count)
                .help("Increase verbosity of logging")
                .long_help(
                    "Increase verbosity of logging.\n\nCan be specified multiple times to \
                     increase verbosity.\n\nDefaults to printing \"error\" messages. For each \
                     increase in verbosity, the log level is set to \"warn\", \"info\", \
                     \"debug\", and \"trace\" respectively.",
                ),
        ))
        .arg(global(
            Arg::new("quiet")
                .short('q')
                .long("quiet")
                .action(ArgAction::SetTrue)
                .help("Suppress chrome: progress indicators, status lines, and tool call headers")
                .long_help(
                    "Suppress chrome: progress indicators, status lines, and tool call \
                     headers.\n\nCommand output and the assistant's response still go to stdout, \
                     and a failing run still reports its error. Redirect them (`>/dev/null`, \
                     `2>/dev/null`) to drop those as well.\n\nTracing written to stderr by `-v` \
                     or `--log-file=-` is suppressed too. The trace log file still records the \
                     run in full.",
                ),
        ))
        .arg(global(
            Arg::new("no_interactive")
                .long("no-interactive")
                .visible_alias("non-interactive")
                .action(ArgAction::SetTrue)
                .help("Assume no user is available to answer prompts")
                .long_help(
                    "Assume no user is available to answer prompts.\n\nEverything JP would ask \
                     about resolves the way it does when JP runs without a terminal: the \
                     workspace and conversation pickers, the lock-timeout prompt, and a \
                     third-party plugin's approval all fail, and a tool set to ask before running \
                     runs unconfirmed.\n\nA command with no answer to fall back on fails too: `jp \
                     init`, and any removal or archive it would have to confirm.\n\nSet \
                     `JP_NONINTERACTIVE=1` to apply this to every invocation in an environment, \
                     such as a script or a CI job.\n\nThis does not change output formatting; use \
                     `--format` for that.",
                ),
        ))
        .arg(global(
            Arg::new("format")
                .short('F')
                .long("format")
                .value_name("FORMAT")
                .value_parser(value_parser!(CliFormat))
                .default_value("auto")
                .help("The output format"),
        ))
        .arg(global(
            Arg::new("persist")
                .short('!')
                .long("no-persist")
                .visible_short_alias('P')
                .action(ArgAction::SetTrue)
                .default_value("false")
                // The flag turns persistence off, so its value is inverted:
                // present reads as `false`, absent as `true`.
                .value_parser(BoolValueParser::new().map(|v| !v))
                .help("Disable persistence for the duration of the command")
                .long_help(
                    "Disable persistence for the duration of the command.\n\nModified state is \
                     persisted to disk by default. Disabling it helps debug certain actions, and \
                     lets you send a query to the assistant without adding it to the conversation \
                     history.",
                ),
        ))
        .arg(global(
            Arg::new("workspace")
                .short('w')
                .long("workspace")
                .value_name("WORKSPACE")
                .value_parser(value_parser!(WorkspaceTarget))
                .help("The workspace to use for the command")
                .long_help(
                    "The workspace to use for the command.\n\nAccepts the workspace targeting \
                     grammar (see `jp w use help`): a workspace ID, a path, `cwd` / `.`, or `-` \
                     to read an ID from stdin. Interactive runs can also use the session keywords \
                     (`s`, `?s`), the pickers (`?`), and free-text matching.\n\nSelects the \
                     workspace for this invocation only; it does not change the session's active \
                     workspace (that is `jp w use`).\n\nOn `jp workspace use` and `jp workspace \
                     show` it names the workspace the subcommand acts on, as an alternative to \
                     their positional target.",
                ),
        ))
        .arg(global(
            Arg::new("width")
                .long("width")
                .value_name("COLUMNS")
                .value_parser(value_parser!(u16))
                .help("Lay output out against this many columns")
                .long_help(
                    "Lay output out against this many columns.\n\nDetected automatically when \
                     stdout is a terminal. Set it when stdout is a pipe that still renders for a \
                     human at a known width, such as a preview pane laid out by another program. \
                     `0` means unknown, which is the default when piped: content keeps its \
                     natural width instead of being fitted to a guess.",
                ),
        ))
        .arg(global(
            Arg::new("log_format")
                .long("log-format")
                .value_name("LOG_FORMAT")
                .value_parser(value_parser!(LogFormat))
                .default_value("auto")
                .help("The format of the log output written to stderr")
                .long_help(
                    "The format of the log output written to stderr.\n\nDefaults to \"text\" when \
                     stderr is a terminal, and \"json\" when stderr is redirected to a file or \
                     pipe. Only takes effect when tracing is written to stderr (via `-v` or \
                     `--log-file=-`).",
                ),
        ))
        .arg(global(
            Arg::new("log_file")
                .long("log-file")
                .value_name("PATH")
                .help("Write the full tracing log to the given file")
                .long_help(
                    "Write the full tracing log to the given file.\n\nUse `-` to stream logs to \
                     stderr instead. When unset, the log is written to a temporary file. \
                     `JP_DEBUG=1` announces its path before execution and keeps it afterward. \
                     Runs lasting at least five minutes repeat the path at completion. A failed \
                     run without `JP_DEBUG` reports the retained path at completion.",
                ),
        ))
        .arg(global(
            Arg::new("log")
                .long("log")
                .value_name("DIRECTIVE")
                .help("Filter log output by target and level")
                .long_help(
                    "Filter log output by target and level.\n\nAccepts a comma-separated list of \
                     `target=level` pairs. Levels are one of: off, error, warn, info, debug, \
                     trace. Targets match path prefixes.\n\nExamples: --log=tool::stderr=trace \
                     Show stderr output from local tools. --log=mcp::stderr=debug Show stderr \
                     from MCP servers. --log='jp_llm=trace,plugin=off' Trace jp_llm internals, \
                     silence plugins.\n\nComposes with `-v`: verbosity sets the baseline for jp's \
                     own modules; `--log` adds or overrides specific targets. Passing `--log` \
                     without `-v` still enables stderr log output.",
                ),
        ))
    }

    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            config: matches
                .get_many("config")
                .map(|values| values.cloned().collect())
                .unwrap_or_default(),
            no_config: matches.get_flag("no_config"),
            verbose: matches.get_count("verbose"),
            quiet: matches.get_flag("quiet"),
            no_interactive: matches.get_flag("no_interactive"),
            format: matches.get_one("format").copied().unwrap_or_default(),
            persist: matches.get_one("persist").copied().unwrap_or(true),
            workspace: matches.get_one("workspace").cloned(),
            width: matches.get_one("width").copied(),
            log_format: matches.get_one("log_format").copied().unwrap_or_default(),
            log_file: matches.get_one("log_file").cloned(),
            log: matches.get_one("log").cloned(),
        }
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
