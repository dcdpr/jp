//! The label vocabulary: which keys exist, and which values each accepts.
//!
//! A consumer that wants free-form labels doesn't need this module.
//! One that wants a closed set declares it, and resolves user input through
//! [`Vocabulary::resolve`], which is the only thing that turns a string into a
//! [`Labels`] a write can trust.
//!
//! The declaration is a map of keys to entries, read from JSON with
//! [`Vocabulary::parse`] or from an already parsed value with
//! [`Vocabulary::from_value`]:
//!
//! ```json
//! {
//!   "client": {
//!     "description": "The client surface the work lands in.",
//!     "values": ["cli", "macos", "web"]
//!   },
//!   "package": {
//!     "description": "The crate the work lands in.",
//!     "values": ["jp_cli", "jp_config"],
//!     "retired": ["jp_legacy"]
//!   }
//! }
//! ```
//!
//! A key with values can bound how many of them one item carries, with `min`
//! and `max`:
//!
//! ```json
//! {
//!   "type": {
//!     "description": "What kind of work it is.",
//!     "values": ["bug", "feature"],
//!     "min": 1,
//!     "max": 1
//!   }
//! }
//! ```
//!
//! Every label write is held to them, so an item can be required to carry a
//! key, or held to one value of it.
//! A key without values is present or absent, and takes neither.
//!
//! A key with an empty list of values takes any value, such as an RFD number:
//!
//! ```json
//! { "rfd": { "description": "The RFD the work comes from.", "values": [] } }
//! ```
//!
//! Leaving `values` out makes a bare key instead, which takes none.
//!
//! A key can also be held to one item across a collection, with `unique`:
//! `"key"` lets one item carry the key at all, and `"key-value"` lets one item
//! carry each of its values.
//! A collection is the consumer's, so [`Vocabulary::clashes`] compares two
//! items, and the consumer compares an item against the rest.
//!
//! Retiring a value is not deleting it.
//! A retired value stays readable and stays writable on an item that already
//! carries it, so relabelling something old doesn't force its history to be
//! rewritten; it just can't be added somewhere new.
//! Deleting the entry outright is the other option, and it turns every item
//! carrying that value into a validation failure.
//!
//! Reading is liberal and writing is strict: an item parsed off disk reports
//! whatever labels it carries, but only [`Vocabulary::resolve`] and
//! [`Vocabulary::resolve_against`] produce a set a write will accept.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::Deserialize;
use serde_json::Value;

use crate::{KeyError, Labels, parse_token, validate_key, validate_value};

/// The on-disk shape of one key's entry.
///
/// Unknown fields are rejected so a file written in some other shape fails
/// loudly instead of parsing as an empty facet and refusing every value.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    #[serde(default)]
    description: String,
    /// Absent for a key that takes no value, empty for one that takes any.
    #[serde(default)]
    values: Option<BTreeSet<String>>,
    #[serde(default)]
    retired: BTreeSet<String>,
    #[serde(default)]
    min: Option<usize>,
    #[serde(default)]
    max: Option<usize>,
    #[serde(default)]
    unique: Option<Unique>,
}

/// How much of a key one item of a collection holds on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unique {
    /// One item carries the key, whatever its value.
    Key,
    /// One item carries each value of the key.
    KeyValue,
}

impl Unique {
    /// The spelling a declaration uses: `key` or `key-value`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::KeyValue => "key-value",
        }
    }
}

impl fmt::Display for Unique {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One key of a vocabulary: what it means, and what it accepts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facet {
    description: String,
    active: BTreeSet<String>,
    retired: BTreeSet<String>,
    /// Whether the key takes any value, declared as `values = []`.
    open: bool,
    min: usize,
    max: Option<usize>,
    unique: Option<Unique>,
}

impl Facet {
    /// What this key covers.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The values a write may add, in order.
    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.active.iter().map(String::as_str)
    }

    /// The values that may stay where they already are but not be added, in
    /// order.
    pub fn retired(&self) -> impl Iterator<Item = &str> {
        self.retired.iter().map(String::as_str)
    }

    /// Whether this key accepts no values at all, making it a bare-only key.
    #[must_use]
    pub fn is_bare(&self) -> bool {
        !self.open && self.active.is_empty() && self.retired.is_empty()
    }

    /// Whether this key takes any value rather than a listed one.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// How much of this key one item of a collection holds on its own, or
    /// `None` when any number of items may carry it.
    #[must_use]
    pub fn unique(&self) -> Option<Unique> {
        self.unique
    }

