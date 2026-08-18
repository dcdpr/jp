//! Human-readable byte sizes.
//!
//! [`ByteSize`] is the unit for size thresholds in configuration.
//! It accepts a human-readable string (`"1MB"`, `"512 KB"`) or a bare byte
//! count, and compares as a plain number of bytes.
//!
//! ```toml
//! [[conversation.compaction.rules]]
//! tool_calls = { policy = "strip-responses", over = "1MB" }
//! ```

use std::{fmt, str::FromStr};

use schematic::{Schema, SchemaBuilder, Schematic};
use serde::{Deserialize, Serialize};

use crate::BoxedError;

/// One kibibyte, in bytes.
const KB: u64 = 1024;
/// One mebibyte, in bytes.
const MB: u64 = KB * 1024;
/// One gibibyte, in bytes.
const GB: u64 = MB * 1024;

/// Units recognized on input and used on output, largest first.
const UNITS: [(u64, &str); 3] = [(GB, "GB"), (MB, "MB"), (KB, "KB")];

/// A size in bytes.
///
/// Written as a human-readable string (`"1MB"`, `"512 KB"`, `"4GiB"`) or a bare
/// byte count (`1048576`).
///
/// Unit suffixes are binary: `1KB` is 1024 bytes, `1MB` is 1048576 bytes.
/// `KiB` / `MiB` / `GiB` are accepted as explicit spellings of the same values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct ByteSize(u64);

impl ByteSize {
    /// A size of zero bytes.
    pub const ZERO: Self = Self(0);

    /// Build a size from a raw byte count.
    #[must_use]
    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    /// The size as a raw byte count.
    #[must_use]
    pub const fn as_bytes(self) -> u64 {
        self.0
    }

    /// An approximate, one-decimal rendering for terminal output, e.g. `10.4
    /// MB`.
    ///
    /// Lossy by design.
    /// [`Display`] is the exact, round-trippable form used for serialization.
    ///
    /// [`Display`]: fmt::Display
    #[must_use]
    pub fn human(self) -> String {
        for (size, suffix) in UNITS {
            if self.0 >= size {
                let whole = self.0 / size;
                let tenths = (self.0 % size) * 10 / size;
                return format!("{whole}.{tenths} {suffix}");
            }
        }
        format!("{} B", self.0)
    }
}

impl FromStr for ByteSize {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        let digit_count = trimmed
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(trimmed.len());

        if digit_count == 0 {
            return Err(format!("invalid size `{s}`: expected a leading byte count").into());
        }

        let value: u64 = trimmed[..digit_count]
            .parse()
            .map_err(|_| format!("invalid size `{s}`: byte count out of range"))?;

        let multiplier = match trimmed[digit_count..].trim().to_ascii_lowercase().as_str() {
            "" | "b" => 1,
            "k" | "kb" | "kib" => KB,
            "m" | "mb" | "mib" => MB,
            "g" | "gb" | "gib" => GB,
            unit => {
                return Err(format!(
                    "invalid size `{s}`: unknown unit `{unit}` (expected B, KB, MB, or GB)"
                )
                .into());
            }
        };

        value.checked_mul(multiplier).map_or_else(
            || Err(format!("invalid size `{s}`: value overflows").into()),
            |bytes| Ok(Self(bytes)),
        )
    }
}

impl fmt::Display for ByteSize {
    /// Render the exact size, using the largest unit that divides it evenly.
    ///
    /// The output always parses back to the same value, which is what makes it
    /// safe to use as the serialized form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (size, suffix) in UNITS {
            if self.0 >= size && self.0.is_multiple_of(size) {
                return write!(f, "{}{suffix}", self.0 / size);
            }
        }
        write!(f, "{}", self.0)
    }
}

impl From<u64> for ByteSize {
    fn from(bytes: u64) -> Self {
        Self(bytes)
    }
}

impl Serialize for ByteSize {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ByteSizeVisitor;

        impl serde::de::Visitor<'_> for ByteSizeVisitor {
            type Value = ByteSize;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a byte count or a size string like `1MB`")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<ByteSize, E> {
                Ok(ByteSize(v))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<ByteSize, E> {
                u64::try_from(v)
                    .map(ByteSize)
                    .map_err(|_| E::custom(format!("size must be non-negative, got `{v}`")))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<ByteSize, E> {
                v.parse().map_err(E::custom)
            }
        }

        // `deserialize_any` lets self-describing formats supply either an
        // integer (`over = 1048576`) or a string (`over = "1MB"`).
        deserializer.deserialize_any(ByteSizeVisitor)
    }
}

impl Schematic for ByteSize {
    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        // Accepts either a bare integer (byte count) or a string (`"1MB"`),
        // matching what the deserializer takes.
        schema.union(schematic::schema::UnionType {
            variants_types: vec![
                Box::new(schema.infer::<u64>()),
                Box::new(schema.infer::<String>()),
            ],
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[path = "byte_size_tests.rs"]
mod tests;
