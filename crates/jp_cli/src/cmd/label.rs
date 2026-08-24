//! Label operands and filters for the commands that carry them.
//!
//! `jp conversation label` owns label management: it parses the bare arguments
//! of `add`, `set`, and `rm` into operands, groups them by key, and applies the
//! result under the conversation lock.
//!
//! `--label` on `jp conversation ls` and `jp conversation grep` filters the
//! conversation set instead: every selector must match.
//!
//! Values are taken literally.
//! A value containing a comma needs no escaping, because the shell hands each
//! operand over whole.
//!
//! `:name` is an *alias*: it names a `conversation.labels` rule and resolves to
//! whatever that rule produces.
//! Aliases are only accepted where a single target conversation is known,
//! because a rule resolves against that conversation's effective config.

pub(crate) mod resolve;

use std::str::FromStr;

use indexmap::{IndexMap, IndexSet};
use jp_config::conversation::label;
use jp_conversation::{ConversationId, Labels};
use jp_printer::Printer;

use crate::format::label_text;

/// The prefix marking a label argument as a reference to a configured rule.
const ALIAS_PREFIX: char = ':';

/// Values grouped by key, deduplicated, in the order the keys were named.
pub(crate) type Grouped = IndexMap<String, IndexSet<String>>;

/// A single `KEY[=VALUE]` argument, or a `:NAME` alias awaiting resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelOperand {
    /// `key=value`, or a bare `key` when `value` is `None`.
    ///
    /// A bare key means the empty value under `add` and `set`, and the whole
    /// key under `rm`.
    Pair {
        /// The label key.
        key: String,

        /// The value written after `=`, or `None` for a bare key.
        value: Option<String>,
    },

    /// `:name`: resolve the `conversation.labels.name` rule and use what it
    /// produces.
    Alias(String),
}

impl LabelOperand {
    /// Parse a `KEY[=VALUE]` argument, or `:NAME` when `aliases` is on.
    ///
    /// # Errors
    ///
    /// Returns a message naming the problem when the key is malformed.
    /// With `aliases` off, a leading `:` is an invalid key character rather
    /// than an alias marker.
    pub(crate) fn parse(raw: &str, aliases: bool) -> Result<Self, String> {
        if aliases && let Some(name) = raw.strip_prefix(ALIAS_PREFIX) {
            label::validate_key(name)?;
            return Ok(Self::Alias(name.to_owned()));
        }

        let (key, value) = match raw.split_once('=') {
            Some((key, value)) => (key, Some(value.to_owned())),
            None => (raw, None),
        };
        label::validate_key(key)?;

        Ok(Self::Pair {
            key: key.to_owned(),
            value,
        })
    }

    /// The alias name, when this operand is one.
    pub(crate) fn as_alias(&self) -> Option<&str> {
        match self {
            Self::Alias(name) => Some(name.as_str()),
            Self::Pair { .. } => None,
        }
    }
}

/// Operands with every alias resolved to a concrete key and value.
///
/// The only way to obtain one is [`expand_aliases`], which does the resolving,
/// so an unresolved alias cannot be applied by accident.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Resolved(Vec<(String, Option<String>)>);

impl Resolved {
    /// Group the operands for `add` and `set`, where a bare key names the empty
    /// value.
    ///
    /// Grouping is what makes a verb act on the union of what it was given: `jp
    /// c label set crate=jp_config crate=jp_llm` replaces the key's set once,
    /// with both values, rather than twice.
    pub(crate) fn grouped(&self) -> Grouped {
        let mut grouped = Grouped::new();
        for (key, value) in &self.0 {
            grouped
                .entry(key.clone())
                .or_default()
                .insert(value.clone().unwrap_or_default());
        }

        grouped
    }

    /// Group the operands for `rm`, where a bare key names the whole key.
    ///
    /// A key named bare absorbs the values named for it in the same invocation,
    /// since removing the key removes them too.
    pub(crate) fn grouped_for_removal(&self) -> Grouped {
        let mut grouped = Grouped::new();
        let mut whole_keys = IndexSet::new();

        for (key, value) in &self.0 {
            let values = grouped.entry(key.clone()).or_default();
            match value {
                None => {
                    values.clear();
                    whole_keys.insert(key.clone());
                }
                Some(value) if !whole_keys.contains(key) => {
                    values.insert(value.clone());
                }
                Some(_) => {}
            }
        }

        grouped
    }
}

/// Replace each alias with the key and value its rule resolves to, preserving
/// the order the operands were given in.
///
/// An alias whose confirmation prompt the user declines is dropped.
///
/// # Errors
///
/// Returns an error when a named rule is missing or its command fails; see
/// [`Resolver::alias`].
///
/// [`Resolver::alias`]: resolve::Resolver::alias
pub(crate) async fn expand_aliases(
    operands: &[LabelOperand],
    resolver: &resolve::Resolver<'_>,
) -> crate::error::Result<Resolved> {
    let mut resolved = Vec::with_capacity(operands.len());

    for operand in operands {
        match operand {
            LabelOperand::Pair { key, value } => resolved.push((key.clone(), value.clone())),
            // An alias contributes every value its rule produces to the key's
            // group, which may be none at all.
            LabelOperand::Alias(name) => {
                if let Some((key, values)) = resolver.alias(name).await? {
                    resolved.extend(values.into_iter().map(|value| (key.clone(), Some(value))));
                }
            }
        }
    }

    Ok(Resolved(resolved))
}

