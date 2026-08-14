//! CLI parsing for the `--label` and `--no-label` flags.
//!
//! Two shapes share the flag name:
//!
//! - On mutating commands ([`LabelDirectives`]), `--label` sets a label and
//!   `--no-label` removes one (or all).
//!   Directives are applied in command-line order, so the last one wins per
//!   key.
//! - On listing commands ([`LabelSelector`]), `--label` filters the
//!   conversation set: every selector must match.
//!
//! `--label=:name` is an *alias*: it names a `conversation.labels` rule and
//! resolves to whatever that rule produces.
//! Aliases are only accepted where a single target conversation is known,
//! because a rule resolves against that conversation's effective config.

pub(crate) mod resolve;

use std::{collections::BTreeMap, str::FromStr};

use clap::{ArgAction, ArgMatches, Error, error::ErrorKind};
use jp_config::conversation::label;

/// The prefix marking a `--label` value as a reference to a configured rule.
const ALIAS_PREFIX: char = ':';

/// A single label mutation, in command-line order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelDirective {
    /// Set `key` to `value`.
    /// A bare `--label=key` sets an empty value.
    Set { key: String, value: String },

    /// Remove a single label.
    Remove(String),

    /// Remove every label on the conversation.
    RemoveAll,

    /// Resolve the named `conversation.labels` rule and set its result.
    Alias(String),
}

impl LabelDirective {
    fn set<const ALIASES: bool>(raw: &str) -> Result<Self, String> {
        if let Some(name) = raw.strip_prefix(ALIAS_PREFIX) {
            if !ALIASES {
                return Err(format!(
                    "label alias ':{name}' is not supported here, because this command may target \
                     several conversations and a rule resolves against one conversation's config; \
                     use `jp conversation label <ID> --label=:{name}`"
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

    fn remove(raw: &str) -> Result<Self, String> {
        if raw.is_empty() {
            return Ok(Self::RemoveAll);
        }

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

/// The ordered `--label` / `--no-label` directives of a single invocation.
///
/// `REMOVABLE` registers `--no-label`; commands that only set labels leave it
/// off so the flag is rejected with an unknown-argument error.
/// `ALIASES` accepts `--label=:name`; commands that may target several
/// conversations leave it off, since a rule resolves against one conversation's
/// config.
#[derive(Debug, Clone, Default)]
pub(crate) struct LabelDirectives<const REMOVABLE: bool, const ALIASES: bool>(
    pub(crate) Vec<LabelDirective>,
);

impl<const REMOVABLE: bool, const ALIASES: bool> std::ops::Deref
    for LabelDirectives<REMOVABLE, ALIASES>
{
    type Target = [LabelDirective];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const REMOVABLE: bool, const ALIASES: bool> clap::FromArgMatches
    for LabelDirectives<REMOVABLE, ALIASES>
{
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        let mut indexed = vec![];

        indexed.extend(collect(matches, "labels", LabelDirective::set::<ALIASES>)?);
        if REMOVABLE {
            indexed.extend(collect(matches, "no_labels", LabelDirective::remove)?);
        }

        indexed.sort_by_key(|(index, _)| *index);
        Ok(Self(indexed.into_iter().map(|(_, d)| d).collect()))
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

/// Pair each value of `id` with its position on the command line, parsing it
/// with `parse`.
fn collect(
    matches: &ArgMatches,
    id: &str,
    parse: fn(&str) -> Result<LabelDirective, String>,
) -> Result<Vec<(usize, LabelDirective)>, Error> {
    let values: Vec<String> = matches
        .get_many(id)
        .map(|v| v.cloned().collect())
        .unwrap_or_default();
    let indices: Vec<usize> = matches
        .indices_of(id)
        .map(Iterator::collect)
        .unwrap_or_default();

    values
        .into_iter()
        .zip(indices)
        .map(|(value, index)| {
            parse(&value)
                .map(|directive| (index, directive))
                .map_err(|error| Error::raw(ErrorKind::InvalidValue, format!("{error}\n")))
        })
        .collect()
}

impl<const REMOVABLE: bool, const ALIASES: bool> clap::Args
    for LabelDirectives<REMOVABLE, ALIASES>
{
    fn augment_args(cmd: clap::Command) -> clap::Command {
        let alias_help = if ALIASES {
            "\n\n`--label=:name` resolves the `conversation.labels.name` rule and applies whatever \
             it produces."
        } else {
            ""
        };
        let value_name = if ALIASES {
            "KEY[=VALUE]|:NAME"
        } else {
            "KEY[=VALUE]"
        };

        let cmd = cmd.arg(
            clap::Arg::new("labels")
                .long("label")
                .value_name(value_name)
                .help("Set a label on the conversation")
                .long_help(format!(
                    "Set a label on the conversation.\n\nAccepts `key=value`, or a bare `key` for \
                     an empty value. Repeat the flag to set several labels; the last value wins \
                     when a key is repeated.\n\nLabel keys may contain ASCII letters, digits, \
                     underscores, and hyphens.{alias_help}"
                ))
                .action(ArgAction::Append)
                .num_args(1),
        );

        if !REMOVABLE {
            return cmd;
        }

        cmd.arg(
            clap::Arg::new("no_labels")
                .long("no-label")
                .value_name("KEY")
                .help("Remove a label from the conversation")
                .long_help(
                    "Remove a label from the conversation.\n\nWith a key, removes that label; \
                     without one, removes every label. Evaluated left-to-right together with \
                     `--label`.",
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

impl<const REMOVABLE: bool> LabelDirectives<REMOVABLE, false> {
    /// These directives are alias-free by construction: the parser rejects
    /// `:name` when `ALIASES` is off.
    pub(crate) fn resolved(&self) -> Resolved {
        Resolved(self.0.clone())
    }
}

/// Replace each alias directive with the `key=value` its rule resolves to,
/// preserving command-line order.
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
pub(crate) fn apply(labels: &mut BTreeMap<String, String>, directives: &Resolved) {
    for directive in &directives.0 {
        match directive {
            LabelDirective::Set { key, value } => {
                labels.insert(key.clone(), value.clone());
            }
            LabelDirective::Remove(key) => {
                labels.remove(key);
            }
            LabelDirective::RemoveAll => labels.clear(),
            // `Resolved` can only be built from alias-free directives.
            LabelDirective::Alias(_) => unreachable!("alias in resolved directives"),
        }
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
