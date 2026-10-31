//! The `query` command line: its arguments, and how they are read.
//!
//! `Query::augment` declares the arguments and `Query::from_matches` reads them
//! into a `Query<Parsed>`.
//! Most flags are decided as they are read.
//! `-t`, `-T`, and `-u` can only tell a tool name from the first word of the
//! query once the configured tools are known, so they stay undecided until
//! `Query::settle` turns the command into a `Query<Settled>`.

use std::collections::HashSet;

use clap::{
    Arg, ArgAction, ArgGroup, ArgMatches, Command,
    builder::{BoolValueParser, NonEmptyStringValueParser, TypedValueParser},
    value_parser,
};
use jp_config::{
    PartialAppConfig,
    assignment::KvAssignment,
    model::parameters::{ReasoningConfig, ServiceTier},
    providers::llm::AuthEntry,
};

use super::{Query, parse_schema, string_or_path, tool};
use crate::{
    Spelling,
    cli::flag::{
        ArgPrompts, Claim, Confirmation, Flag, FlagValues, FreeText, OptionalValue, Parsed, Settle,
        SettleError, Settled, flag_arg,
    },
    cmd::{compact_flag::Compact, conversation_id::Ids},
    parser::{AttachmentUrlOrPath, split_list},
};

/// The `--id` flag `query` takes: one conversation, outside any session.
type Target = Ids<false, false, false>;

/// The names of the tools a query can enable or use: those the configuration
/// declares, and the built-ins.
#[derive(Debug, Default)]
pub(crate) struct KnownTools(HashSet<String>);

impl KnownTools {
    fn of(config: &PartialAppConfig) -> Self {
        let configured = config.conversation.tools.tools.keys().cloned();
        let builtins = tool::builtins::all().into_keys();
        Self(configured.chain(builtins).collect())
    }

    fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }
}

