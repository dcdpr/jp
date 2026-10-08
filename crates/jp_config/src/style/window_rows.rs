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

use std::{fmt, str::FromStr};

use schematic::{
    Schema, SchemaBuilder, Schematic,
    schema::{
        BooleanType, EnumType, IntegerKind, IntegerType, LiteralValue, SchemaField, UnionType,
    },
};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowRows {
    /// Show no output; the indicator is its status row alone.
    #[default]
    Off,

    /// Size the window from the terminal's height.
    Auto,

    /// Show exactly this many rows.
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

/// A string that is neither a keyword nor a row count.
#[derive(Debug, thiserror::Error)]
#[error("invalid row count `{0}`: expected `off`, `auto`, `false`, `true`, or a number of rows")]
pub struct WindowRowsParseError(String);

impl FromStr for WindowRows {
    type Err = WindowRowsParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" | "false" => Ok(Self::Off),
            "auto" | "true" => Ok(Self::Auto),
            _ => s
                .parse::<u16>()
                .map(fixed)
                .map_err(|_| WindowRowsParseError(s.to_owned())),
        }
    }
}

impl TryFrom<&str> for WindowRows {
    type Error = WindowRowsParseError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl Schematic for WindowRows {
    fn schema_name() -> Option<String> {
        Some("WindowRows".to_owned())
    }

    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        // The string spellings of the booleans are listed beside the keywords
        // they mean, since `--cfg KEY=false` hands the value over as a string.
        let keyword = |value: &str, alias: &str| {
            let mut field = SchemaField::new(Schema::literal_value(LiteralValue::String(
                value.to_owned(),
            )));
            field.aliases = vec![alias.to_owned()];
            field
        };

        schema.union(UnionType::new_any([
            schema.nest().enumerable(EnumType::from_fields(
                [
                    ("Off".to_owned(), keyword("off", "false")),
                    ("Auto".to_owned(), keyword("auto", "true")),
                ],
                Some(0),
            )),
            schema.nest().boolean(BooleanType::default()),
            schema
                .nest()
                .integer(IntegerType::new_kind(IntegerKind::U16)),
        ]))
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
                v.parse()
                    .map_err(|_| serde::de::Error::unknown_variant(v, &["off", "auto", "a number"]))
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
