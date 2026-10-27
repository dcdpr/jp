//! The `query` arguments clap cannot derive on its own.
//!
//! Every type here exists because its flag depends on *where* a value sat on
//! the command line, which a derived `clap::Args` cannot see.
//! [`QueryInput`] has to know whether the word after `--quote` was its value or
//! the start of the query, and which words the flags in front of the query gave
//! back to it.
//! [`ToolDirectives`] has to know the order `--tool` and `--no-tool` were
//! written in.
//! It, [`ToolUse`], [`ForkFlag`], and [`ExpiryFlag`] each have to know whether
//! their value sits directly before the query, where it may instead be the
//! query's first word.
//! Everything else on `Query` is derived as usual.

use std::{fmt, sync::Arc};

use clap::{ArgAction, error::ErrorKind};
use jp_inquire::prompt::PromptBackend;

use crate::{
    cmd::{compact_flag::CompactFlag, conversation_id::FlagIds},
    parser::{GivesBack, given_back, query_indices, split_list},
};

/// The query text and the `--quote` seed.
///
/// The two are parsed together because `--quote` accepts its value either
/// attached (`--quote=false`) or as the word right after the flag (`--quote
/// false`), and in the second form that word arrives as query text.
#[derive(Debug, Default)]
pub(crate) struct QueryInput {
    /// The query words, in the order they were given.
    pub(super) query: Option<Vec<String>>,

    /// `Some(true)` prefixes the quoted message with ` >  `, `Some(false)`
    /// seeds it verbatim, `None` means `--quote` was not given.
    pub(super) quote: Option<bool>,

    /// The clap index of each word in `query`, in the same order.
    pub(super) indices: Vec<usize>,
}

impl QueryInput {
    /// Split the parsed arguments into the query and the quote seed.
    fn resolve(args: QueryInputArgs, matches: &clap::ArgMatches) -> Self {
        let QueryInputArgs {
            mut query,
            escaped_query,
            quote,
        } = args;

        let mut indices = query_indices(matches);

        let quote = match quote {
            None => None,
            Some(QuoteArg::Attached(prefixed)) => Some(prefixed),
            // A bare `--quote` reads its value from the next word when that
            // word is exactly `true` or `false`. Anything else there is query
            // text, and the flag falls back to its default.
            Some(QuoteArg::Bare) => {
                Some(take_quote_value(&mut query, &mut indices, matches).unwrap_or(true))
            }
        };

        // The two halves are one query. They stay apart until here so that
        // `--quote` above only ever sees the unescaped words, and stay in this
        // order because `--` always comes last.
        if let Some(escaped) = escaped_query {
            query.get_or_insert_default().extend(escaped);
            indices.extend(matches.indices_of("escaped_query").into_iter().flatten());
        }

        let mut input = Self {
            query,
            quote,
            indices,
        };

        // These flags decide from the word alone, and read the same
        // `given_back` to fall back to their bare form. `-t`, `-T`, and `-u` need
        // the configured tools, and hand their words over later, through
        // `insert_word`.
        let given_back = [
            given_back::<ForkFlag>(matches),
            given_back::<ExpiryFlag>(matches),
            given_back::<CompactFlag>(matches),
            // The instantiation `Query` uses; `--id` claims the same words for
            // any of them.
            given_back::<FlagIds<false, false>>(matches),
        ];
        for (index, word) in given_back.into_iter().flatten() {
            input.insert_word(index, word);
        }

        input
    }

    /// Put `word` back into the query at the place it held on the command line.
    ///
    /// `index` is the word's clap index.
    pub(super) fn insert_word(&mut self, index: usize, word: String) {
        let at = self.indices.partition_point(|&i| i < index);
        self.query.get_or_insert_default().insert(at, word);
        self.indices.insert(at, index);
    }
}

