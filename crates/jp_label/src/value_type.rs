//! The kinds of value a vocabulary key can hold its values to.
//!
//! A key that takes any value takes free text unless its entry names a `type`,
//! which [`Vocabulary::resolve`] then holds every value written under the key
//! to.
//!
//! [`Vocabulary::resolve`]: crate::Vocabulary::resolve

use std::fmt;

use chrono::NaiveDate;
use serde::Deserialize;

/// How a date is written: the only spelling a date-typed value takes.
///
/// Fixed-width and zero-padded, so two dates order the same way as text.
pub(crate) const DATE_FORMAT: &str = "%Y-%m-%d";

/// The kind of value a key holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueType {
    /// A calendar date, written `YYYY-MM-DD`.
    Date,
}

impl ValueType {
    /// The spelling a declaration uses: `date`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Date => "date",
        }
    }

    /// What a value of this type looks like, in words: `a date, written
    /// YYYY-MM-DD`.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Date => "a date, written YYYY-MM-DD",
        }
    }

    /// Whether `value` is a value of this type, spelled the one way the type
    /// spells it.
    #[must_use]
    pub fn accepts(self, value: &str) -> bool {
        match self {
            Self::Date => is_date(value),
        }
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether `value` is a calendar date written `YYYY-MM-DD`.
///
/// Only the zero-padded spelling is a date: `2026-1-5` parses, but would order
/// after `2026-10-01` as text, so it is refused.
pub(crate) fn is_date(value: &str) -> bool {
    NaiveDate::parse_from_str(value, DATE_FORMAT)
        .is_ok_and(|date| date.format(DATE_FORMAT).to_string() == value)
}

#[cfg(test)]
#[path = "value_type_tests.rs"]
mod tests;
