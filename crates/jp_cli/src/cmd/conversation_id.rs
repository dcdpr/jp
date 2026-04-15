//! Shared clap argument types for conversation targeting.
//!
//! Two generic types handle all commands:
//!
//! - [`PositionalIds`]: positional `[ID]...` arguments
//! - [`FlagIds`]: `--id/-i` flag arguments
//!
//! Both are parameterized by const generics:
//!
//! - `SESSION`: whether the `session` keyword is accepted
//! - `MULTI`: whether multiple IDs are accepted
//!
//! Because clap's derive macro doesn't evaluate const generics in `#[arg]`
//! attributes, both types implement [`clap::Args`] and [`clap::FromArgMatches`]
//! manually. The const generics control parser selection, help text, and
//! validation.

use std::{borrow::Cow, ffi::OsStr};

use clap::{Arg, ArgAction, ArgMatches, Command, FromArgMatches, builder::TypedValueParser};
use crossterm::style::Stylize as _;

use super::target::ConversationTarget;

/// Positional conversation ID arguments: `[ID]` or `[ID]...`
///
/// # Type parameters
///
/// - `SESSION`: accept the `session` keyword (resolves to all session
///   conversations)
/// - `MULTI`: accept multiple IDs (when false, at most one is allowed)
#[derive(Debug, Default)]
pub(crate) struct PositionalIds<const SESSION: bool, const MULTI: bool> {
    ids: Vec<ConversationTarget>,
}

#[cfg(test)]
impl<const SESSION: bool, const MULTI: bool> PositionalIds<SESSION, MULTI> {
    pub fn from_targets(ids: Vec<ConversationTarget>) -> Self {
        Self { ids }
    }
}

/// Flag-based conversation ID arguments: `-i/--id/--ids`
///
/// Always supports bare `--id` (no value) for the interactive picker. When
/// `MULTI` is true, supports comma-separated values and repeated flags.
///
/// # Type parameters
///
/// - `SESSION`: accept the `session` keyword
/// - `MULTI`: accept multiple IDs via comma separation or repeated `--id`
#[derive(Debug, Default)]
pub(crate) struct FlagIds<const SESSION: bool, const MULTI: bool> {
    ids: Vec<ConversationTarget>,
}

#[cfg(test)]
impl<const SESSION: bool, const MULTI: bool> FlagIds<SESSION, MULTI> {
    pub fn from_targets(ids: Vec<ConversationTarget>) -> Self {
        Self { ids }
    }
}

/// Common interface for conversation ID argument types.
pub(crate) trait ConversationIds {
    /// The parsed conversation targets.
    fn ids(&self) -> &[ConversationTarget];

    /// Whether this argument type accepts multiple targets.
    fn is_multi(&self) -> bool;
}

impl<const SESSION: bool, const MULTI: bool> ConversationIds for PositionalIds<SESSION, MULTI> {
    fn ids(&self) -> &[ConversationTarget] {
        &self.ids
    }

    fn is_multi(&self) -> bool {
        MULTI
    }
}

impl<const SESSION: bool, const MULTI: bool> ConversationIds for FlagIds<SESSION, MULTI> {
    fn ids(&self) -> &[ConversationTarget] {
        &self.ids
    }

    fn is_multi(&self) -> bool {
        MULTI
    }
}

impl<const SESSION: bool, const MULTI: bool> clap::Args for PositionalIds<SESSION, MULTI> {
    fn augment_args(cmd: Command) -> Command {
        let mut arg = Arg::new("id")
            .value_parser(TargetParser::<SESSION>)
            .help(short_help())
            .long_help(long_help(SESSION, MULTI));

        if MULTI {
            arg = arg.action(ArgAction::Append);
        } else {
            arg = arg.num_args(0..=1);
        }

        cmd.arg(arg)
    }

    fn augment_args_for_update(cmd: Command) -> Command {
        Self::augment_args(cmd)
    }
}

impl<const SESSION: bool, const MULTI: bool> FromArgMatches for PositionalIds<SESSION, MULTI> {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let ids = read_ids(matches);
        validate_multi::<MULTI>(&ids)?;
        Ok(Self { ids })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        self.ids = read_ids(matches);
        validate_multi::<MULTI>(&self.ids)?;
        Ok(())
    }
}

impl<const SESSION: bool, const MULTI: bool> clap::Args for FlagIds<SESSION, MULTI> {
    fn augment_args(cmd: Command) -> Command {
        let mut arg = Arg::new("id")
            .short('i')
            .long("id")
            .visible_alias("ids")
            .value_parser(TargetParser::<SESSION>)
            .help(short_help())
            .long_help(long_help(SESSION, MULTI))
            .num_args(0..=1)
            .default_missing_value("");

        if MULTI {
            arg = arg.action(ArgAction::Append).value_delimiter(',');
        }

        cmd.arg(arg)
    }