/// Take the `true` / `false` word sitting directly after `--quote` out of the
/// query and return its value.
///
/// Returns `None` — leaving the query untouched — when the flag is followed
/// by anything else.
/// Words given after `--` are never candidates: they land in a separate
/// argument that this never reads.
fn take_quote_value(
    query: &mut Option<Vec<String>>,
    indices: &mut Vec<usize>,
    matches: &clap::ArgMatches,
) -> Option<bool> {
    // clap counts a flag and its value as two separate indices, so the word
    // directly after `--quote` sits one past the index of the flag's own
    // (defaulted) value.
    let after_quote = matches.index_of("quote")? + 1;
    let position = indices.iter().position(|&index| index == after_quote)?;

    let words = query.as_mut()?;
    let value = words.get(position)?.parse::<bool>().ok()?;

    words.remove(position);
    indices.remove(position);
    if words.is_empty() {
        *query = None;
    }

    Some(value)
}

/// Argument declarations for [`QueryInput`].
///
/// [`QueryInput`] borrows these declarations and resolves the parsed values
/// itself; it is never constructed as a command's own arguments.
#[derive(Debug, clap::Args)]
struct QueryInputArgs {
    /// The query to send.
    /// If not provided, uses `$JP_EDITOR`, `$VISUAL` or `$EDITOR` to open edit
    /// the query in an editor.
    ///
    /// A query consisting of a single `@path` value is read from that file.
    query: Option<Vec<String>>,

    /// Query words given after `--`.
    ///
    /// clap only fills this argument through the `--` separator, which makes it
    /// the record of which words were escaped.
    /// They are appended to `query` once `--quote` has been resolved, so `--`
    /// shields a `true` / `false` word from being read as the flag's value.
    #[arg(last = true, hide = true)]
    escaped_query: Option<Vec<String>>,

    /// Pre-fill the editor with the last assistant message quoted as a markdown
    /// blockquote (each line prefixed with ` >  `).
    ///
    /// Useful for inline replies: open `$EDITOR` with the assistant's last
    /// response pre-quoted, then intersperse your replies between the quoted
    /// lines (mutt/email style).
    /// The complete buffer — quotes plus your replies — becomes your next
    /// message.
    ///
    /// `--quote=false` seeds the message verbatim, without the ` >  ` prefixes.
    /// `--quote=true` is the same as a bare `--quote`.
    /// Both values also work unattached (`--quote false`); any other word after
    /// `--quote` stays part of the query, so `jp q --quote what now?` still
    /// asks "what now?".
    /// To ask a question that *is* `true` or `false`, put it after `--`.
    ///
    /// Forces the editor open by default; respects `--no-edit` / `--edit=false`
    /// if explicitly suppressed, in which case the quoted text is sent as-is
    /// and echoed to the terminal before the turn runs.
    /// Composes with `--replay`: the quote is taken from the stream *after* the
    /// replayed turn has been trimmed, i.e. the assistant message preceding the
    /// turn being replayed.
    ///
    /// If no prior assistant message exists in this conversation, a warning is
    /// emitted and the editor opens with whatever other content was seeded
    /// (query, stdin, or empty).
    #[arg(
        long = "quote",
        value_name = "BOOL",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "",
        value_parser = parse_quote_arg,
    )]
    quote: Option<QuoteArg>,
}

/// The `--quote` value as it was written on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteArg {
    /// `--quote` with nothing attached.
    Bare,

    /// `--quote=true` or `--quote=false`.
    Attached(bool),
}

/// Parse the `--quote` value.
///
/// The empty string is what a bare `--quote` yields, since `require_equals`
/// keeps it from swallowing the next word and the flag falls back to its
/// `default_missing_value`.
fn parse_quote_arg(s: &str) -> Result<QuoteArg, String> {
    match s {
        "" => Ok(QuoteArg::Bare),
        "true" => Ok(QuoteArg::Attached(true)),
        "false" => Ok(QuoteArg::Attached(false)),
        _ => Err("expected `true` or `false`".to_owned()),
    }
}

impl clap::Args for QueryInput {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        QueryInputArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        QueryInputArgs::augment_args_for_update(cmd)
    }
}

impl clap::FromArgMatches for QueryInput {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        QueryInputArgs::from_arg_matches(matches).map(|args| Self::resolve(args, matches))
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

/// A single tool selection directive from the CLI.
///
/// Directives are evaluated left-to-right, allowing users to compose tool sets
/// precisely (e.g. `--no-tools --tool=write --no-tools=fs_modify_file`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolDirective {
    EnableAll,
    DisableAll,
    Enable(String),
    Disable(String),
}

