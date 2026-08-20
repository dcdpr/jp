//! The label vocabulary: the set of labels a ticket may carry.
//!
//! A board defines its labels in `.labels.json`, next to the ticket files:
//!
//! ```json
//! {
//!   "active": {
//!     "cli": "The command-line surface.",
//!     "config": "Configuration loading and merging."
//!   },
//!   "retired": {
//!     "legacy-ui": "The pre-rewrite terminal UI."
//!   }
//! }
//! ```
//!
//! The set is closed.
//! A label named by neither list is refused, and the refusal names what is on
//! offer — otherwise a board accumulates near-synonyms (`macos`, `mac-os`,
//! `app/macos`) and grouping stops working.
//!
//! Retiring a label is not deleting it.
//! A retired label stays readable and stays writable on a ticket that already
//! carries it, so relabelling an old ticket doesn't force its history to be
//! rewritten; it just can't be added somewhere new.
//! Deleting the entry outright is the other option, and it turns every ticket
//! carrying that label into a build failure.
//!
//! Reading is liberal and writing is strict: a ticket parsed off disk reports
//! whatever labels it carries, but only [`Vocabulary::resolve`] and
//! [`Vocabulary::resolve_against`] produce the [`Label`] a write needs.

use std::{collections::BTreeMap, fmt};

use serde::Deserialize;

/// The vocabulary file, inside the ticket directory.
pub const FILE: &str = ".labels.json";

/// A label the board's vocabulary defines.
///
/// Only [`Vocabulary::resolve`] and [`Vocabulary::resolve_against`] hand one
/// out, and it always carries the vocabulary's own spelling.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Label(String);

impl Label {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The on-disk shape of `.labels.json`.
///
/// Unknown fields are rejected so a file written in some other shape fails
/// loudly instead of parsing as an empty vocabulary and refusing every label.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[serde(default)]
    active: BTreeMap<String, String>,
    #[serde(default)]
    retired: BTreeMap<String, String>,
}

/// The labels a board defines, each with what it covers.
///
/// An empty vocabulary is a board that hasn't defined any: it reads fine and
/// refuses every label.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vocabulary {
    active: BTreeMap<String, String>,
    retired: BTreeMap<String, String>,
}

impl Vocabulary {
    /// Read a vocabulary from the contents of a `.labels.json`.
    ///
    /// # Errors
    ///
    /// Returns an error when the text isn't the documented shape, or when a
    /// label is listed as both active and retired.
    pub fn parse(source: &str) -> Result<Self, Error> {
        // An empty file is an empty vocabulary rather than a syntax error: it
        // is what `touch` leaves behind, and it means the same thing.
        if source.trim().is_empty() {
            return Ok(Self::default());
        }

        let document: Document =
            serde_json::from_str(source).map_err(|error| Error::Malformed(error.to_string()))?;

        let both: Vec<String> = document
            .active
            .keys()
            .filter(|name| document.retired.contains_key(*name))
            .cloned()
            .collect();
        if !both.is_empty() {
            return Err(Error::BothActiveAndRetired(both));
        }

        Ok(Self {
            active: document.active,
            retired: document.retired,
        })
    }

