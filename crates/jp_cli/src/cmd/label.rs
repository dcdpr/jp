//! Label directives and filters for the commands that carry label flags.
//!
//! `jp conversation label` owns label management and takes its keys as bare
//! arguments, so it needs no flags.
//! What lives here serves the commands whose argument slot is already spoken
//! for:
//!
//! - `--label` on `jp query` and `jp conversation fork` sets one label, and is
//!   repeatable.
//!   `jp conversation fork` additionally has `--reset-labels`, which drops
//!   everything accumulated up to that point.
//! - `--label` on `jp conversation ls` and `jp conversation grep` filters the
//!   conversation set: every selector must match.
//!
//! Values are taken literally.
//! A label containing a comma needs no escaping, because one flag carries one
//! label.
//!
//! `:name` is an *alias*: it names a `conversation.labels` rule and resolves to
//! whatever that rule produces.
//! Aliases are only accepted where a single target conversation is known,
//! because a rule resolves against that conversation's effective config.

pub(crate) mod resolve;

use std::{collections::BTreeMap, str::FromStr};

use clap::{ArgAction, ArgMatches, Error, error::ErrorKind};
use jp_config::conversation::label;
use jp_conversation::ConversationId;
use jp_printer::Printer;

/// The prefix marking a label argument as a reference to a configured rule.
const ALIAS_PREFIX: char = ':';

/// A single label mutation, in the order it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelDirective {
    /// Set `key` to `value`.
    /// A bare `key` sets an empty value.
    Set { key: String, value: String },

    /// Remove a single label.
    Remove(String),

    /// Remove every label accumulated so far.
    RemoveAll,

    /// Resolve the named `conversation.labels` rule and set its result.
    Alias(String),
}

impl LabelDirective {
    /// Parse a `KEY[=VALUE]` argument, or `:NAME` when `ALIASES` is on.
    ///
    /// # Errors
    ///
    /// Returns a message naming the problem when the key is malformed, or when
    /// an alias is given where the command cannot resolve one.
    pub(crate) fn parse_set<const ALIASES: bool>(raw: &str) -> Result<Self, String> {
        if let Some(name) = raw.strip_prefix(ALIAS_PREFIX) {
            if !ALIASES {
                return Err(format!(
                    "label alias ':{name}' is not supported here, because this command may target \
                     several conversations and a rule resolves against one conversation's config; \
                     use `jp conversation label add :{name}`"
                ));
            }

            label::validate_key(name)?;
            return Ok(Self::Alias(name.to_owned()));
        }

        let (key, value) = raw.split_once('=').unwrap_or((raw, ""));
        label::validate_key(key)?;

        Ok(Self::Set {
            key: key.to_owned(),
            value: value.to_owned(),
        })
    }

    /// Parse a bare `KEY` argument for removal.
    ///
    /// # Errors
    ///
    /// Returns a message naming the problem when the key is malformed.
    pub(crate) fn parse_remove(raw: &str) -> Result<Self, String> {
        label::validate_key(raw)?;
        Ok(Self::Remove(raw.to_owned()))
    }

    /// The alias name, when this directive is one.
    pub(crate) fn as_alias(&self) -> Option<&str> {
        match self {
            Self::Alias(name) => Some(name.as_str()),
            _ => None,
        }
    }
}

/// The ordered label flags of a single invocation.
///
/// Two switches decide which flags a command registers:
///
/// - `ALIASES`: whether `--label=:name` is accepted.
///   Off for commands that may target several conversations, since a rule
///   resolves against one conversation's config.
/// - `RESET`: whether `--reset-labels` is registered.
///
/// | Command  | `ALIASES` | `RESET` |
/// | -------- | --------- | ------- |
/// | `query`  | yes       | no      |
/// | `c fork` | no        | yes     |
#[derive(Debug, Clone, Default)]
pub(crate) struct LabelDirectives<const ALIASES: bool, const RESET: bool>(
    pub(crate) Vec<LabelDirective>,
);

