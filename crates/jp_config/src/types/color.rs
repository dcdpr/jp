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

    fn build_schema(_schema: SchemaBuilder) -> Schema {
        // Color is either a u8 or a hex string; model as a generic string
        // in the schema since TOML/JSON can't express union types directly.
        Schema::new(schematic::SchemaType::String(Box::default()))
    }
}

impl Color {
    /// Returns the SGR background parameter for this color.
    ///
    /// - `Ansi256(n)` → `"48;5;n"`
    /// - `Rgb { r, g, b }` → `"48;2;r;g;b"`
    #[must_use]
    pub fn to_ansi_bg_param(&self) -> String {
        match self {
            Self::Ansi256(n) => format!("48;5;{n}"),
            Self::Rgb { r, g, b } => format!("48;2;{r};{g};{b}"),
        }
    }

    /// Returns the full ANSI escape sequence for this color as a background.
    #[must_use]
    pub fn to_ansi_bg_escape(&self) -> String {
        format!("\x1b[{}m", self.to_ansi_bg_param())
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ansi256(n) => write!(f, "{n}"),
            Self::Rgb { r, g, b } => write!(f, "#{r:02x}{g:02x}{b:02x}"),
        }
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
            Self::Rgb { .. } => serializer.serialize_str(&self.to_string()),
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
