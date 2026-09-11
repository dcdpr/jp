//! Opaque identifiers for entries in a conversation stream.

use std::fmt;

use getrandom::fill;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

/// Alphabet used only when generating IDs, not when deserializing them.
const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// An opaque, non-empty identifier for a conversation stream entry.
///
/// Serialized as a string.
/// Deserialization preserves any non-empty string verbatim, including values
/// outside the generated format.
/// IDs carry identity within a stream, not ordering or content information.
/// The generated format is internal and may change.
///
/// Once persisted, an entry's ID can be used as a stable reference into the raw
/// conversation stream.
/// Retained entries keep that ID in projected views.
///
/// Entries synthesized only for a projected view have ephemeral IDs with no
/// corresponding entry in the raw stream.
/// The ID value does not encode this distinction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct EventId(String);

impl EventId {
    /// Generate an ID using the OS random source.
    ///
    /// Collisions are possible; the stream must enforce uniqueness on
    /// insertion.
    ///
    /// # Panics
    ///
    /// Panics if the OS random source fails.
    #[must_use]
    pub fn random() -> Self {
        let mut bytes = [0; 7];
        let mut id = String::with_capacity(bytes.len());

        loop {
            fill(&mut bytes).expect("failed to generate event ID: OS random source failed");
            for byte in bytes {
                // 252 is the largest multiple of 36 below 256. Reject the
                // remaining values so every character is equally likely.
                if byte >= 252 {
                    continue;
                }

                id.push(char::from(ALPHABET[usize::from(byte % 36)]));
                if id.len() == bytes.len() {
                    return Self(id);
                }
            }
        }
    }

    /// Build a non-empty, readable ID for a test fixture.
    #[cfg(test)]
    pub(crate) fn fixed(value: &str) -> Self {
        assert!(!value.is_empty(), "event ID must not be empty");
        Self(value.to_owned())
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for EventId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty() {
            return Err(D::Error::custom("event ID must not be empty"));
        }

        Ok(Self(value))
    }
}

#[cfg(test)]
#[path = "event_id_tests.rs"]
mod tests;