impl<const ALIASES: bool, const RESET: bool> std::ops::Deref for LabelDirectives<ALIASES, RESET> {
    type Target = [LabelDirective];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const ALIASES: bool, const RESET: bool> clap::FromArgMatches
    for LabelDirectives<ALIASES, RESET>
{
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        let values: Vec<String> = matches
            .get_many("label")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        let indices: Vec<usize> = matches
            .indices_of("label")
            .map(Iterator::collect)
            .unwrap_or_default();

        let mut indexed = Vec::with_capacity(values.len());
        for (value, index) in values.into_iter().zip(indices) {
            let directive = LabelDirective::parse_set::<ALIASES>(&value)
                .map_err(|error| Error::raw(ErrorKind::InvalidValue, format!("{error}\n")))?;
            indexed.push((index, directive));
        }

        // `--reset-labels` is positioned like any other directive, so
        // `--label=a=1 --reset-labels --label=b=2` keeps only `b`.
        if RESET && matches.get_flag("reset_labels") {
            for index in matches.indices_of("reset_labels").into_iter().flatten() {
                indexed.push((index, LabelDirective::RemoveAll));
            }
        }

        indexed.sort_by_key(|(index, _)| *index);
        Ok(Self(indexed.into_iter().map(|(_, d)| d).collect()))
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl<const ALIASES: bool, const RESET: bool> clap::Args for LabelDirectives<ALIASES, RESET> {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        let alias_help = if ALIASES {
            "\n\n`:name` resolves the `conversation.labels.name` rule and applies whatever it \
             produces."
        } else {
            ""
        };
        let value_name = if ALIASES {
            "KEY[=VALUE]|:NAME"
        } else {
            "KEY[=VALUE]"
        };

        let cmd = cmd.arg(
            clap::Arg::new("label")
                .long("label")
                .value_name(value_name)
                .help("Set a label on the conversation")
                .long_help(format!(
                    "Set a label on the conversation.\n\nAccepts `key=value`, or a bare `key` for \
                     an empty value. The value is taken literally, so it may contain commas. \
                     Repeat the flag to set several labels; the last value wins when a key is \
                     repeated.\n\nA label key starts with a letter, followed by any number of \
                     letters, digits, underscores, and hyphens.{alias_help}\n\nUse `jp \
                     conversation label` to manage labels on an existing conversation."
                ))
                .action(ArgAction::Append)
                .num_args(1),
        );

        if !RESET {
            return cmd;
        }

        cmd.arg(
            clap::Arg::new("reset_labels")
                .long("reset-labels")
                .help("Drop every label inherited from the source conversation")
                .long_help(
                    "Drop every label accumulated so far.\n\nA fork inherits the source \
                     conversation's labels by default. This flag discards them, along with any \
                     `--label` given before it, so `--reset-labels --label=a=1` produces exactly \
                     one label.",
                )
                .action(ArgAction::SetTrue),
        )
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

/// Directives with every alias already resolved to a concrete `key=value`.
///
/// The only ways to obtain one are [`LabelDirectives::resolved`], for commands
/// whose parser rejects aliases outright, and [`expand_aliases`], which does
/// the resolving.
/// That makes it impossible to apply an unresolved alias by accident.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Resolved(Vec<LabelDirective>);

impl std::ops::Deref for Resolved {
    type Target = [LabelDirective];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const RESET: bool> LabelDirectives<false, RESET> {
    /// These directives are alias-free by construction: the parser rejects
    /// `:name` when `ALIASES` is off.
    pub(crate) fn resolved(&self) -> Resolved {
        Resolved(self.0.clone())
    }
}

/// Replace each alias directive with the `key=value` its rule resolves to,
/// preserving the order they were given in.
///
/// A directive whose confirmation prompt the user declines is dropped.
///
/// # Errors
///
/// Returns an error when a named rule is missing or its command fails; see
/// [`Resolver::alias`].
///
/// [`Resolver::alias`]: resolve::Resolver::alias
pub(crate) async fn expand_aliases(
    directives: &[LabelDirective],
    resolver: &resolve::Resolver<'_>,
) -> crate::error::Result<Resolved> {
    let mut expanded = Vec::with_capacity(directives.len());

    for directive in directives {
        let Some(name) = directive.as_alias() else {
            expanded.push(directive.clone());
            continue;
        };

        if let Some((key, value)) = resolver.alias(name).await? {
            expanded.push(LabelDirective::Set { key, value });
        }
    }

    Ok(Resolved(expanded))
}

/// Apply `directives` to a conversation's label set, in order.
///
/// Returns the keys named for removal that matched nothing, in the order they
/// were given.
/// Removal is idempotent, so this is not an error, but it is worth telling the
/// user about: a removal that did nothing usually means the key was mistyped or
/// the command targeted a different conversation than intended.
pub(crate) fn apply(labels: &mut BTreeMap<String, String>, directives: &Resolved) -> Vec<String> {
    let mut missing = vec![];

    for directive in &directives.0 {
        match directive {
            LabelDirective::Set { key, value } => {
                labels.insert(key.clone(), value.clone());
            }
            LabelDirective::Remove(key) => {
                if labels.remove(key).is_none() {
                    missing.push(key.clone());
                }
            }
            LabelDirective::RemoveAll => labels.clear(),
            // `Resolved` can only be built from alias-free directives.
            LabelDirective::Alias(_) => unreachable!("alias in resolved directives"),
        }
    }

    missing
}

/// Report removal keys that matched nothing on `id`.
///
/// Names the conversation so a removal aimed at the wrong one is visible: the
/// reported ID is the conversation actually targeted, which is not always the
/// one the user had in mind.
pub(crate) fn report_missing(printer: &Printer, id: ConversationId, missing: &[String]) {
    for key in missing {
        printer.eprintln(format!(
            "⚠ Conversation {id} has no label '{key}'; nothing to remove."
        ));
    }
}

/// A single `--label` filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelSelector {
    /// `key=value`: the label must be present with exactly this value.
    Exact { key: String, value: String },

    /// `key`: the label must be present, with any value.
    Present(String),
}

impl FromStr for LabelSelector {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Filters read persisted label values; a rule name has nothing to
        // resolve against here, so point at the resolved syntax instead of
        // failing on the `:` character.
        if let Some(name) = s.strip_prefix(ALIAS_PREFIX) {
            return Err(format!(
                "label alias ':{name}' cannot be used as a filter: filters match the labels \
                 stored on a conversation, not `conversation.labels` rules; filter on the \
                 resolved value instead (`--label={name}=VALUE`, or `--label={name}` for any \
                 value)"
            ));
        }

        let Some((key, value)) = s.split_once('=') else {
            label::validate_key(s)?;
            return Ok(Self::Present(s.to_owned()));
        };

        label::validate_key(key)?;
        Ok(Self::Exact {
            key: key.to_owned(),
            value: value.to_owned(),
        })
    }
}

/// Whether `labels` satisfies every selector.
///
/// Every selector must match; an empty selector list matches everything.
pub(crate) fn matches(labels: &BTreeMap<String, String>, selectors: &[LabelSelector]) -> bool {
    selectors.iter().all(|selector| match selector {
        LabelSelector::Exact { key, value } => labels.get(key).is_some_and(|v| v == value),
        LabelSelector::Present(key) => labels.contains_key(key),
    })
}

#[cfg(test)]
#[path = "label_tests.rs"]
mod tests;
