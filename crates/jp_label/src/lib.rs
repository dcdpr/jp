//! Labels: `key=value` annotations on a thing, and the vocabularies that
//! constrain them.
//!
//! A label is a key holding an ordered set of values.
//! A key holding the empty value is a *bare* label, and a filter reads it as
//! "key present, any value".
//! A key holding several values is one label with several values, not several
//! labels: `crate=jp_cli` and `crate=jp_config` on the same item are two values
//! of `crate`.
//!
//! ```
//! use jp_label::Labels;
//!
//! let mut labels = Labels::default();
//! labels.insert("crate", "jp_cli");
//! labels.insert("crate", "jp_config");
//! labels.insert("draft", "");
//!
//! assert_eq!(labels.to_tokens(), [
//!     "crate=jp_cli",
//!     "crate=jp_config",
//!     "draft"
//! ]);
//! ```
//!
//! [`Vocabulary`] is the optional other half: a declaration of which keys exist
//! and which values each accepts.
//! A consumer that wants free-form labels ignores it; one that wants a closed
//! set resolves through it, which is what turns a user-supplied string into a
//! [`Labels`] a write can trust.
//!
//! This crate owns the *shape* of a label, the rules a key must satisfy, and
//! the on-disk contract for a label set.
//! It deliberately owns no storage location: where a label set is persisted,
//! and when it is applied, belong to each consumer.

use std::fmt;

mod labels;
pub mod vocabulary;

pub use labels::Labels;
pub use vocabulary::{Rejected, Vocabulary};

/// The label key grammar, in words.
///
/// Every excluded character is significant somewhere a key is used: `.`
/// separates dotted config paths, `=` splits a key from its value, `,` and `:`
/// are CLI separators.
/// The leading character is narrower still, because a key that starts with `-`
/// would be read as a flag where keys are written as bare arguments.
pub const KEY_GRAMMAR: &str = "a label key starts with an ASCII letter, followed by any number of \
                               letters, digits, underscores, and hyphens";

/// A key or value that can't be used as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The key is empty.
    EmptyKey,
    /// The key doesn't match [`KEY_GRAMMAR`].
    Key { key: String, character: char },
    /// The key doesn't start with an ASCII letter.
    KeyStart { key: String, character: char },
    /// The value carries something that wouldn't survive being written and read
    /// back.
    Value { value: String, reason: &'static str },
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyKey => f.write_str("label key must not be empty"),
            Self::KeyStart { key, character } => {
                write!(
                    f,
                    "label key '{key}' starts with '{character}': {KEY_GRAMMAR}"
                )
            }
            Self::Key { key, character } => {
                write!(
                    f,
                    "invalid character '{character}' in label key '{key}': {KEY_GRAMMAR}"
                )
            }
            Self::Value { value, reason } => {
                write!(f, "label value '{value}' {reason}")
            }
        }
    }
}

impl std::error::Error for KeyError {}

/// Check a key against [`KEY_GRAMMAR`].
///
/// # Errors
///
/// Returns an error when the key is empty, starts with something other than an
/// ASCII letter, or carries a character outside the grammar.
pub fn validate_key(key: &str) -> Result<(), KeyError> {
    let mut chars = key.chars();

    let Some(first) = chars.next() else {
        return Err(KeyError::EmptyKey);
    };

    if !first.is_ascii_alphabetic() {
        return Err(KeyError::KeyStart {
            key: key.to_owned(),
            character: first,
        });
    }

    if let Some(character) = chars.find(|c| !c.is_ascii_alphanumeric() && *c != '_' && *c != '-') {
        return Err(KeyError::Key {
            key: key.to_owned(),
            character,
        });
    }

    Ok(())
}

/// Check a value is usable as written, for a consumer declaring one up front.
///
/// [`Labels`] itself is forgiving: it folds line breaks into spaces as a value
/// is stored, so nothing a caller supplies can break the one-label-one-line
/// rule.
/// A *declaration* is different — a vocabulary naming a value that would be
/// folded on the way in could never match what it declared, so the mismatch is
/// refused where it is written.
///
/// # Errors
///
/// Returns an error when the value differs from its trimmed form, or carries a
/// line break or other control character.
pub fn validate_value(value: &str) -> Result<(), KeyError> {
    let reason = if value != value.trim() {
        Some("has leading or trailing whitespace")
    } else if value.chars().any(char::is_control) {
        Some("contains a line break or control character")
    } else {
        None
    };

    match reason {
        Some(reason) => Err(KeyError::Value {
            value: value.to_owned(),
            reason,
        }),
        None => Ok(()),
    }
}

/// Split a `key=value` token into its parts.
///
/// A token with no `=` is a bare key.
/// Only the first `=` splits, so a value may contain more of them.
///
/// # Errors
///
/// Returns an error when the key can't be used as written.
pub fn parse_token(token: &str) -> Result<(String, Option<String>), KeyError> {
    let token = token.trim();

    let (key, value) = match token.split_once('=') {
        Some((key, value)) => (key.trim(), Some(value.trim())),
        None => (token, None),
    };

    validate_key(key)?;

    Ok((
        key.to_owned(),
        value.filter(|v| !v.is_empty()).map(ToOwned::to_owned),
    ))
}

/// One term of a label filter.
///
/// A bare key matches any value; a pair matches exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub key: String,
    pub value: Option<String>,
}

impl Selector {
    /// Read a selector from a `key` or `key=value` token.
    ///
    /// # Errors
    ///
    /// Returns an error when the key can't be used as written.
    pub fn parse(token: &str) -> Result<Self, KeyError> {
        let (key, value) = parse_token(token)?;

        Ok(Self { key, value })
    }

    /// Read several selectors.
    ///
    /// # Errors
    ///
    /// Returns an error on the first token whose key can't be used as written.
    pub fn parse_all<I, S>(tokens: I) -> Result<Vec<Self>, KeyError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        tokens
            .into_iter()
            .map(|token| Self::parse(token.as_ref()))
            .collect()
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.value {
            Some(value) => write!(f, "{}={value}", self.key),
            None => f.write_str(&self.key),
        }
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
