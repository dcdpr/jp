//! Terminal color type for configuration values.

use std::{fmt, str::FromStr};

use schematic::{Schema, SchemaBuilder, Schematic};
use serde::{Deserialize, Serialize};

/// A terminal color, either an ANSI 256-color palette index or 24-bit RGB.
///
/// Accepts both integer and string representations:
///
/// - `236` or `"236"` → ANSI 256-color index
/// - `"#504945"` → 24-bit RGB
///
/// # Examples
///
/// ```toml
/// # ANSI 256-color
/// background = 236
///
/// # Hex RGB
/// background = "#504945"
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// ANSI 256-color palette index (0–255).
    Ansi256(u8),
    /// 24-bit RGB color.
    Rgb {
        /// Red channel.
        r: u8,
        /// Green channel.
        g: u8,
        /// Blue channel.
        b: u8,
    },
}

impl Schematic for Color {
    fn schema_name() -> Option<String> {
        Some("Color".into())
    }

    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        schema.union(schematic::schema::UnionType {
            variants_types: vec![
                Box::new(schema.infer::<u8>()),
                Box::new(schema.infer::<String>()),
            ],
            ..Default::default()
        })
    }
}

/// Error when parsing a [`Color`] from a string.
#[derive(Debug, thiserror::Error)]
#[error("invalid color: {0:?} (expected a number 0-255 or #RRGGBB)")]
pub struct ColorParseError(String);

impl FromStr for Color {
    type Err = ColorParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.strip_prefix('#').map_or_else(
            || {
                s.parse::<u8>()
                    .map(Self::Ansi256)
                    .map_err(|_| ColorParseError(s.to_owned()))
            },
            |hex| parse_hex_rgb(hex).ok_or_else(|| ColorParseError(s.to_owned())),
        )
    }
}

/// Parse a 6-digit hex string (without `#`) into an RGB color.
fn parse_hex_rgb(hex: &str) -> Option<Color> {
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb { r, g, b })
}

impl Serialize for Color {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Ansi256(n) => serializer.serialize_u8(*n),
            Self::Rgb { r, g, b } => {
                serializer.serialize_str(&format!("#{r:02x}{g:02x}{b:02x}"))
            }
        }
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ColorVisitor;

        impl serde::de::Visitor<'_> for ColorVisitor {
            type Value = Color;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an integer 0-255 or a hex color string like \"#504945\"")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                u8::try_from(v)
                    .map(Color::Ansi256)
                    .map_err(|_| E::custom(format!("color index {v} out of range 0-255")))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                u8::try_from(v)
                    .map(Color::Ansi256)
                    .map_err(|_| E::custom(format!("color index {v} out of range 0-255")))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                v.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_any(ColorVisitor)
    }
}

#[cfg(test)]
#[path = "color_tests.rs"]
mod tests;
