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
use jp_conversation::ConversationId;
use jp_printer::Printer;

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

/// Reject a label key that is spelled like a conversation ID.
///
/// The ID grammar (`jp-c…`) is a subset of the label-key grammar, so a
/// mistyped `jp c label --no-label <ID>` binds the ID as a key, leaves no
/// positional target, and silently retargets the session's active conversation.
/// A bare conversation ID is meaningless as a key — it carries no value — so
/// refusing it costs nothing and turns that mistake into an error.
/// The ID is still fine as a label *value* (`related=jp-c…`).
///
/// This is a CLI-level rule, not part of the key grammar: a config file has no
/// positional argument to swallow, so `conversation.labels` is unaffected.
fn reject_conversation_id(key: &str) -> Result<(), String> {
    if key.parse::<ConversationId>().is_ok() {
        return Err(format!(
            "'{key}' is a conversation ID, not a label key: a bare ID carries no value, and \
             accepting it here would swallow the conversation this command was meant to \
             target.\n\nTo label a conversation *with* an ID, give it a key: \
             `--label=related={key}`.\nTo target that conversation, put the ID before the flag: \
             `jp conversation label {key} --no-label=KEY`."
        ));
    }

    Ok(())
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
        reject_conversation_id(key)?;

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
        reject_conversation_id(raw)?;
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

/// The ordered label directives of a single invocation.
///
/// Three switches decide which flags a command registers:
///
/// - `REMOVABLE`: `--no-label` / `--no-labels`.
///   Off for commands that only set.
/// - `ALIASES`: `--label=:name`.
///   Off for commands that may target several conversations, since a rule
///   resolves against one conversation's config.
/// - `RAW`: `--raw-label`, which takes its value literally.
///   Off everywhere the escape isn't reachable in practice.
///
/// | Command   | `REMOVABLE` | `ALIASES` | `RAW` |
/// | --------- | ----------- | --------- | ----- |
/// | `c label` | yes         | yes       | yes   |
/// | `query`   | no          | yes       | no    |
/// | `c edit`  | yes         | no        | no    |
/// | `c fork`  | no          | no        | no    |
#[derive(Debug, Clone, Default)]
pub(crate) struct LabelDirectives<const REMOVABLE: bool, const ALIASES: bool, const RAW: bool>(
    pub(crate) Vec<LabelDirective>,
);

impl<const REMOVABLE: bool, const ALIASES: bool, const RAW: bool> std::ops::Deref
    for LabelDirectives<REMOVABLE, ALIASES, RAW>
{
    type Target = [LabelDirective];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const REMOVABLE: bool, const ALIASES: bool, const RAW: bool> clap::FromArgMatches
    for LabelDirectives<REMOVABLE, ALIASES, RAW>
{
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        let mut indexed = vec![];
        let set = LabelDirective::set::<ALIASES>;

        indexed.extend(collect(matches, "label", Split::OnComma, set)?);
        if RAW {
            indexed.extend(collect(matches, "raw_label", Split::No, set)?);
        }
        if REMOVABLE {
            indexed.extend(collect(
                matches,
                "no_label",
                Split::OnComma,
                LabelDirective::remove,
            )?);
        }

        // Stable, so the entries a single `--label` expanded to keep their
        // left-to-right order within that flag's position.
        indexed.sort_by_key(|(index, _)| *index);
        Ok(Self(indexed.into_iter().map(|(_, d)| d).collect()))
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

/// Whether a flag's value carries one directive or a comma-separated list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Split {
    /// The value is one directive; `,` is part of it.
    No,

    /// The value is a list of directives separated by `,`.
    OnComma,
}

/// Pair each directive from `id` with its position on the command line, parsing
/// it with `parse`.
///
/// Under [`Split::OnComma`] one value expands to several directives that share
/// the flag's position; the caller's stable sort keeps them in order.
/// An empty value is never split, so a bare `--no-label` still means "remove
/// every label" rather than expanding to nothing.
fn collect(
    matches: &ArgMatches,
    id: &str,
    split: Split,
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

    let mut collected = Vec::with_capacity(values.len());
    for (value, index) in values.into_iter().zip(indices) {
        let parts: Vec<&str> = if split == Split::OnComma && !value.is_empty() {
            value.split(',').collect()
        } else {
            vec![value.as_str()]
        };

        for part in parts {
            let directive = parse(part)
                .map_err(|error| Error::raw(ErrorKind::InvalidValue, format!("{error}\n")))?;
            collected.push((index, directive));
        }
    }

    Ok(collected)
}

impl<const REMOVABLE: bool, const ALIASES: bool, const RAW: bool> clap::Args
    for LabelDirectives<REMOVABLE, ALIASES, RAW>
{
    fn augment_args(cmd: clap::Command) -> clap::Command {
        let alias_help = if ALIASES {
            "\n\n`:name` resolves the `conversation.labels.name` rule and applies whatever it \
             produces."
        } else {
            ""
        };
        let value_name = if ALIASES {
            "KEY[=VALUE]|:NAME,..."
        } else {
            "KEY[=VALUE],..."
        };
        let raw_hint = if RAW {
            "\n\nUse `--raw-label` for a value that contains a comma."
        } else {
            ""
        };

        let mut cmd = cmd.arg(
            clap::Arg::new("label")
                .long("label")
                .alias("labels")
                .value_name(value_name)
                .help("Set labels on the conversation, separated by commas")
                .long_help(format!(
                    "Set labels on the conversation.\n\nAccepts `key=value`, or a bare `key` for \
                     an empty value. Several are separated by commas, and the flag can also be \
                     repeated; the last value wins when a key is repeated.\n\nLabel keys may \
                     contain ASCII letters, digits, underscores, and \
                     hyphens.{alias_help}{raw_hint}"
                ))
                .action(ArgAction::Append)
                .num_args(1),
        );

        if RAW {
            cmd = cmd.arg(
                clap::Arg::new("raw_label")
                    .long("raw-label")
                    .value_name("KEY=VALUE")
                    .help("Set one label, taking the value literally")
                    .long_help(
                        "Set one label, taking the value literally.\n\nIdentical to `--label` \
                         except that the value is never split, so it can contain commas. Repeat \
                         the flag to set several.\n\nMostly useful from scripts; label values \
                         rarely contain commas.",
                    )
                    .action(ArgAction::Append)
                    .num_args(1),
            );
        }

        if !REMOVABLE {
            return cmd;
        }

        cmd.arg(
            clap::Arg::new("no_label")
                .long("no-label")
                .alias("no-labels")
                .value_name("KEY,...")
                .help("Remove labels from the conversation, separated by commas")
                .long_help(
                    "Remove labels from the conversation.\n\nWith one or more comma-separated \
                     keys, removes those labels; without a value, removes every label. Evaluated \
                     left-to-right together with `--label`.\n\nRemoval names keys, and a key can \
                     never contain a comma, so there is no literal form of this flag.",
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

impl<const REMOVABLE: bool, const RAW: bool> LabelDirectives<REMOVABLE, false, RAW> {
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
///
/// Returns the keys named by a `--no-label` directive that matched nothing, in
/// the order they were given.
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

/// Report `--no-label` keys that matched nothing on `id`.
///
/// Names the conversation so a removal aimed at the wrong one is visible: the
/// reported ID is the conversation actually targeted, which is not always the
/// one the user typed.
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
            reject_conversation_id(s)?;
            return Ok(Self::Present(s.to_owned()));
        };

        label::validate_key(key)?;
        reject_conversation_id(key)?;
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