impl ToolDirective {
    fn bulk(enable: bool) -> Self {
        if enable {
            Self::EnableAll
        } else {
            Self::DisableAll
        }
    }

    fn named(enable: bool, name: String) -> Self {
        if enable {
            Self::Enable(name)
        } else {
            Self::Disable(name)
        }
    }

    /// Returns the single-tool directive as a string slice.
    #[must_use]
    pub(crate) fn as_single(&self) -> Option<&str> {
        match self {
            Self::Enable(name) | Self::Disable(name) => Some(name.as_str()),
            _ => None,
        }
    }
}

/// Ordered sequence of tool directives from `--tool` and `--no-tools`.
///
/// Implements manual [`clap::Args`] and [`clap::FromArgMatches`] to recover the
/// position of each flag value using [`ArgMatches::indices_of`], then merges
/// and sorts them by index into a single ordered list.
///
/// A value written directly before the query can only be told apart from the
/// query's first word once the configured tools are known, so the directives
/// are only reachable through [`settle`].
///
/// [`ArgMatches::indices_of`]: clap::ArgMatches::indices_of
/// [`settle`]: Self::settle
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolDirectives(State);

#[derive(Debug, Clone)]
enum State {
    /// As written on the command line.
    Parsed(Vec<ParsedDirective>),

    /// Decided against the configured tools.
    Settled(Vec<ToolDirective>),
}

impl Default for State {
    fn default() -> Self {
        Self::Settled(vec![])
    }
}

/// A `--tool` or `--no-tool` flag as written on the command line.
#[derive(Debug, Clone)]
enum ParsedDirective {
    Decided(ToolDirective),

    /// A value written directly before query text, which either names tools or
    /// is the first word of the query.
    Ambiguous {
        /// `true` for `--tool`, `false` for `--no-tool`.
        enable: bool,
        value: String,
        /// The clap index of `value`.
        index: usize,
    },
}

/// The directives to apply, and each value read as query text with its clap
/// index, in command-line order.
type Settlement = (Vec<ToolDirective>, Vec<(usize, String)>);

impl ToolDirectives {
    /// Return the directives to apply, deciding what each value written
    /// directly before the query is.
    ///
    /// Such a value names tools when it is a valid comma-separated list and
    /// `is_tool` accepts every name in it.
    /// Any other value is query text: its flag applies to every tool, and the
    /// value is returned with its clap index, in command-line order.
    ///
    /// A `--tool` value is only read as query text once `confirm_enable_all`
    /// accepts it, since its flag then enables every tool.
    /// An error from `confirm_enable_all` refuses, is returned as is, and
    /// leaves the directives unsettled.
    /// A `--no-tool` value is never asked about: disabling every tool grants
    /// nothing.
    ///
    /// Only the first successful call decides anything; later calls return the
    /// same directives and no query text.
    pub(crate) fn settle<E>(
        &mut self,
        is_tool: impl Fn(&str) -> bool,
        mut confirm_enable_all: impl FnMut(&str) -> Result<(), E>,
    ) -> Result<Settlement, E> {
        let parsed = match &self.0 {
            State::Settled(directives) => return Ok((directives.clone(), vec![])),
            State::Parsed(parsed) => parsed,
        };

        let mut directives = Vec::with_capacity(parsed.len());
        let mut words = vec![];
        for directive in parsed {
            let (enable, value, index) = match directive {
                ParsedDirective::Decided(directive) => {
                    directives.push(directive.clone());
                    continue;
                }
                ParsedDirective::Ambiguous {
                    enable,
                    value,
                    index,
                } => (*enable, value, *index),
            };

            let names = split_list::<String>(value, "tool name")
                .ok()
                .filter(|names| names.iter().all(|name| is_tool(name)));

            if let Some(names) = names {
                directives.extend(
                    names
                        .into_iter()
                        .map(|name| ToolDirective::named(enable, name)),
                );
                continue;
            }

            if enable {
                confirm_enable_all(value)?;
            }

            directives.push(ToolDirective::bulk(enable));
            words.push((index, value.clone()));
        }

        self.0 = State::Settled(directives.clone());
        Ok((directives, words))
    }
}

impl From<Vec<ToolDirective>> for ToolDirectives {
    fn from(directives: Vec<ToolDirective>) -> Self {
        Self(State::Settled(directives))
    }
}