    /// The fewest values of this key an item carries; `0` when it may carry
    /// none.
    #[must_use]
    pub fn min(&self) -> usize {
        self.min
    }

    /// The most values of this key an item carries, or `None` for no limit.
    #[must_use]
    pub fn max(&self) -> Option<usize> {
        self.max
    }

    /// Whether `min` or `max` holds an item to some number of this key's
    /// values.
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.min > 0 || self.max.is_some()
    }

    /// How many of this key's values an item carries, in words: `exactly one
    /// label`, `at least 2 labels`, `at most one label`.
    ///
    /// `None` for a key with no bounds.
    #[must_use]
    pub fn bound(&self) -> Option<String> {
        bound(self.min, self.max)
    }
}

/// `min` and `max` in words, or `None` when neither holds an item to anything.
fn bound(min: usize, max: Option<usize>) -> Option<String> {
    let labels = |count: usize| match count {
        1 => "one label".to_owned(),
        count => format!("{count} labels"),
    };

    match (min, max) {
        (0, None) => None,
        (min, Some(max)) if min == max => Some(format!("exactly {}", labels(min))),
        (0, Some(max)) => Some(format!("at most {}", labels(max))),
        (min, None) => Some(format!("at least {}", labels(min))),
        (min, Some(max)) => Some(format!("between {min} and {max} labels")),
    }
}

/// Check a facet asks for something an item, and a collection of them, can
/// meet.
fn check(key: &str, facet: &Facet) -> Result<(), Error> {
    let key = key.to_owned();
    if facet.open && !facet.retired.is_empty() {
        return Err(Error::RetiredOnOpenKey { key });
    }
    if facet.is_bare() && facet.is_bounded() {
        return Err(Error::BoundOnBareKey { key });
    }

    let min = facet.min;
    match facet.max {
        Some(0) => return Err(Error::ZeroMax { key }),
        Some(max) if min > max => return Err(Error::MinAboveMax { key, min, max }),
        _ => {}
    }

    // A retired value can't be added, so only the active ones can make up the
    // count on a new item.
    if !facet.open && min > facet.active.len() {
        return Err(Error::MinAboveValues {
            key,
            min,
            values: facet.active.len(),
        });
    }

    if facet.unique == Some(Unique::Key) && min > 0 {
        return Err(Error::UniqueRequired { key, min });
    }

    Ok(())
}

/// The keys a consumer defines, each with the values it accepts.
///
/// An empty vocabulary declares no closed set, so it accepts every well-formed
/// label as written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vocabulary(BTreeMap<String, Facet>);

impl Vocabulary {
    /// Read a vocabulary from JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when the text isn't the documented shape, when a key or
    /// value can't be used as written, or when two entries differ only by case.
    pub fn parse(source: &str) -> Result<Self, Error> {
        // An empty file is an empty vocabulary rather than a syntax error: it
        // is what `touch` leaves behind, and it means the same thing.
        if source.trim().is_empty() {
            return Ok(Self::default());
        }

        let document: BTreeMap<String, Entry> =
            serde_json::from_str(source).map_err(|error| Error::Malformed(error.to_string()))?;

        Self::validated(document)
    }

    /// Read a vocabulary from a value already parsed, such as a table of a
    /// configuration file.
    ///
    /// # Errors
    ///
    /// Returns an error when the value isn't the documented shape, when a key
    /// or value can't be used as written, or when two entries differ only by
    /// case.
    pub fn from_value(value: Value) -> Result<Self, Error> {
        let document: BTreeMap<String, Entry> =
            serde_json::from_value(value).map_err(|error| Error::Malformed(error.to_string()))?;

        Self::validated(document)
    }