impl Query<Parsed> {
    /// Declare the arguments of `jp query` on `cmd`.
    #[expect(clippy::too_many_lines)]
    pub(crate) fn augment(cmd: Command) -> Command {
        cmd.group(ArgGroup::new("new").arg("new_conversation"))
            .arg(
                Arg::new("template")
                    .short('%')
                    .long("template")
                    .action(ArgAction::SetTrue)
                    .help("Use the query string as a Jinja2 template")
                    .long_help(
                        "Use the query string as a Jinja2 template.\n\nYou can provide values for \
                         template variables using the `template.values` config key.",
                    ),
            )
            .arg(
                Arg::new("schema")
                    .short('s')
                    .long("schema")
                    .value_name("SCHEMA")
                    .value_parser(string_or_path.try_map(parse_schema))
                    .help("Constrain the assistant's response to match a JSON schema")
                    .long_help(
                        "Constrain the assistant's response to match a JSON schema.\n\nAccepts \
                         either a full JSON Schema object or a concise DSL:\n\n-s 'summary' → \
                         single string field -s 'name, age int, bio' → mixed types -s 'summary: \
                         a brief summary' → field with description\n\nSee: \
                         <https://jp.computer/rfd/030-schema-dsl>",
                    ),
            )
            .arg(
                Arg::new("replay")
                    .long("replay")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("new")
                    .help("Replay the last turn in the conversation")
                    .long_help(
                        "Replay the last turn in the conversation.\n\nThe turn is removed, \
                         including any reply sent while it was interrupted, and the message \
                         that started it is sent again. If a query is provided, it is placed \
                         before that message. If no query is provided, $EDITOR will open with \
                         that message.",
                    ),
            )
            .arg(flag_arg::<Target>())
            .arg(flag_arg::<Fork>())
            .arg(
                Arg::new("new_conversation")
                    .short('n')
                    .long("new")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("id")
                    .help("Start a new conversation without any message history"),
            )
            .arg(
                Arg::new("local")
                    .short('l')
                    .long("local")
                    .action(ArgAction::SetTrue)
                    .requires("new_conversation")
                    .conflicts_with("no_local")
                    .help("Store the conversation locally, outside of the workspace"),
            )
            .arg(
                Arg::new("no_local")
                    .short('L')
                    .long("no-local")
                    .action(ArgAction::SetTrue)
                    .requires("new_conversation")
                    .conflicts_with("local")
                    .help("Store the conversation in the current workspace"),
            )
            .arg(
                Arg::new("attachments")
                    .short('a')
                    .long("attachment")
                    .alias("attach")
                    .action(ArgAction::Append)
                    .value_name("ATTACHMENTS")
                    .value_parser(value_parser!(AttachmentUrlOrPath))
                    .help("Add attachment to the configuration"),
            )
            .arg(
                Arg::new("edit")
                    .short('e')
                    .long("edit")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("no_edit")
                    .help("Open an editor to compose the query")
                    .long_help(
                        "Open an editor to compose the query.\n\nForces the editor open even when \
                         a query is provided inline (or piped on stdin). Use `--no-edit` (`-E`) to \
                         suppress it.\n\nTo change which editor is used, set `editor.cmd` in your \
                         config (or pass `--cfg editor.cmd=...`).",
                    ),
            )
            .arg(
                Arg::new("no_edit")
                    .short('E')
                    .long("no-edit")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("edit")
                    .help("Do not edit the query")
                    .long_help("Do not edit the query.\n\nSee `--edit` for more details."),
            )
            .arg(
                Arg::new(QueryText::WORDS)
                    .action(ArgAction::Append)
                    .num_args(1..)
                    .value_name("QUERY")
                    .help(
                        "The query to send. If not provided, uses `$JP_EDITOR`, `$VISUAL` or \
                         `$EDITOR` to open edit the query in an editor",
                    )
                    .long_help(
                        "The query to send. If not provided, uses `$JP_EDITOR`, `$VISUAL` or \
                         `$EDITOR` to open edit the query in an editor.\n\nA query consisting of a \
                         single `@path` value is read from that file.",
                    ),
            )
            // clap only fills this argument through the `--` separator, which
            // makes it the record of which words were escaped: no flag's value
            // is ever read as a word written after `--`.
            .arg(
                Arg::new(QueryText::ESCAPED)
                    .action(ArgAction::Append)
                    .num_args(1..)
                    .last(true)
                    .hide(true)
                    .value_name("ESCAPED_QUERY")
                    .help("Query words given after `--`"),
            )
            .arg(flag_arg::<Quote>())
            .arg(
                Arg::new("model")
                    .short('m')
                    .long("model")
                    .value_name("MODEL")
                    .help("The model to use"),
            )
            .arg(
                Arg::new("auth")
                    .long("auth")
                    .action(ArgAction::Append)
                    .value_name("CHAIN")
                    .value_delimiter(',')
                    .value_parser(value_parser!(AuthEntry))
                    .help("Which credential to bill this turn to")
                    .long_help(
                        "Which credential to bill this turn to.\n\nTakes the same entries as the \
                         `auth` chain in configuration, comma separated: a credential name, \
                         `api_key`, `subscription`, or `<kind>:<name>`. `api` and `sub` are \
                         accepted for the kinds.\n\nApplies to the provider the chosen model \
                         belongs to, and is recorded on the turn, so the rest of the conversation \
                         keeps billing the same way until another `--auth` changes it.",
                    ),
            )
            .arg(
                Arg::new("parameters")
                    .short('p')
                    .long("param")
                    .action(ArgAction::Append)
                    .value_name("KEY=VALUE")
                    .value_parser(value_parser!(KvAssignment))
                    .help("The model parameters to use"),
            )
            .arg(
                Arg::new("reasoning")
                    .short('r')
                    .long("reasoning")
                    .value_name("REASONING")
                    .value_parser(value_parser!(ReasoningConfig))
                    .help("Enable reasoning"),
            )
            .arg(
                Arg::new("no_reasoning")
                    .short('R')
                    .long("no-reasoning")
                    .action(ArgAction::SetTrue)
                    .help("Disable reasoning"),
            )
            .arg(
                Arg::new("service_tier")
                    .long("tier")
                    .value_name("TIER")
                    .value_parser(value_parser!(ServiceTier))
                    .conflicts_with("no_service_tier")
                    .help("Capacity tier to request from the provider")
                    .long_help(
                        "Capacity tier to request from the provider.\n\nProviders sell more than \
                         one grade of capacity for the same model: `flex` is cheaper and slower, \
                         `priority` is faster and pricier, `standard` is the regular rate, and \
                         `off` asks for no tier at all.\n\nWhich tiers are available depends on \
                         the provider, the model, and the account; a provider that sells no \
                         equivalent refuses the query rather than falling back to a rung that \
                         costs something else. One that sells the tier but cannot serve this \
                         model with it may still fall back.\n\nApplies to this query and every \
                         later one on the conversation, because each tier is served from separate \
                         capacity and switching discards the prompt cache.",
                    ),
            )
            .arg(
                Arg::new("no_service_tier")
                    .long("no-tier")
                    .action(ArgAction::SetTrue)
                    .help("Stop asking for a capacity tier")
                    .long_help(
                        "Stop asking for a capacity tier.\n\nShorthand for `--tier off`, which \
                         sends no tier and leaves the choice to whatever the account is set up \
                         for.\n\nThis is not `--tier standard`: `standard` names a rung and is \
                         sent as one.",
                    ),
            )
            .arg(
                Arg::new("hide_reasoning")
                    .long("hide-reasoning")
                    .action(ArgAction::SetTrue)
                    .help("Do not display the reasoning content")
                    .long_help(
                        "Do not display the reasoning content.\n\nThis does not stop the \
                         assistant from generating reasoning tokens to help with its accuracy, \
                         but it does not display them in the output.",
                    ),
            )
            .arg(
                Arg::new("hide_tool_calls")
                    .long("hide-tool-calls")
                    .action(ArgAction::SetTrue)
                    .help("Do not display tool calls")
                    .long_help(
                        "Do not display tool calls.\n\nThis does not stop the assistant from \
                         running tool calls, but it does not display them in the output.",
                    ),
            )
            .arg(flag_arg::<EnableTools>())
            .arg(flag_arg::<DisableTools>())
            .arg(flag_arg::<Expiry>())
            .arg(
                Arg::new("title")
                    .long("title")
                    .value_name("TITLE")
                    .conflicts_with("no_title")
                    .help("Set a custom title for the conversation")
                    .long_help(
                        "Set a custom title for the conversation.\n\nApplied to the resolved \
                         conversation (new, forked, or resumed) before the turn runs. Skips title \
                         auto-generation for new conversations — your title wins. Mutually \
                         exclusive with `--no-title`.",
                    ),
            )
            .arg(
                Arg::new("no_title")
                    .long("no-title")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("title")
                    .help("Disable the title for the conversation")
                    .long_help(
                        "Disable the title for the conversation.\n\nClears any existing title on \
                         the resolved conversation (new, forked, or resumed) and skips \
                         auto-generation for this run. Mutually exclusive with `--title`.",
                    ),
            )
            .arg(flag_arg::<ToolUse>())
            .arg(
                Arg::new("no_tool_use")
                    .short('U')
                    .long("no-tool-use")
                    .action(ArgAction::SetTrue)
                    .help("Disable tool use by the assistant")
                    .long_help(
                        "Disable tool use by the assistant.\n\nThis applies to a single query and \
                         is *not* persisted. To keep tools off, use `--no-tool`, which disables \
                         them for every future query on the conversation.",
                    ),
            )
            .arg(flag_arg::<Compact>())
            .arg(
                Arg::new("mount")
                    .long("mount")
                    .action(ArgAction::Append)
                    .value_name("[TOOL:][NAME=]PATH[:MODE]")
                    .help(
                        "Mount an external path into the workspace as a symlink and grant the \
                         assistant access to it",
                    )
                    .long_help(
                        "Mount an external path into the workspace as a symlink and grant the \
                         assistant access to it.\n\nForm: `[TOOL:][NAME=]PATH[:MODE]`. `NAME` is \
                         the workspace-relative location for the symlink, `PATH` is the external \
                         target, and `MODE` is `ro` (default) or `rw`. Without `NAME=`, `PATH` \
                         must exist and the symlink is named after its last segment; a leading \
                         `TOOL:` is then a tool only if the rest of the path exists and the whole \
                         value does not. `rw` requires a `TOOL:` prefix; without a `TOOL:` prefix \
                         the grant applies to all enabled local and command plugin tools. Repeat \
                         the flag to mount several paths.",
                    ),
            )
    }