/// Who answers when an argument's meaning needs the user's confirmation.
///
/// Empty unless the invocation has a user to ask, and an empty one refuses
/// every question.
#[derive(Clone, Default)]
pub(crate) struct ArgPrompts(Option<Arc<dyn PromptBackend>>);

impl ArgPrompts {
    /// Ask `backend` whenever an argument needs confirming.
    pub(crate) fn new(backend: Arc<dyn PromptBackend>) -> Self {
        Self(Some(backend))
    }

    /// The backend to ask, or `None` when nobody is there to answer.
    pub(crate) fn backend(&self) -> Option<&dyn PromptBackend> {
        self.0.as_deref()
    }
}

impl fmt::Debug for ArgPrompts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = if self.0.is_some() {
            "available"
        } else {
            "none"
        };
        f.debug_tuple("ArgPrompts").field(&state).finish()
    }
}

impl clap::FromArgMatches for ToolDirectives {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        let tool_values: Vec<String> = matches
            .get_many("tools")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        let tool_indices: Vec<_> = matches
            .indices_of("tools")
            .map(Iterator::collect)
            .unwrap_or_default();

        let no_tool_values: Vec<String> = matches
            .get_many("no_tools")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        let no_tool_indices: Vec<_> = matches
            .indices_of("no_tools")
            .map(Iterator::collect)
            .unwrap_or_default();

        let query_indices = query_indices(matches);

        let tools = tool_values.into_iter().zip(tool_indices).map(|v| (true, v));
        let no_tools = no_tool_values
            .into_iter()
            .zip(no_tool_indices)
            .map(|v| (false, v));

        let mut indexed = vec![];
        for (enable, (val, idx)) in tools.chain(no_tools) {
            if val.is_empty() {
                let directive = ToolDirective::bulk(enable);
                indexed.push((idx, ParsedDirective::Decided(directive)));
                continue;
            }

            // Only the configured tools can tell a tool name from the start of
            // the query, so the value is left whole, unsplit and unvalidated: a
            // query may well start with `hello,`.
            if query_indices.contains(&(idx + 1)) {
                indexed.push((idx, ParsedDirective::Ambiguous {
                    enable,
                    value: val,
                    index: idx,
                }));
                continue;
            }

            for name in split_list(&val, "tool name")? {
                let directive = ToolDirective::named(enable, name);
                indexed.push((idx, ParsedDirective::Decided(directive)));
            }
        }

        // A stable sort, so the names of a single flag keep the order they were
        // written in: they all carry that flag's index.
        indexed.sort_by_key(|(idx, _)| *idx);
        let parsed = indexed.into_iter().map(|(_, d)| d).collect();
        Ok(Self(State::Parsed(parsed)))
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

/// `-u` / `--tool-use`: the tool choice for this query alone.
///
/// A value written directly before the query is the query's first word unless
/// it names a tool, which only the configured tools can tell; until [`settle`]
/// decides, it reads as a tool name.
///
/// [`settle`]: Self::settle
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolUse {
    /// `None` without `-u`, `Some(None)` for a bare `-u`, `Some(Some(value))`
    /// for a value.
    choice: Option<Option<String>>,

    /// The clap index of a value that may be the query's first word instead.
    undecided: Option<usize>,
}

impl ToolUse {
    const ARG: &'static str = "tool_use";

    /// `None` without `-u`, `Some(None)` for a bare `-u`, `Some(Some(value))`
    /// for a value.
    pub(crate) fn choice(&self) -> Option<Option<&str>> {
        self.choice.as_ref().map(Option::as_deref)
    }

    /// Decide whether a value written directly before the query names a tool.
    ///
    /// When `is_tool` rejects it, the flag reads as a bare `-u` and the value
    /// is returned with its clap index, to go back into the query.
    /// No prompt is needed: a bare `-u` only requires the assistant to call one
    /// of the tools it already has.
    /// Only the first call decides anything.
    pub(crate) fn settle(&mut self, is_tool: impl Fn(&str) -> bool) -> Option<(usize, String)> {
        let index = self.undecided.take()?;
        if is_tool(self.choice()??) {
            return None;
        }

        let value = self.choice.replace(None).flatten()?;
        Some((index, value))
    }

