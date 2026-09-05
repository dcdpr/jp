//! Whether a progress indicator shows the child process's output.
//!
//! Waits that spawn a child process — an MCP server starting, a tool running
//! — can show the last few lines that child wrote to stderr above the timer,
//! so a five-minute build looks like progress rather than a hang.
//! The lines are erased with the timer and never join the transcript.
//!
//! ```toml
//! [style.mcp_startup]
//! print_stderr = true   # false | true | N
//! ```

use std::{fmt, num::ParseIntError};

use schematic::ConfigEnum;
use serde::{Deserialize, Serialize};

/// How many rows of a child's stderr to show above a progress indicator.
///
/// - `false` or `0`: show no output, only the timer.
/// - `true`: size the window from the terminal height (a tenth of it).
/// - `N`: show exactly `N` rows.
///
/// The count is shared across every source feeding the indicator rather than
/// being one window each: `print_stderr = 1` is a single row that each source
/// replaces with its latest line.
/// A terminal whose height cannot be determined shows no output whatever the
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum PrintStderr {
    /// Show no output; the indicator is the timer alone.
    #[default]
    Off,

    /// Size the window from the terminal's height.
    Auto,

    /// Show exactly this many rows.
    #[variant(fallback)]
    Rows(StderrRows),
}

impl PrintStderr {
    /// Whether any output rows are shown.
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// A fixed number of output rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StderrRows {
    /// Rows to show.
    pub rows: u16,
}

impl TryFrom<&str> for StderrRows {
    type Error = ParseIntError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse().map(|rows| Self { rows })
    }
}

impl fmt::Display for StderrRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.rows)
    }
}

impl<'de> Deserialize<'de> for PrintStderr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct PrintStderrVisitor;

        impl<'de> serde::de::Visitor<'de> for PrintStderrVisitor {
            type Value = PrintStderr;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a boolean, a string (\"off\", \"auto\"), or a row count")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(if v {
                    PrintStderr::Auto
                } else {
                    PrintStderr::Off
                })
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v {
                    "off" => Ok(PrintStderr::Off),
                    "auto" => Ok(PrintStderr::Auto),
                    s => s.parse::<u16>().map(rows).map_err(|_| {
                        serde::de::Error::unknown_variant(v, &["off", "auto", "a number"])
                    }),
                }
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                u16::try_from(v).map(rows).map_err(|_| {
                    serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(v), &"a number")
                })
            }

            // TOML hands every integer to `visit_i64`, whatever its sign.
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                u16::try_from(v).map(rows).map_err(|_| {
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
                    Rows(StderrRows),
                }

                let helper =
                    Helper::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;

                Ok(match helper {
                    Helper::Off => PrintStderr::Off,
                    Helper::Auto => PrintStderr::Auto,
                    Helper::Rows(rows) => PrintStderr::Rows(rows),
                })
            }
        }

        deserializer.deserialize_any(PrintStderrVisitor)
    }
}

/// A row count as a [`PrintStderr`], collapsing `0` to [`PrintStderr::Off`].
///
/// `0` and `false` are the same request, and a zero-row window is something
/// nothing can render into.
const fn rows(rows: u16) -> PrintStderr {
    if rows == 0 {
        PrintStderr::Off
    } else {
        PrintStderr::Rows(StderrRows { rows })
    }
}

#[cfg(test)]
#[path = "print_stderr_tests.rs"]
mod tests;