    /// Read the arguments `matches` holds for `jp query`.
    ///
    /// Each flag decided by its word alone is decided here, and gives the words
    /// it does not claim back to the query.
    ///
    /// # Errors
    ///
    /// Returns an error when a value that is a flag's does not parse.
    pub(crate) fn from_matches(
        matches: &ArgMatches,
        spelling: &Spelling,
    ) -> Result<Self, clap::Error> {
        let mut text = QueryText::read(matches);

        let target = Flag::<Target>::decide_now(matches, spelling, &mut text)?;
        let fork = Flag::<Fork>::decide_now(matches, spelling, &mut text)?;
        let expires_in = Flag::<Expiry>::decide_now(matches, spelling, &mut text)?;
        let compact = Flag::<Compact>::decide_now(matches, spelling, &mut text)?;
        let quote = Flag::<Quote>::decide_now(matches, spelling, &mut text)?;

        let tool_directives = ToolFlags {
            enable: Flag::read(matches, spelling, &text)?,
            disable: Flag::read(matches, spelling, &text)?,
        };
        let tool_use = Flag::read(matches, spelling, &text)?;

        Ok(Self {
            template: matches.get_flag("template"),
            schema: matches.get_one("schema").cloned(),
            replay: matches.get_flag("replay"),
            target,
            fork,
            new_conversation: matches.get_flag("new_conversation"),
            local: matches.get_flag("local"),
            no_local: matches.get_flag("no_local"),
            attachments: values(matches, "attachments"),
            edit: matches.get_flag("edit"),
            no_edit: matches.get_flag("no_edit"),
            query: text,
            quote,
            model: matches.get_one("model").cloned(),
            auth: values(matches, "auth"),
            parameters: values(matches, "parameters"),
            reasoning: matches.get_one("reasoning").copied(),
            no_reasoning: matches.get_flag("no_reasoning"),
            service_tier: matches.get_one("service_tier").copied(),
            no_service_tier: matches.get_flag("no_service_tier"),
            hide_reasoning: matches.get_flag("hide_reasoning"),
            hide_tool_calls: matches.get_flag("hide_tool_calls"),
            tool_directives,
            expires_in,
            title: matches.get_one("title").cloned(),
            no_title: matches.get_flag("no_title"),
            tool_use,
            no_tool_use: matches.get_flag("no_tool_use"),
            compact,
            mount: values(matches, "mount"),
        })
    }