    /// Check a declaration and build the vocabulary it declares.
    fn validated(document: BTreeMap<String, Entry>) -> Result<Self, Error> {
        let mut facets = BTreeMap::new();
        let mut seen_keys: BTreeMap<String, String> = BTreeMap::new();

        for (key, entry) in document {
            validate_key(&key).map_err(Error::Name)?;

            // Two keys differing only by case would make the canonical
            // spelling depend on sort order rather than on intent.
            if let Some(existing) = seen_keys.insert(key.to_ascii_lowercase(), key.clone()) {
                return Err(Error::DuplicateKey {
                    first: existing,
                    second: key,
                });
            }

            let open = entry.values.as_ref().is_some_and(BTreeSet::is_empty);
            let values = entry.values.unwrap_or_default();

            let mut seen_values: BTreeMap<String, String> = BTreeMap::new();
            for value in values.iter().chain(&entry.retired) {
                validate_value(value).map_err(Error::Name)?;
                if value.is_empty() {
                    return Err(Error::EmptyValue { key: key.clone() });
                }

                // Covers both within-list and active-versus-retired
                // collisions: a value in both lists has no answer to "may this
                // be added?".
                if let Some(existing) =
                    seen_values.insert(value.to_ascii_lowercase(), value.clone())
                {
                    return Err(Error::DuplicateValue {
                        key: key.clone(),
                        first: existing,
                        second: value.clone(),
                    });
                }
            }

            let facet = Facet {
                description: entry.description,
                active: values,
                retired: entry.retired,
                open,
                min: entry.min.unwrap_or(0),
                max: entry.max,
                unique: entry.unique,
            };
            check(&key, &facet)?;

            facets.insert(key, facet);
        }

        Ok(Self(facets))
    }

    /// Whether the vocabulary defines no keys at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every key, in order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// One key's entry.
    #[must_use]
    pub fn facet(&self, key: &str) -> Option<&Facet> {
        self.0.get(key)
    }

    /// Every key, paired with its entry, in order.
    pub fn facets(&self) -> impl Iterator<Item = (&str, &Facet)> {
        self.0.iter().map(|(key, facet)| (key.as_str(), facet))
    }

    /// The keys that take any value, in order.
    ///
    /// None of their values are in [`Vocabulary::tokens`], which can only list
    /// what is declared.
    pub fn open_keys(&self) -> impl Iterator<Item = &str> {
        self.facets()
            .filter(|(_, facet)| facet.open)
            .map(|(key, _)| key)
    }

    /// Every addable `key=value` token, in order.
    ///
    /// This is what a consumer advertises to a caller that needs the set up
    /// front, such as a JSON Schema `enum`.
    /// A key that takes any value contributes none; see
    /// [`Vocabulary::open_keys`].
    #[must_use]
    pub fn tokens(&self) -> Vec<String> {
        self.tokens_with(false)
    }

    /// Every token a write may name, including retired values.
    ///
    /// A caller replacing an item's whole label set has to be able to name a
    /// retired value to keep it, so an interface that only offers
    /// [`Vocabulary::tokens`] cannot express "keep what is already there".
    #[must_use]
    pub fn tokens_including_retired(&self) -> Vec<String> {
        self.tokens_with(true)
    }

    fn tokens_with(&self, retired: bool) -> Vec<String> {
        let mut tokens = vec![];
        for (key, facet) in &self.0 {
            if facet.open {
                continue;
            }
            if facet.is_bare() {
                tokens.push(key.clone());
                continue;
            }

            let extra = if retired {
                facet.retired.iter().collect::<Vec<_>>()
            } else {
                vec![]
            };
            for value in facet.active.iter().chain(extra) {
                tokens.push(format!("{key}={value}"));
            }
        }
        tokens.sort();

        tokens
    }

    /// Check tokens for an item that carries no labels yet.
    ///
    /// # Errors
    ///
    /// Returns every token the vocabulary doesn't define and every retired one,
    /// since a new item has nothing for a retired value to be kept on.
    pub fn resolve(&self, requested: &[String]) -> Result<Labels, Rejected> {
        self.resolve_against(requested, &Labels::default())
    }

