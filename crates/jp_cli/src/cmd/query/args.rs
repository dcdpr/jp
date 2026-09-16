//! The `query` arguments clap cannot derive on its own.
//!
//! Both types here exist because their flags depend on *where* a value sat on
//! the command line, which a derived `clap::Args` cannot see.
//! [`QueryInput`] has to know whether the word after `--quote` was its value or
//! the start of the query; [`ToolDirectives`] has to know the order `--tool`
//! and `--no-tool` were written in.
//! Everything else on `Query` is derived as usual.

use clap::ArgAction;

use crate::parser::split_list;

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
}

impl QueryInput {
    /// Split the parsed arguments into the query and the quote seed.
    fn resolve(args: QueryInputArgs, matches: &clap::ArgMatches) -> Self {
        let QueryInputArgs {
            mut query,
            escaped_query,
            quote,
        } = args;

        let quote = match quote {
            None => None,
            Some(QuoteArg::Attached(prefixed)) => Some(prefixed),
            // A bare `--quote` reads its value from the next word when that
            // word is exactly `true` or `false`. Anything else there is query
            // text, and the flag falls back to its default.
            Some(QuoteArg::Bare) => Some(take_quote_value(&mut query, matches).unwrap_or(true)),
        };

        // The two halves are one query. They stay apart until here so that
        // `--quote` above only ever sees the unescaped words, and stay in this
        // order because `--` always comes last.
        if let Some(escaped) = escaped_query {
            query.get_or_insert_default().extend(escaped);
        }

        Self { query, quote }
    }
}

/// Take the `true` / `false` word sitting directly after `--quote` out of the
/// query and return its value.
///
/// Returns `None` — leaving the query untouched — when the flag is followed
/// by anything else.
/// Words given after `--` are never candidates: they land in a separate
/// argument that this never reads.
fn take_quote_value(query: &mut Option<Vec<String>>, matches: &clap::ArgMatches) -> Option<bool> {
    // clap counts a flag and its value as two separate indices, so the word
    // directly after `--quote` sits one past the index of the flag's own
    // (defaulted) value.
    let after_quote = matches.index_of("quote")? + 1;
    let position = matches
        .indices_of("query")?
        .position(|index| index == after_quote)?;

    let words = query.as_mut()?;
    let value = words.get(position)?.parse::<bool>().ok()?;

    words.remove(position);
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
    /// Returns the single-tool directive as a string slice.
    #[must_use]
    pub(crate) fn as_single(&self) -> Option<&str> {
        match self {
            Self::Enable(name) | Self::Disable(name) => Some(name.as_str()),
            _ => None,
        }
    }
}

/// Ordered sequence of tool directives parsed from `--tool` and `--no-tools`.
///
/// Implements manual [`clap::Args`] and [`clap::FromArgMatches`] to recover the
/// position of each flag value using [`ArgMatches::indices_of`], then merges
/// and sorts them by index into a single ordered list.
///
/// [`ArgMatches::indices_of`]: clap::ArgMatches::indices_of
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolDirectives(pub(super) Vec<ToolDirective>);

impl std::ops::Deref for ToolDirectives {
    type Target = [ToolDirective];

    fn deref(&self) -> &Self::Target {
        &self.0
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

        let mut indexed = vec![];
        for (val, idx) in tool_values.into_iter().zip(tool_indices) {
            if val.is_empty() {
                indexed.push((idx, ToolDirective::EnableAll));
                continue;
            }

            for name in split_list(&val, "tool name")? {
                indexed.push((idx, ToolDirective::Enable(name)));
            }
        }

        for (val, idx) in no_tool_values.into_iter().zip(no_tool_indices) {
            if val.is_empty() {
                indexed.push((idx, ToolDirective::DisableAll));
                continue;
            }

            for name in split_list(&val, "tool name")? {
                indexed.push((idx, ToolDirective::Disable(name)));
            }
        }

        // A stable sort, so the names of a single flag keep the order they were
        // written in: they all carry that flag's index.
        indexed.sort_by_key(|(idx, _)| *idx);
        Ok(Self(indexed.into_iter().map(|(_, d)| d).collect()))
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
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
                     everything, then re-enables only 'write'.",
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
                     left-to-right together with `--tool`.",
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