    /// Decide whether each `-t`, `-T`, or `-u` value written directly before
    /// the query names tools or starts the query.
    ///
    /// The value names tools when every name in it is a tool `config`
    /// configures, or a built-in.
    /// Otherwise it goes back into the query where it was written, and its flag
    /// reads as if written bare: `-t`/`-T` apply to every tool, `-u` requires
    /// some tool.
    /// For `-t`, that needs the user's confirmation through `prompts` first.
    ///
    /// # Errors
    ///
    /// Returns an error when a `-t` value names no configured tool and the user
    /// declines, or nobody is there to ask.
    pub(crate) fn settle(
        self,
        config: &PartialAppConfig,
        prompts: &ArgPrompts,
    ) -> Result<Query<Settled>, SettleError> {
        let tools = KnownTools::of(config);

        let Self {
            template,
            schema,
            replay,
            target,
            fork,
            new_conversation,
            local,
            no_local,
            attachments,
            edit,
            no_edit,
            query: mut text,
            quote,
            model,
            auth,
            parameters,
            reasoning,
            no_reasoning,
            service_tier,
            no_service_tier,
            hide_reasoning,
            hide_tool_calls,
            tool_directives,
            expires_in,
            title,
            no_title,
            tool_use,
            no_tool_use,
            compact,
            mount,
        } = self;

        let tool_directives = tool_directives.settle(&tools, prompts, &mut text)?;
        let tool_use = tool_use.settle(&tools, prompts, &mut text)?;

        Ok(Query {
            template,
            schema,
            replay,
            target,
            fork,
            new_conversation,
            local,
            no_local,
            attachments,
            edit,
            no_edit,
            query: text.into_words(),
            quote,
            model,
            auth,
            parameters,
            reasoning,
            no_reasoning,
            service_tier,
            no_service_tier,
            hide_reasoning,
            hide_tool_calls,
            tool_directives,
            expires_in,
            title,
            no_title,
            tool_use,
            no_tool_use,
            compact,
            mount,
        })
    }
}