    /// Check tokens for an item that already carries `current`.
    ///
    /// Matching ignores case and surrounding whitespace, and the result carries
    /// the vocabulary's own spelling, so labels read the same on every item.
    ///
    /// A retired value already in `current` resolves; one that isn't is
    /// refused.
    /// That is what lets a label be added to an old item without first
    /// stripping the retired values it happens to carry.
    ///
    /// The tokens are the item's whole set, so each key's `min` and `max` are
    /// checked against them.
    ///
    /// A key that takes any value keeps the value as given.
    /// An empty vocabulary accepts every well-formed token, spelled as given.
    /// Neither checks `unique`, which needs the rest of the collection; see
    /// [`Vocabulary::clashes`].
    ///
    /// # Errors
    ///
    /// Returns every rejected token at once, so a caller fixing them doesn't
    /// discover them one at a time.
    pub fn resolve_against(
        &self,
        requested: &[String],
        current: &Labels,
    ) -> Result<Labels, Rejected> {
        if self.is_empty() {
            return free_form(requested);
        }

        // Staged in the vocabulary's own order rather than the caller's, so a
        // resolved set is the same whichever order the tokens arrived in and a
        // relabel produces no spurious diff.
        let mut accepted: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        let mut malformed = vec![];
        let mut unknown_keys = vec![];
        let mut unknown_values = vec![];
        let mut retired = vec![];

        for token in requested {
            let Ok((key, value)) = parse_token(token) else {
                malformed.push(token.clone());
                continue;
            };

            let Some((key, facet)) = self.matching_key(&key) else {
                unknown_keys.push(key);
                continue;
            };

            let Some(value) = value else {
                if facet.is_bare() {
                    // A bare label records its key's presence with the empty
                    // value, which is how `Labels` stores it.
                    accepted.entry(key).or_default().insert(String::new());
                } else {
                    unknown_values.push(key.to_owned());
                }
                continue;
            };

            if let Some(known) = matching(facet.active.iter(), &value) {
                accepted.entry(key).or_default().insert(known.to_owned());
            } else if let Some(known) = matching(facet.retired.iter(), &value) {
                if current.contains(key, known) {
                    accepted.entry(key).or_default().insert(known.to_owned());
                } else {
                    retired.push(format!("{key}={known}"));
                }
            } else if facet.open {
                accepted.entry(key).or_default().insert(value);
            } else {
                unknown_values.push(format!("{key}={value}"));
            }
        }

        let miscounted = self.miscounted(&accepted);

        if !malformed.is_empty()
            || !unknown_keys.is_empty()
            || !unknown_values.is_empty()
            || !retired.is_empty()
            || !miscounted.is_empty()
        {
            return Err(Rejected {
                malformed,
                unknown_keys,
                unknown_values,
                retired,
                miscounted,
                addable: self.tokens(),
                open_keys: self.open_keys().map(ToOwned::to_owned).collect(),
            });
        }

        Ok(accepted.into_iter().collect())
    }

    /// The labels of `labels` that `other` carries too, among the keys this
    /// vocabulary makes unique.
    ///
    /// Comparing an item against every other item of a collection is how a
    /// consumer holds a collection to `unique`.
    /// `other` is read as written, so keys and values match without case.
    #[must_use]
    pub fn clashes(&self, labels: &Labels, other: &Labels) -> Vec<Clash> {
        let mut clashes = vec![];

        for (key, facet) in &self.0 {
            let Some(unique) = facet.unique else {
                continue;
            };
            let mine = values_under(labels, key);
            let theirs = values_under(other, key);
            if mine.is_empty() || theirs.is_empty() {
                continue;
            }

            // A bare key has no values to tell apart, so it is unique as a
            // whole either way.
            if unique == Unique::Key || facet.is_bare() {
                clashes.push(Clash {
                    key: key.clone(),
                    value: None,
                });
                continue;
            }

            for value in mine {
                if theirs
                    .iter()
                    .any(|theirs| theirs.eq_ignore_ascii_case(value))
                {
                    clashes.push(Clash {
                        key: key.clone(),
                        value: Some(value.to_owned()),
                    });
                }
            }
        }

        clashes
    }

    /// The bounded keys `accepted` carries too few or too many values of.
    ///
    /// Counted on what resolved, so a value the vocabulary refused counts as
    /// missing rather than as given.
    fn miscounted(&self, accepted: &BTreeMap<&str, BTreeSet<String>>) -> Vec<Miscount> {
        self.0
            .iter()
            .filter(|(_, facet)| facet.is_bounded())
            .filter_map(|(key, facet)| {
                let given: Vec<String> = accepted
                    .get(key.as_str())
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect();
                let fits =
                    given.len() >= facet.min && facet.max.is_none_or(|max| given.len() <= max);

                (!fits).then(|| Miscount {
                    key: key.clone(),
                    min: facet.min,
                    max: facet.max,
                    given,
                    values: facet.active.iter().cloned().collect(),
                    open: facet.open,
                })
            })
            .collect()
    }

    /// The vocabulary's own spelling of a key, matched without case.
    fn matching_key(&self, key: &str) -> Option<(&str, &Facet)> {
        self.0
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(key))
            .map(|(known, facet)| (known.as_str(), facet))
    }
}

/// The values `labels` holds under `key`, matching the key without case.
fn values_under<'a>(labels: &'a Labels, key: &str) -> Vec<&'a str> {
    labels
        .iter()
        .filter(|(known, _)| known.eq_ignore_ascii_case(key))
        .flat_map(|(_, values)| values.iter().map(String::as_str))
        .collect()
}

