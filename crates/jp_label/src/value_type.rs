//! Value types for open vocabulary keys.
//!
//! An open key (`values = []`) accepts arbitrary text by default.
//! When its declaration sets `type`, [`Vocabulary::resolve`] validates every
//! value written under that key against the [`ValueType`].
//!
//! [`Vocabulary::resolve`]: crate::Vocabulary::resolve

use std::fmt;

use chrono::NaiveDate;
use serde::Deserialize;

/// `chrono` format string for [`ValueType::Date`] values (`YYYY-MM-DD`).
///
/// Fixed-width and zero-padded so lexicographic order matches chronological
/// order.
pub(crate) const DATE_FORMAT: &str = "%Y-%m-%d";

/// The shape a [`ValueType::Date`] value has before it is parsed: a digit for
/// each `0`, and the `-` separators where they stand.
const DATE_SHAPE: &[u8] = b"0000-00-00";

/// Expected format for values of an open vocabulary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueType {
    /// Calendar date in zero-padded `YYYY-MM-DD` format.
    Date,
}

impl ValueType {
    /// Type name used in vocabulary declarations (for example, `"date"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Date => "date",
        }
    }

    /// Human-readable description of the expected format (for example, `"a
    /// date, written YYYY-MM-DD"`).
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Date => "a date, written YYYY-MM-DD",
        }
    }

    /// Whether `value` is a valid, canonically formatted value of this type.
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

/// Whether `value` is a valid calendar date in zero-padded `YYYY-MM-DD` format.
///
/// The shape is checked before parsing: `NaiveDate::parse_from_str` accepts
/// unpadded components (`2026-1-5`) and signed or expanded years
/// (`+10000-01-01`), neither of which orders chronologically as text.
pub(crate) fn is_date(value: &str) -> bool {
    let shaped = value.len() == DATE_SHAPE.len()
        && value
            .bytes()
            .zip(DATE_SHAPE)
            .all(|(byte, &shape)| match shape {
                b'-' => byte == b'-',
                _ => byte.is_ascii_digit(),
            });

    shaped && NaiveDate::parse_from_str(value, DATE_FORMAT).is_ok()
}

#[cfg(test)]
#[path = "value_type_tests.rs"]
mod tests;