/// Every value of the argument `id`, in command-line order.
fn values<T: Clone + Send + Sync + 'static>(matches: &ArgMatches, id: &str) -> Vec<T> {
    matches
        .get_many(id)
        .map(|values| values.cloned().collect())
        .unwrap_or_default()
}

/// The words of the query, written before `--` or after it.
#[derive(Debug, Default)]
pub(crate) struct QueryText {
    /// The words, in command-line order.
    words: Vec<String>,

    /// The clap index of each word in `words`.
    indices: Vec<usize>,

    /// The clap indices of the words written before `--`, as read.
    ///
    /// Only those can directly follow a flag's value.
    unescaped: Vec<usize>,
}

impl QueryText {
    /// The clap id of the words written before `--`.
    const WORDS: &'static str = "query";

    /// The clap id of the words written after `--`.
    const ESCAPED: &'static str = "escaped_query";

    fn read(matches: &ArgMatches) -> Self {
        let mut text = Self::default();
        for id in [Self::WORDS, Self::ESCAPED] {
            text.words.extend(values::<String>(matches, id));
            text.indices
                .extend(matches.indices_of(id).into_iter().flatten());

            if id == Self::WORDS {
                text.unescaped.clone_from(&text.indices);
            }
        }

        text
    }
}

impl FreeText for QueryText {
    type Words = Option<Vec<String>>;

    fn takes_words(&self) -> bool {
        true
    }

    fn is_text(&self, index: usize) -> bool {
        self.unescaped.contains(&index)
    }

    fn give(&mut self, index: usize, word: String) {
        let at = self.indices.partition_point(|&i| i < index);
        self.words.insert(at, word);
        self.indices.insert(at, index);
    }

    fn into_words(self) -> Option<Vec<String>> {
        (!self.words.is_empty()).then_some(self.words)
    }
}

/// `--fork[=N]`: fork the conversation before the query, keeping its last `N`
/// turns, or all of them.
pub(crate) enum Fork {}

impl OptionalValue for Fork {
    const ID: &'static str = "fork";

    type Context = ();
    type Output = Option<Option<usize>>;
    type Value = usize;

    fn declare(arg: Arg) -> Arg {
        arg.long("fork")
            .value_name("N")
            .conflicts_with("new")
            .help("Fork the conversation and start a new turn on the fork")
            .long_help(
                "Fork the session's active conversation (or the one specified by --id) and start \
                 a new turn on the fork.\n\nIf N is given, the fork keeps only the last N turns.",
            )
    }

    fn parser() -> impl TypedValueParser<Value = usize> {
        parse_turns
    }

    fn claim(_: &str, parsed: Option<usize>, (): &()) -> Claim<usize> {
        Claim::value_or_text(parsed)
    }

    fn collect(values: FlagValues<usize>) -> Result<Self::Output, clap::Error> {
        Ok(values.last())
    }
}