    fn augment_args_for_update(cmd: Command) -> Command {
        Self::augment_args(cmd)
    }
}

impl<const SESSION: bool, const MULTI: bool> FromArgMatches for FlagIds<SESSION, MULTI> {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let ids = read_ids(matches);
        validate_multi::<MULTI>(&ids)?;
        Ok(Self { ids })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        self.ids = read_ids(matches);
        validate_multi::<MULTI>(&self.ids)?;
        Ok(())
    }
}

/// Read parsed [`ConversationTarget`] values from the arg matches.
fn read_ids(matches: &ArgMatches) -> Vec<ConversationTarget> {
    matches
        .get_many::<ConversationTarget>("id")
        .map(|vals| vals.cloned().collect())
        .unwrap_or_default()
}

/// When multiple values are provided, only literal conversation IDs are
/// allowed — keywords like `last`, `session`, etc. are rejected.
fn validate_multi<const MULTI: bool>(ids: &[ConversationTarget]) -> Result<(), clap::Error> {
    if !MULTI && ids.len() > 1 {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::TooManyValues,
            "this command accepts at most one conversation ID\n",
        ));
    }

    if ids.len() > 1 {
        for target in ids {
            if let Some(kw) = target.keyword_name() {
                return Err(clap::Error::raw(
                    clap::error::ErrorKind::InvalidValue,
                    format!("keywords are not supported when multiple IDs are given; got '{kw}'\n"),
                ));
            }
        }
    }

    Ok(())
}

/// A clap [`TypedValueParser`] for [`ConversationTarget`].
///
/// The `SESSION` const generic controls whether the `session` keyword is
/// accepted.
#[derive(Clone)]
struct TargetParser<const SESSION: bool>;

impl<const SESSION: bool> TypedValueParser for TargetParser<SESSION> {
    type Value = ConversationTarget;

    fn parse_ref(
        &self,
        _cmd: &Command,
        _arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let s = value
            .to_str()
            .ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;

        let target = ConversationTarget::parse(s);

        if !SESSION && target.requires_session() {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::InvalidValue,
                "session-based targets are not supported by this command\n",
            ));
        }

        Ok(target)
    }
}

fn short_help() -> &'static str {
    "Conversation ID or keyword"
}

fn long_help(session: bool, multi: bool) -> String {
    let header = "Conversation ID, Interactive Filter/Picker, Alias, or Multi-Target Keyword.";
    let mut s = format!("{header}\n\n");
    s.push_str(&keyword_help(session, false));

    if multi {
        s.push_str("\nWhen multiple IDs are given, only literal conversation IDs are accepted.");
    }

    s
}

fn keyword_help(session: bool, ansi: bool) -> String {
    let t = |text: &'static str| -> Cow<'static, str> {
        if !ansi {
            return text.into();
        }

        text.yellow().bold().to_string().into()
    };

    let h = |text: &'static str| -> Cow<'static, str> {
        if !ansi {
            return text.into();
        }

        format!("# {text}").dim().to_string().into()
    };

    let picker = t("Interactive Filter/Picker");
    let aliases = t("Conversation Aliases");
    let multi_target = t("Multi-Target Keywords");

    let h_pick_all = h("select from all");
    let h_pick_pinned = h("select from pinned");
    let h_pick_session = h("select from session");

    let h_alias_newest = h("target newest created");
    let h_alias_latest = h("target latest active in workspace");
    let h_alias_pinned = h("target latest pinned");
    let h_alias_session = h("target previous active in session");

    let h_multi_session = h("target all activated in session");
    let h_multi_pinned = h("target all pinned");

    let help = indoc::formatdoc! {"
        {picker}:
          ?                             {h_pick_all}
          ?p, ?pinned                   {h_pick_pinned}
          ?s, ?session                  {h_pick_session}

        {aliases}:
          n, newest                     {h_alias_newest}
          l, latest                     {h_alias_latest}
          p, pinned                     {h_alias_pinned}
          s, session                    {h_alias_session}

        {multi_target}:
          +s, +session                  {h_multi_session}
          +p, +pinned                   {h_multi_pinned}
    "};

    if session {
        return help;
    }

    // Strip session-related lines for commands that don't support it.
    help.lines()
        .filter(|l| !l.contains("session"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn format_target_help(session: bool, ansi: bool) -> String {
    let mut header: Cow<'_, str> = "Conversation Targeting".into();
    if ansi {
        header = header.bold().to_string().into();
    }

    let mut example_id: Cow<'_, str> = "jp-c17761673600".into();
    if ansi {
        example_id = example_id.bold().to_string().into();
    }

    indoc::formatdoc! {"
        {header}

        Use a conversation ID (e.g. {example_id}), a keyword, or any text to
        fuzzy-search by title.

        {}
    ", keyword_help(session, ansi)}
}

#[cfg(test)]
#[path = "conversation_id_tests.rs"]
mod tests;