/// What one invocation of a mutating verb asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LabelChange {
    /// Insert the values into each named key's set.
    Add(Grouped),

    /// Replace each named key's set with the values.
    Set(Grouped),

    /// Remove the values from each named key; an empty set removes the key
    /// itself.
    Remove(Grouped),

    /// Remove every label the conversation holds.
    RemoveAll,
}

/// What one key held before a mutation, and what it holds after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    /// The label key.
    pub(crate) key: String,

    /// The values the key held.
    pub(crate) before: IndexSet<String>,

    /// The values the key holds now; empty when the key is gone.
    pub(crate) after: IndexSet<String>,
}

/// What applying a change did to one conversation.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Applied {
    /// One entry per key the invocation wrote, in the order the keys were
    /// named; [`LabelChange::RemoveAll`] reports every key the conversation
    /// held.
    ///
    /// Reporting both sides is what makes a mutation recoverable: a `set` that
    /// displaced more than the user expected can be undone from its own output.
    pub(crate) changes: Vec<Change>,

    /// Removal operands that matched nothing, in the order given.
    ///
    /// `None` is a bare key the conversation does not carry; `Some` is a value
    /// the key does not hold.
    pub(crate) missing: Vec<(String, Option<String>)>,
}

/// Apply `change` to a conversation's labels.
///
/// Removal is idempotent, so naming an absent key is not an error, but it is
/// worth telling the user about: a removal that did nothing usually means the
/// key was mistyped or the command targeted a different conversation than
/// intended.
pub(crate) fn apply(labels: &mut Labels, change: &LabelChange) -> Applied {
    let mut applied = Applied::default();

    match change {
        LabelChange::Add(grouped) => {
            for (key, values) in grouped {
                let before = held(labels, key);
                for value in values {
                    labels.insert(key.as_str(), value.as_str());
                }

                applied.changes.push(Change {
                    key: key.clone(),
                    before,
                    after: held(labels, key),
                });
            }
        }
        LabelChange::Set(grouped) => {
            for (key, values) in grouped {
                let before = labels.set(key.as_str(), values.iter().map(String::as_str));

                applied.changes.push(Change {
                    key: key.clone(),
                    before,
                    after: held(labels, key),
                });
            }
        }
        LabelChange::Remove(grouped) => {
            for (key, values) in grouped {
                if values.is_empty() {
                    match labels.remove_key(key) {
                        Some(before) => applied.changes.push(Change {
                            key: key.clone(),
                            before,
                            after: IndexSet::new(),
                        }),
                        None => applied.missing.push((key.clone(), None)),
                    }
                    continue;
                }

                let before = held(labels, key);
                for value in values {
                    if !labels.remove_value(key, value) {
                        applied.missing.push((key.clone(), Some(value.clone())));
                    }
                }

                let after = held(labels, key);
                if before != after {
                    applied.changes.push(Change {
                        key: key.clone(),
                        before,
                        after,
                    });
                }
            }
        }
        LabelChange::RemoveAll => {
            for (key, before) in std::mem::take(labels) {
                applied.changes.push(Change {
                    key,
                    before,
                    after: IndexSet::new(),
                });
            }
        }
    }

    applied
}

/// The values `key` holds, or an empty set when it holds none.
fn held(labels: &Labels, key: &str) -> IndexSet<String> {
    labels.get(key).cloned().unwrap_or_default()
}

/// Report removal operands that matched nothing on `id`.
///
/// Names the conversation so a removal aimed at the wrong one is visible: the
/// reported ID is the conversation actually targeted, which is not always the
/// one the user had in mind.
pub(crate) fn report_missing(
    printer: &Printer,
    id: ConversationId,
    missing: &[(String, Option<String>)],
) {
    for (key, value) in missing {
        let label = value
            .as_deref()
            .map_or_else(|| key.clone(), |value| label_text(key, value));

        printer.eprintln(format!(
            "⚠ Conversation {id} has no label '{label}'; nothing to remove."
        ));
    }
}

/// A single `--label` filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelSelector {
    /// `key=value`: the key must hold this value.
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
pub(crate) fn matches(labels: &Labels, selectors: &[LabelSelector]) -> bool {
    selectors.iter().all(|selector| match selector {
        LabelSelector::Exact { key, value } => labels.contains(key, value),
        LabelSelector::Present(key) => labels.get(key).is_some(),
    })
}

#[cfg(test)]
#[path = "label_tests.rs"]
mod tests;