/// `--tmp[=DURATION]`: the new conversation expires once it is no longer
/// active, or after `DURATION`.
pub(crate) enum Expiry {}

impl OptionalValue for Expiry {
    const ID: &'static str = "expires_in";

    type Context = ();
    type Output = Option<Option<humantime::Duration>>;
    type Value = humantime::Duration;

    fn declare(arg: Arg) -> Arg {
        arg.long("tmp")
            .value_name("DURATION")
            .requires("new")
            .help("Set the expiration date of the conversation")
            .long_help(
                "Set the expiration date of the conversation.\n\nThe conversation is persisted, \
                 but only until the conversation is no longer marked as active (e.g. when a new \
                 conversation is started), and when the expiration date is reached.\n\nThis \
                 differs from `--no-persist` in that the conversation can contain multiple turns, \
                 as long as it remains active and not expired.",
            )
    }

    fn parser() -> impl TypedValueParser<Value = humantime::Duration> {
        parse_duration
    }

    fn claim(_: &str, parsed: Option<humantime::Duration>, (): &()) -> Claim<humantime::Duration> {
        Claim::value_or_text(parsed)
    }

    fn collect(values: FlagValues<humantime::Duration>) -> Result<Self::Output, clap::Error> {
        Ok(values.last())
    }
}

/// `--quote[=BOOL]`: seed the editor with the last assistant message, quoted.
pub(crate) enum Quote {}

impl OptionalValue for Quote {
    // `true` and `false` are rarely the start of a query, and no other word is
    // ever the flag's, so a separate word is the flag's only when it is one of
    // those two, wherever it sits.
    const CLAIMS_EVERY_SEPARATE_VALUE: bool = true;
    const ID: &'static str = "quote";

    type Context = ();
    /// `Some(true)` prefixes the quoted message with ` >  `, `Some(false)`
    /// seeds it verbatim, `None` means `--quote` was not given.
    type Output = Option<bool>;
    type Value = bool;

    fn declare(arg: Arg) -> Arg {
        arg.long("quote")
            .value_name("BOOL")
            .help(
                "Pre-fill the editor with the last assistant message quoted as a markdown \
                 blockquote (each line prefixed with ` >  `)",
            )
            .long_help(
                "Pre-fill the editor with the last assistant message quoted as a markdown \
                 blockquote (each line prefixed with ` >  `).\n\nUseful for inline replies: open \
                 `$EDITOR` with the assistant's last response pre-quoted, then intersperse your \
                 replies between the quoted lines (mutt/email style). The complete buffer — \
                 quotes plus your replies — becomes your next message.\n\n`--quote=false` seeds \
                 the message verbatim, without the ` >  ` prefixes. `--quote=true` is the same as \
                 a bare `--quote`. Both values also work unattached (`--quote false`); any other \
                 word after `--quote` stays part of the query, so `jp q --quote what now?` still \
                 asks \"what now?\". To ask a question that *is* `true` or `false`, put it after \
                 `--`.\n\nForces the editor open by default; respects `--no-edit` / \
                 `--edit=false` if explicitly suppressed, in which case the quoted text is sent \
                 as-is and echoed to the terminal before the turn runs. Composes with `--replay`: \
                 the quote is taken from the stream *after* the replayed turn has been trimmed, \
                 i.e. the assistant message preceding the turn being replayed.\n\nIf no prior \
                 assistant message exists in this conversation, a warning is emitted and the \
                 editor opens with whatever other content was seeded (query, stdin, or empty).",
            )
    }

    fn parser() -> impl TypedValueParser<Value = bool> {
        BoolValueParser::new()
    }

    fn claim(_: &str, parsed: Option<bool>, (): &()) -> Claim<bool> {
        Claim::value_or_text(parsed)
    }

    fn collect(values: FlagValues<bool>) -> Result<Option<bool>, clap::Error> {
        Ok(values.last().map(|value| value.unwrap_or(true)))
    }
}

/// `-u` / `--tool-use[=TOOL]`: the tool choice for this query alone.
pub(crate) enum ToolUse {}

