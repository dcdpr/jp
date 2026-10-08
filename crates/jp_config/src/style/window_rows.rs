//! How many rows of output a progress indicator shows above its status row.
//!
//! The rows form a window holding the most recent output of whatever feeds the
//! indicator, such as the stderr of an MCP server that is still starting.
//! They are erased with the status row and never join the transcript.
//!
//! ```toml
//! [style.mcp_startup]
//! stderr_rows = "auto"   # false | true | N
//! ```
//!
//! This is the size of the window, which is screen space and therefore shared:
//! there is one window per indicator, however many sources feed it.

use std::{fmt, num::ParseIntError};

use schematic::ConfigEnum;
use serde::{Deserialize, Serialize};

/// How many rows of output to show above a progress indicator's status row.
///
/// - `false` or `0`: show no output, only the status row.
/// - `true`: size the window from the terminal height (a tenth of it).
/// - `N`: show exactly `N` rows.
///
/// The count is shared across every source feeding the indicator rather than
/// being one window each: `1` is a single row that each source replaces with
/// its latest line.
/// A terminal whose height cannot be determined shows no output whatever the
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum WindowRows {
    /// Show no output; the indicator is its status row alone.
    #[default]
    #[serde(alias = "false")]
    Off,

    /// Size the window from the terminal's height.
    #[serde(alias = "true")]
    Auto,

    /// Show exactly this many rows.
    #[variant(fallback)]
    Fixed(RowCount),
}

impl WindowRows {
    /// Whether any output rows are shown.
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

impl From<bool> for WindowRows {
    /// `false` shows the status row alone; `true` sizes the window from the
    /// terminal.
    fn from(v: bool) -> Self {
        if v { Self::Auto } else { Self::Off }
    }
}

/// A fixed number of output rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RowCount {
    /// Rows to show.
    pub rows: u16,
}

impl TryFrom<&str> for RowCount {
    type Error = ParseIntError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse().map(|rows| Self { rows })
    }
}

impl fmt::Display for RowCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.rows)
    }
}

impl<'de> Deserialize<'de> for WindowRows {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct WindowRowsVisitor;

        impl<'de> serde::de::Visitor<'de> for WindowRowsVisitor {
            type Value = WindowRows;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a boolean, a string (\"off\", \"auto\"), or a row count")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(WindowRows::from(v))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v {
                    "off" | "false" => Ok(WindowRows::Off),
                    "auto" | "true" => Ok(WindowRows::Auto),
                    s => s.parse::<u16>().map(fixed).map_err(|_| {
                        serde::de::Error::unknown_variant(v, &["off", "auto", "a number"])
                    }),
                }
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                u16::try_from(v).map(fixed).map_err(|_| {
                    serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &"a number")
                })
            }

            // TOML hands every integer to `visit_i64`, whatever its sign.
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                u16::try_from(v).map(fixed).map_err(|_| {
                    serde::de::Error::invalid_value(serde::de::Unexpected::Signed(v), &"a number")
                })
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                // Reuse the derived deserializer for the tagged form a
                // serialized partial round-trips through.
                #[derive(Deserialize)]
                #[serde(rename_all = "snake_case")]
                enum Helper {
                    Off,
                    Auto,
                    Fixed(RowCount),
                }

                let helper =
                    Helper::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;

                Ok(match helper {
                    Helper::Off => WindowRows::Off,
                    Helper::Auto => WindowRows::Auto,
                    Helper::Fixed(rows) => WindowRows::Fixed(rows),
                })
            }
        }

        deserializer.deserialize_any(WindowRowsVisitor)
    }
}

/// A row count as a [`WindowRows`], collapsing `0` to [`WindowRows::Off`].
///
/// `0` and `false` are the same request, and a zero-row window is something
/// nothing can render into.
const fn fixed(rows: u16) -> WindowRows {
    if rows == 0 {
        WindowRows::Off
    } else {
        WindowRows::Fixed(RowCount { rows })
    }
}

#[cfg(test)]
#[path = "window_rows_tests.rs"]
mod tests;