/// A label an item can't carry because another item of its collection already
/// does, and the vocabulary lets one item hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clash {
    /// The key the vocabulary makes unique, as the vocabulary spells it.
    pub key: String,
    /// The value both items carry, for a key each of whose values is unique;
    /// `None` for a key unique as a whole.
    pub value: Option<String>,
}

impl fmt::Display for Clash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.value {
            Some(value) => write!(f, "{}={value}", self.key),
            None => f.write_str(&self.key),
        }
    }
}

/// Every token in `requested`, as written: what a vocabulary that declares no
/// keys accepts.
///
/// Staged sorted, as a vocabulary's resolution is, so the result is the same
/// whichever order the tokens arrived in.
fn free_form(requested: &[String]) -> Result<Labels, Rejected> {
    let mut accepted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut malformed = vec![];

    for token in requested {
        match parse_token(token) {
            Ok((key, value)) => {
                accepted
                    .entry(key)
                    .or_default()
                    .insert(value.unwrap_or_default());
            }
            Err(_) => malformed.push(token.clone()),
        }
    }

    if !malformed.is_empty() {
        return Err(Rejected {
            malformed,
            unknown_keys: vec![],
            unknown_values: vec![],
            retired: vec![],
            miscounted: vec![],
            addable: vec![],
            open_keys: vec![],
        });
    }

    let mut labels = Labels::default();
    for (key, values) in accepted {
        for value in values {
            labels.insert(key.clone(), value);
        }
    }

    Ok(labels)
}

/// The entry in `candidates` matching `name`, ignoring case.
fn matching<'a>(mut candidates: impl Iterator<Item = &'a String>, name: &str) -> Option<&'a str> {
    candidates
        .find(|candidate| candidate.eq_ignore_ascii_case(name))
        .map(String::as_str)
}

/// A vocabulary declaration that can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The declaration isn't a map of key to entry.
    Malformed(String),
    /// A key or value can't be used as written.
    Name(KeyError),
    /// A key holds an empty value, which nothing can name.
    EmptyValue { key: String },
    /// Two keys differ only by case.
    DuplicateKey { first: String, second: String },
    /// Two values under one key differ only by case, or one value appears as
    /// both active and retired.
    DuplicateValue {
        key: String,
        first: String,
        second: String,
    },
    /// `min` or `max` on a key that takes no value.
    BoundOnBareKey { key: String },
    /// `max = 0`, which no item carrying the key could meet.
    ZeroMax { key: String },
    /// `min` above `max`.
    MinAboveMax { key: String, min: usize, max: usize },
    /// `min` above the number of values the key defines.
    MinAboveValues {
        key: String,
        min: usize,
        values: usize,
    },
    /// `retired` on a key that takes any value.
    RetiredOnOpenKey { key: String },
    /// `unique = "key"` on a key every item has to carry.
    UniqueRequired { key: String, min: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(reason) => write!(
                f,
                "The label vocabulary is not a map of each key to a `description`, `values`, and \
                 the optional `retired`, `min`, and `max`: {reason}"
            ),
            Self::Name(error) => write!(f, "The label vocabulary is unusable: {error}."),
            Self::EmptyValue { key } => {
                write!(f, "`{key}` in the label vocabulary holds an empty value.")
            }
            Self::DuplicateKey { first, second } => write!(
                f,
                "`{first}` and `{second}` in the label vocabulary differ only by case."
            ),
            Self::DuplicateValue { key, first, second } => write!(
                f,
                "`{first}` and `{second}` under `{key}` in the label vocabulary are the same \
                 value."
            ),
            Self::BoundOnBareKey { key } => write!(
                f,
                "`{key}` in the label vocabulary takes no value, so it has no `min` or `max`."
            ),
            Self::ZeroMax { key } => write!(
                f,
                "`{key}` in the label vocabulary has `max = 0`, which no item carrying it meets; \
                 retire its values instead."
            ),
            Self::MinAboveMax { key, min, max } => write!(
                f,
                "`{key}` in the label vocabulary has `min = {min}` above `max = {max}`."
            ),
            Self::MinAboveValues { key, min, values } => write!(
                f,
                "`{key}` in the label vocabulary has `min = {min}`, but only {values} values to \
                 choose from."
            ),
            Self::RetiredOnOpenKey { key } => write!(
                f,
                "`{key}` in the label vocabulary takes any value (`values = []`), so it has \
                 nothing to retire. To retire every value of a key, list them under `retired` and \
                 leave `values` out."
            ),
            Self::UniqueRequired { key, min } => write!(
                f,
                "`{key}` in the label vocabulary is required on every item (`min = {min}`) but \
                 allowed on one (`unique = \"key\"`), which no two items can meet."
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Name(error) => Some(error),
            _ => None,
        }
    }
}