impl OptionalValue for ToolUse {
    const ID: &'static str = "tool_use";

    type Context = KnownTools;
    /// `None` without `-u`, `Some(None)` for a bare `-u`, `Some(Some(value))`
    /// for a value.
    type Output = Option<Option<String>>;
    type Value = String;

    fn declare(arg: Arg) -> Arg {
        arg.short('u')
            .long("tool-use")
            .value_name("TOOL")
            .help("The tool to use")
            .long_help(
                "The tool to use.\n\nIf a value is provided, the tool matching the value will be \
                 used. The named tool runs for this query even when it is disabled, so it does \
                 not have to be enabled separately; a tool whose policy locks it off is \
                 refused.\n\nWithout a value, or with `true`, the assistant must use one of its \
                 enabled tools. With `false`, it may use none.\n\nThis applies to a single query \
                 and is *not* persisted: the next query forces nothing, and a tool that was \
                 disabled stays disabled. To persist tool choice behavior, set the \
                 `assistant.tool_choice` field in a configuration file.",
            )
    }

    fn parser() -> impl TypedValueParser<Value = String> {
        NonEmptyStringValueParser::new()
    }

    /// `true` and `false` are the flag's own spellings, never a tool name.
    ///
    /// A word naming no tool needs no confirmation: a bare `-u` only requires
    /// the assistant to call one of the tools it already has.
    fn claim(_: &str, parsed: Option<String>, tools: &KnownTools) -> Claim<String> {
        match parsed {
            Some(name) if matches!(name.as_str(), "true" | "false") || tools.contains(&name) => {
                Claim::Value(name)
            }
            _ => Claim::Text,
        }
    }