    /// `-u NAME`, written where it cannot be the start of the query.
    #[cfg(test)]
    pub(crate) fn forcing(name: &str) -> Self {
        Self {
            choice: Some(Some(name.to_owned())),
            undecided: None,
        }
    }
}

impl clap::FromArgMatches for ToolUse {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        let Some(value) = matches.get_one::<String>(Self::ARG) else {
            return Ok(Self::default());
        };

        if value.is_empty() {
            return Ok(Self {
                choice: Some(None),
                undecided: None,
            });
        }

        // `true` and `false` are the flag's own spellings, never a tool name.
        let undecided = matches
            .index_of(Self::ARG)
            .filter(|index| query_indices(matches).contains(&(index + 1)))
            .filter(|_| value != "true" && value != "false");

        Ok(Self {
            choice: Some(Some(value.clone())),
            undecided,
        })
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl clap::Args for ToolUse {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.arg(
            clap::Arg::new(Self::ARG)
                .short('u')
                .long("tool-use")
                .value_name("TOOL")
                .help("The tool to use")
                .long_help(
                    "The tool to use.\n\nIf a value is provided, the tool matching the value will \
                     be used. The named tool runs for this query even when it is disabled, so it \
                     does not have to be enabled separately; a tool whose policy locks it off is \
                     refused.\n\nWithout a value, or with `true`, the assistant must use one of \
                     its enabled tools. With `false`, it may use none.\n\nThis applies to a \
                     single query and is *not* persisted: the next query forces nothing, and a \
                     tool that was disabled stays disabled. To persist tool choice behavior, set \
                     the `assistant.tool_choice` field in a configuration file.\n\nA value \
                     written directly before the query names a tool when a tool by that name is \
                     configured; otherwise it is the first word of the query, and the assistant \
                     must use one of its tools: `jp q -u fix the bug`.",
                )
                .num_args(0..=1)
                .default_missing_value(""),
        )
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

/// `--fork[=N]`: fork the conversation before the query, keeping its last `N`
/// turns, or all of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ForkFlag(Option<Option<usize>>);

impl ForkFlag {
    /// `None` without `--fork`, `Some(None)` to keep every turn,
    /// `Some(Some(n))` to keep the last `n`.
    pub(crate) fn turns(self) -> Option<Option<usize>> {
        self.0
    }
}

impl From<Option<Option<usize>>> for ForkFlag {
    fn from(turns: Option<Option<usize>>) -> Self {
        Self(turns)
    }
}

impl GivesBack for ForkFlag {
    const ARG: &'static str = "fork";

    fn claims(value: &str) -> bool {
        value.parse::<usize>().is_ok()
    }
}

impl clap::FromArgMatches for ForkFlag {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        let Some(value) = matches.get_one::<String>(Self::ARG) else {
            return Ok(Self(None));
        };

        if value.is_empty() || !given_back::<Self>(matches).is_empty() {
            return Ok(Self(Some(None)));
        }

        let turns = value.parse().map_err(|_| {
            clap::Error::raw(
                ErrorKind::InvalidValue,
                format!("invalid value for '--fork': expected a positive integer, got '{value}'\n"),
            )
        })?;

        Ok(Self(Some(Some(turns))))
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl clap::Args for ForkFlag {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.arg(
            clap::Arg::new(Self::ARG)
                .long("fork")
                .value_name("N")
                .help("Fork the conversation and start a new turn on the fork")
                .long_help(
                    "Fork the session's active conversation (or the one specified by --id) and \
                     start a new turn on the fork.\n\nIf N is given, the fork keeps only the last \
                     N turns.\n\nA value written directly before the query that is not a number \
                     is the query's first word instead, and the fork keeps every turn: `jp q \
                     --fork what if we` forks and asks \"what if we\".",
                )
                .num_args(0..=1)
                .default_missing_value("")
                .conflicts_with("new"),
        )
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

/// `--tmp[=DURATION]`: the new conversation expires once it is no longer
/// active, or after `DURATION`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ExpiryFlag(Option<Option<humantime::Duration>>);

impl ExpiryFlag {
    /// `None` without `--tmp`, `Some(None)` without a duration.
    pub(crate) fn duration(self) -> Option<Option<humantime::Duration>> {
        self.0
    }
}

impl GivesBack for ExpiryFlag {
    const ARG: &'static str = "expires_in";

    fn claims(value: &str) -> bool {
        value.parse::<humantime::Duration>().is_ok()
    }
}

impl clap::FromArgMatches for ExpiryFlag {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        let Some(value) = matches.get_one::<String>(Self::ARG) else {
            return Ok(Self(None));
        };

        if value.is_empty() || !given_back::<Self>(matches).is_empty() {
            return Ok(Self(Some(None)));
        }

        let duration = value.parse::<humantime::Duration>().map_err(|error| {
            clap::Error::raw(
                ErrorKind::InvalidValue,
                format!("invalid value '{value}' for '--tmp': {error}\n"),
            )
        })?;

        Ok(Self(Some(Some(duration))))
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl clap::Args for ExpiryFlag {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.arg(
            clap::Arg::new(Self::ARG)
                .long("tmp")
                .value_name("DURATION")
                .help("Set the expiration date of the conversation")
                .long_help(
                    "Set the expiration date of the conversation.\n\nThe conversation is \
                     persisted, but only until the conversation is no longer marked as active \
                     (e.g. when a new conversation is started), and when the expiration date is \
                     reached.\n\nThis differs from `--no-persist` in that the conversation can \
                     contain multiple turns, as long as it remains active and not expired.\n\nA \
                     value written directly before the query that is not a duration is the \
                     query's first word instead, and no duration is set: `jp q -n --tmp quick \
                     question` asks \"quick question\".",
                )
                .num_args(0..=1)
                .default_missing_value("")
                .requires("new"),
        )
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

impl clap::Args for ToolDirectives {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.arg(
            clap::Arg::new("tools")
                .short('t')
                .long("tool")
                .alias("tools")
                .help("The tool(s) to enable")
                .long_help(
                    "The tool(s) to enable.\n\nIf an existing tool is configured with a matching \
                     name, it is enabled for this query and every later one on the conversation; \
                     use `--no-tool` to turn it back off.\n\nTo run a disabled tool just once, \
                     use `--tool-use NAME` instead.\n\nIf no arguments are provided, every tool \
                     that allows it is enabled; a tool set to `explicit` or `always` is \
                     unaffected.\n\nName several tools at once by separating them with commas \
                     (`--tool=read,write`), or by providing this flag multiple times. Flags are \
                     evaluated left-to-right, so `--no-tools --tool=write` first disables \
                     everything, then re-enables only 'write'.\n\nA value written directly before \
                     the query, as in `jp q -t fix the bug`, names tools when every name in it is \
                     a configured tool. Otherwise JP asks before enabling every tool and reading \
                     the value as the first word of the query, and refuses when there is nobody \
                     to ask. Put the query after `--` to enable every tool without being asked: \
                     `jp q -t -- fix the bug`.",
                )
                .action(ArgAction::Append)
                .num_args(0..=1)
                // The values are split on commas by hand rather than with
                // `value_delimiter(',')`, which splits before this empty string
                // is read: an empty segment in `--tool=read,` would then be
                // indistinguishable from a bare `--tool` and enable every tool.
                .default_missing_value(""),
        )
        .arg(
            clap::Arg::new("no_tools")
                .short('T')
                .long("no-tool")
                .alias("no-tools")
                .help("Disable tool(s)")
                .long_help(
                    "Disable tool(s).\n\nIf provided without a value, every tool that allows it \
                     is disabled (a tool set to `explicit` or `always` is unaffected), otherwise \
                     name the tools to disable, separated by commas (`--no-tool=read,write`) or \
                     across repeated flags.\n\nThe change applies to this query and every later \
                     one on the conversation; use `--tool` to turn tools back on. To suppress \
                     tools for a single query, use `--no-tool-use`.\n\nFlags are evaluated \
                     left-to-right together with `--tool`.\n\nA value written directly before the \
                     query names tools when every name in it is a configured tool; otherwise it \
                     is the first word of the query, and the flag disables every tool.",
                )
                .action(ArgAction::Append)
                .num_args(0..=1)
                .default_missing_value(""),
        )
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}