/// Tokens a write can't apply.
///
/// Carries the addable set as well as the refusals, so the message stands on
/// its own wherever it is printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// Not a `key` or `key=value` token at all.
    pub malformed: Vec<String>,
    /// A key the vocabulary doesn't define.
    pub unknown_keys: Vec<String>,
    /// A value the key doesn't accept, or a missing value on a key that needs
    /// one.
    pub unknown_values: Vec<String>,
    /// Retired, and not already on the item.
    pub retired: Vec<String>,
    /// Keys the item would carry too few or too many values of.
    pub miscounted: Vec<Miscount>,
    /// The tokens a write may add.
    pub addable: Vec<String>,
    /// The keys a write may add with any value.
    pub open_keys: Vec<String>,
}

/// A bounded key an item would carry too few or too many values of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Miscount {
    pub key: String,
    pub min: usize,
    pub max: Option<usize>,
    /// The values of the key the item would carry.
    pub given: Vec<String>,
    /// The values the key accepts, so a refusal for too few can offer them.
    pub values: Vec<String>,
    /// Whether the key takes any value, so there is no list to offer.
    pub open: bool,
}

impl fmt::Display for Miscount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = &self.key;
        let bound = bound(self.min, self.max).unwrap_or_default();
        let tokens = |values: &[String]| {
            values
                .iter()
                .map(|value| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", ")
        };

        if self.given.len() >= self.min {
            return write!(
                f,
                "`{key}` takes {bound}, and {} are given: {}.",
                self.given.len(),
                tokens(&self.given)
            );
        }

        let (one, more) = if self.open {
            let any = format!("`{key}=<value>`, with any value");
            (format!("add one, as {any}"), format!("add more, as {any}"))
        } else {
            let offered = tokens(&self.values);
            (
                format!("add one of {offered}"),
                format!("add from {offered}"),
            )
        };
        match self.given.len() {
            0 if self.min == 1 => write!(f, "`{key}` label missing: {one}."),
            0 => write!(f, "`{key}` takes {bound}, and none is given: {more}."),
            1 => write!(f, "`{key}` takes {bound}, and 1 is given: {more}."),
            count => write!(f, "`{key}` takes {bound}, and {count} are given: {more}."),
        }
    }
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        let mut sentence = |f: &mut fmt::Formatter<'_>, text: String| {
            if !first {
                f.write_str(" ")?;
            }
            first = false;
            f.write_str(&text)
        };

        if !self.malformed.is_empty() {
            sentence(
                f,
                format!("{} is not a `key=value` label.", quoted(&self.malformed)),
            )?;
        }
        if !self.unknown_keys.is_empty() {
            sentence(
                f,
                format!("{} is not a known label key.", quoted(&self.unknown_keys)),
            )?;
        }
        if !self.unknown_values.is_empty() {
            sentence(
                f,
                format!("{} is not a known label.", quoted(&self.unknown_values)),
            )?;
        }
        if !self.retired.is_empty() {
            sentence(
                f,
                format!(
                    "{} is retired and can only stay on an item that already carries it.",
                    quoted(&self.retired)
                ),
            )?;
        }

        // The whole set is offered only for a label the vocabulary refused; a
        // miscount offers the values of its own key.
        let refused = !self.malformed.is_empty()
            || !self.unknown_keys.is_empty()
            || !self.unknown_values.is_empty()
            || !self.retired.is_empty();
        if refused && !(self.addable.is_empty() && self.open_keys.is_empty()) {
            let mut offered = self.addable.join(", ");
            if !self.open_keys.is_empty() {
                let open = self
                    .open_keys
                    .iter()
                    .map(|key| format!("{key}=<value>"))
                    .collect::<Vec<_>>()
                    .join(", ");
                offered = if offered.is_empty() {
                    format!("{open}, with any value")
                } else {
                    format!("{offered}, and {open} with any value")
                };
            }
            sentence(f, format!("Labels you can add: {offered}."))?;
        }

        for miscount in &self.miscounted {
            sentence(f, miscount.to_string())?;
        }

        Ok(())
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

#[cfg(test)]
#[path = "vocabulary_tests.rs"]
mod tests;