    fn collect(values: FlagValues<String>) -> Result<Self::Output, clap::Error> {
        Ok(values.last())
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

/// `-t` and `-T`, whose directives apply together, in the order written.
#[derive(Debug, Default)]
pub(crate) struct ToolFlags {
    enable: Flag<EnableTools>,
    disable: Flag<DisableTools>,
}

impl Settle for ToolFlags {
    type Context = KnownTools;
    type Output = Vec<ToolDirective>;

    fn settle(
        self,
        tools: &KnownTools,
        prompts: &ArgPrompts,
        text: &mut impl FreeText,
    ) -> Result<Vec<ToolDirective>, SettleError> {
        let enable = self.enable.settle(tools, prompts, text)?;
        let disable = self.disable.settle(tools, prompts, text)?;

        let mut indexed: Vec<_> = enable
            .into_indexed()
            .into_iter()
            .map(|(index, names)| (index, true, names))
            .chain(
                disable
                    .into_indexed()
                    .into_iter()
                    .map(|(index, names)| (index, false, names)),
            )
            .collect();

        // A stable sort, so the names of a single flag keep the order they were
        // written in: they all carry that flag's index.
        indexed.sort_by_key(|(index, ..)| *index);

        Ok(indexed
            .into_iter()
            .flat_map(|(_, enable, names)| match names {
                None => vec![ToolDirective::bulk(enable)],
                Some(names) => names
                    .into_iter()
                    .map(|name| ToolDirective::named(enable, name))
                    .collect(),
            })
            .collect())
    }
}

/// `-t` / `--tool[=NAMES]`: tools to enable, or every tool.
pub(crate) enum EnableTools {}

impl OptionalValue for EnableTools {
    const ID: &'static str = "tools";

    type Context = KnownTools;
    type Output = FlagValues<Vec<String>>;
    type Value = Vec<String>;

    fn declare(arg: Arg) -> Arg {
        // The values are split on commas by hand rather than with
        // `value_delimiter(',')`, which splits before the empty value of a bare
        // flag is read: an empty segment in `--tool=read,` would then be
        // indistinguishable from a bare `--tool` and enable every tool.
        arg.short('t')
            .long("tool")
            .alias("tools")
            .action(ArgAction::Append)
            .help("The tool(s) to enable")
            .long_help(
                "The tool(s) to enable.\n\nIf an existing tool is configured with a matching \
                 name, it is enabled for this query and every later one on the conversation; use \
                 `--no-tool` to turn it back off.\n\nTo run a disabled tool just once, use \
                 `--tool-use NAME` instead.\n\nIf no arguments are provided, every tool that \
                 allows it is enabled; a tool set to `explicit` or `always` is \
                 unaffected.\n\nName several tools at once by separating them with commas \
                 (`--tool=read,write`), or by providing this flag multiple times. Flags are \
                 evaluated left-to-right, so `--no-tools --tool=write` first disables everything, \
                 then re-enables only 'write'.",
            )
    }

    fn parser() -> impl TypedValueParser<Value = Vec<String>> {
        parse_tool_names
    }

    /// A word naming no configured tool starts the query only once the user
    /// agrees, because `-t` written bare enables every tool.
    fn claim(value: &str, parsed: Option<Vec<String>>, tools: &KnownTools) -> Claim<Vec<String>> {
        if let Some(names) = known_tools(parsed, tools) {
            return Claim::Value(names);
        }

        Claim::Confirm(Confirmation {
            question: format!(
                "`{value}` is not a configured tool.\nEnable every tool on this conversation, and \
                 start the query with `{value}`?"
            ),
            accept: "yes, enable every tool",
            refusal: format!(
                "`{value}` is not a configured tool. To enable every tool and start the query \
                 with `{value}`, put the query after `--`: `-t -- {value} ...`"
            ),
        })
    }

    fn collect(values: FlagValues<Vec<String>>) -> Result<Self::Output, clap::Error> {
        Ok(values)
    }
}

/// `-T` / `--no-tool[=NAMES]`: tools to disable, or every tool.
pub(crate) enum DisableTools {}

impl OptionalValue for DisableTools {
    const ID: &'static str = "no_tools";

    type Context = KnownTools;
    type Output = FlagValues<Vec<String>>;
    type Value = Vec<String>;

    fn declare(arg: Arg) -> Arg {
        arg.short('T')
            .long("no-tool")
            .alias("no-tools")
            .action(ArgAction::Append)
            .help("Disable tool(s)")
            .long_help(
                "Disable tool(s).\n\nIf provided without a value, every tool that allows it is \
                 disabled (a tool set to `explicit` or `always` is unaffected), otherwise name \
                 the tools to disable, separated by commas (`--no-tool=read,write`) or across \
                 repeated flags.\n\nThe change applies to this query and every later one on the \
                 conversation; use `--tool` to turn tools back on. To suppress tools for a single \
                 query, use `--no-tool-use`.\n\nFlags are evaluated left-to-right together with \
                 `--tool`.",
            )
    }

    fn parser() -> impl TypedValueParser<Value = Vec<String>> {
        parse_tool_names
    }

    /// A word naming no configured tool starts the query without asking:
    /// disabling every tool grants nothing.
    fn claim(_: &str, parsed: Option<Vec<String>>, tools: &KnownTools) -> Claim<Vec<String>> {
        Claim::value_or_text(known_tools(parsed, tools))
    }

    fn collect(values: FlagValues<Vec<String>>) -> Result<Self::Output, clap::Error> {
        Ok(values)
    }
}

/// `names` when every one of them is a known tool.
fn known_tools(names: Option<Vec<String>>, tools: &KnownTools) -> Option<Vec<String>> {
    names.filter(|names| names.iter().all(|name| tools.contains(name)))
}

/// Parse a comma-separated list of tool names.
fn parse_tool_names(value: &str) -> Result<Vec<String>, String> {
    split_list(value, "tool name")
}

/// Parse the number of turns `--fork` keeps.
fn parse_turns(value: &str) -> Result<usize, &'static str> {
    value.parse().map_err(|_| "expected a positive integer")
}

/// Parse the duration `--tmp` keeps a conversation for.
fn parse_duration(value: &str) -> Result<humantime::Duration, humantime::DurationError> {
    value.parse()
}
