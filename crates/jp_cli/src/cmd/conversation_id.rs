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

use std::ffi::OsStr;

use clap::{Arg, ArgAction, ArgMatches, Command, FromArgMatches, builder::TypedValueParser};

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
    pub ids: Vec<ConversationTarget>,
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
    pub ids: Vec<ConversationTarget>,
}

impl<const SESSION: bool, const MULTI: bool> clap::Args for PositionalIds<SESSION, MULTI> {
    fn augment_args(cmd: Command) -> Command {
        let mut arg = Arg::new("id")
            .value_parser(TargetParser::<SESSION>)
            .help(short_help(SESSION))
            .long_help(long_help(SESSION));

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
            .help(short_help(SESSION))
            .long_help(long_help(SESSION))
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

        let target = ConversationTarget::parse(s)
            .map_err(|e| clap::Error::raw(clap::error::ErrorKind::InvalidValue, e.to_string()))?;

        if !SESSION && matches!(target, ConversationTarget::Session) {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::InvalidValue,
                "the 'session' keyword is not supported by this command\n",
            ));
        }

        Ok(target)
    }
}

fn short_help(session: bool) -> &'static str {
    if session {
        "Conversation ID, keyword, or session target"
    } else {
        "Conversation ID or keyword"
    }
}

fn long_help(session: bool) -> String {
    let header = if session {
        "Conversation ID, keyword, or session target."
    } else {
        "Conversation ID or keyword."
    };
    let mut s = format!("{header}\n\nKeywords:\n");
    s.push_str("  last, last-active, l     Most recently activated conversation\n");
    s.push_str("  last-created             Most recently created conversation\n");
    s.push_str("  previous, prev, p        Session's previously active conversation\n");
    s.push_str("  current, c               Current session's active conversation\n");
    if session {
        s.push_str("  session                  All conversations in current session\n");
    }
    s.push_str("  pinned                   Pick from pinned conversations only\n");
    s.push_str("\nWhen multiple IDs are given, only literal conversation IDs are accepted.");
    s
}

#[cfg(test)]
#[path = "conversation_id_tests.rs"]
mod tests;