    /// Whether the board defines no labels at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty() && self.retired.is_empty()
    }

    /// The labels a write may add, in alphabetical order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.active.keys().map(String::as_str)
    }

    /// The labels that may stay where they already are but not be added, in
    /// alphabetical order.
    pub fn retired_names(&self) -> impl Iterator<Item = &str> {
        self.retired.keys().map(String::as_str)
    }

    /// What a label covers, active or retired.
    #[must_use]
    pub fn description(&self, name: &str) -> Option<&str> {
        self.active
            .get(name)
            .or_else(|| self.retired.get(name))
            .map(String::as_str)
    }

    /// Check labels for a ticket that carries none yet.
    ///
    /// # Errors
    ///
    /// Returns every label the vocabulary doesn't define and every retired one,
    /// since a new ticket has nothing for a retired label to be kept on.
    pub fn resolve(&self, requested: &[String]) -> Result<Vec<Label>, Rejected> {
        self.resolve_against(requested, &[])
    }

    /// Check labels for a ticket that already carries `current`.
    ///
    /// Matching ignores case and surrounding whitespace, and the result carries
    /// the vocabulary's spelling, so a board's labels read the same on every
    /// ticket.
    /// The result is sorted and deduplicated: a ticket's label line is a set,
    /// and the order it was typed in says nothing.
    ///
    /// A retired label already in `current` resolves; one that isn't is
    /// refused.
    /// That is what lets a label be added to an old ticket without first
    /// stripping the retired labels it happens to carry.
    ///
    /// # Errors
    ///
    /// Returns every rejected label at once, so a caller fixing them doesn't
    /// discover them one at a time.
    pub fn resolve_against(
        &self,
        requested: &[String],
        current: &[String],
    ) -> Result<Vec<Label>, Rejected> {
        let mut resolved = vec![];
        let mut unknown = vec![];
        let mut retired = vec![];

        for name in requested {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }

            if let Some(known) = matching(self.active.keys(), name) {
                resolved.push(Label(known.clone()));
            } else if let Some(known) = matching(self.retired.keys(), name) {
                match matching(current.iter(), known) {
                    Some(_) => resolved.push(Label(known.clone())),
                    None => retired.push(name.to_owned()),
                }
            } else {
                unknown.push(name.to_owned());
            }
        }

        if !unknown.is_empty() || !retired.is_empty() {
            return Err(Rejected {
                unknown,
                retired,
                active: self.names().map(ToOwned::to_owned).collect(),
            });
        }

        resolved.sort();
        resolved.dedup();

        Ok(resolved)
    }
}

/// The entry in `candidates` matching `name`, ignoring case.
fn matching<'a>(
    mut candidates: impl Iterator<Item = &'a String>,
    name: &str,
) -> Option<&'a String> {
    candidates.find(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// A vocabulary file that can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file isn't the documented `{ "active": {}, "retired": {} }` shape.
    Malformed(String),
    /// A label appears in both lists, so nothing can say whether it may be
    /// added.
    BothActiveAndRetired(Vec<String>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(reason) => write!(
                f,
                "The label vocabulary is not an object with `active` and `retired` maps of label \
                 to description: {reason}"
            ),
            Self::BothActiveAndRetired(labels) => write!(
                f,
                "{} listed as both active and retired in the label vocabulary.",
                quoted(labels)
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Labels a write can't apply.
///
/// Carries the addable set as well as the refusals, so the message stands on
/// its own wherever it is printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// Named by neither list.
    pub unknown: Vec<String>,
    /// Retired, and not already on the ticket.
    pub retired: Vec<String>,
    /// The labels a write may add.
    pub active: Vec<String>,
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;

        if !self.unknown.is_empty() {
            let verb = if self.unknown.len() == 1 {
                "is not a known label"
            } else {
                "are not known labels"
            };
            write!(f, "{} {verb}.", quoted(&self.unknown))?;
            first = false;
        }

        if !self.retired.is_empty() {
            if !first {
                f.write_str(" ")?;
            }
            let verb = if self.retired.len() == 1 {
                "is retired and can only stay on a ticket that already carries it"
            } else {
                "are retired and can only stay on tickets that already carry them"
            };
            write!(f, "{} {verb}.", quoted(&self.retired))?;
        }

        if self.active.is_empty() {
            return write!(
                f,
                " This board defines no labels; add them to `{FILE}` in the ticket directory."
            );
        }

        write!(f, " Labels you can add: {}.", self.active.join(", "))
    }
}

impl std::error::Error for Rejected {}

/// Render a list of names as a comma-separated run of backticked values.
fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Split a ticket's `Labels` metadata value into its labels.
///
/// Plain strings rather than [`Label`]s: a ticket read off disk may carry a
/// label the vocabulary no longer defines, and hiding it would make the file
/// and the listing disagree.
#[must_use]
pub fn split(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Render labels as a ticket's `Labels` metadata value.
#[must_use]
pub fn join(labels: &[Label]) -> String {
    labels
        .iter()
        .map(Label::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "labels_tests.rs"]
mod tests;
