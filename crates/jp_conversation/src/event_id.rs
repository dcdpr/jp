//! Opaque identifiers for entries in a conversation stream.
//!
//! Two types, because identity has two halves.
//! [`EventId`] is one identifier: a value that is persisted, read back out of
//! hand-edited files, and handed to plugins.
//! [`EventIds`] is the set one stream has issued, and is what makes the
//! identifiers mean anything — "unique within its stream" is a property of the
//! set, not of any value in it.
//!
//! They live together because the rules belong together: the generated format,
//! what counts as a valid identifier, and the promise that an issued identifier
//! is never issued twice are one subject, and splitting them would leave a
//! reader of either half unable to check the other.
//!
//! Only [`EventId`] leaves the crate.

use std::{collections::HashSet, fmt, str::FromStr};

use getrandom::fill;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use crate::Error;

/// Characters a generated ID is built from.
///
/// Applies only to generation.
/// Deserialization accepts any non-empty string.
const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Characters in a generated ID.
///
/// Seven base-36 characters give roughly 78 billion values, which matches Git's
/// short-ref ergonomics and leaves collisions negligible at the ~10k entries a
/// conversation holds.
const GENERATED_LEN: usize = 7;

/// The smallest byte value that maps unevenly onto [`ALPHABET`].
///
/// 252 is the largest multiple of 36 at or below 256, so folding 252..=255 into
/// the alphabet would make four of its characters likelier than the rest.
/// Those bytes are rejected instead.
const BIASED_FROM: u8 = 252;

/// Bytes drawn per attempt at generating an ID.
///
/// Each byte has a 4-in-256 chance of being rejected, so 32 bytes yield fewer
/// than [`GENERATED_LEN`] usable ones about once in 10^13 draws.
/// Drawing enough to finish in one pass keeps the retry a formality rather than
/// a path worth reasoning about.
const DRAW_LEN: usize = 32;

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
    /// Collisions are possible; [`EventIds`] is what enforces uniqueness within
    /// a stream.
    ///
    /// # Panics
    ///
    /// Panics if the OS random source fails.
    #[must_use]
    pub fn random() -> Self {
        let mut id = String::with_capacity(GENERATED_LEN);

        // Terminates because each draw is independent: every pass has an
        // overwhelming chance of filling the ID, so the loop is bounded in
        // practice by the first one. A pass that rejected too many bytes simply
        // draws again.
        while id.len() < GENERATED_LEN {
            let mut bytes = [0; DRAW_LEN];
            fill(&mut bytes).expect("failed to generate event ID: OS random source failed");

            for byte in bytes {
                if byte >= BIASED_FROM {
                    continue;
                }

                id.push(char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]));
                if id.len() == GENERATED_LEN {
                    return Self(id);
                }
            }
        }

        Self(id)
    }

    /// Build an ID from a string, as a hand-edited file or a test fixture would
    /// name one.
    ///
    /// Any non-empty string is accepted, including values outside the generated
    /// format: the format is a generation convention, not a parsing constraint.
    ///
    /// A stream assigns IDs itself, so this is for naming an ID that already
    /// exists, not for minting one.
    /// Use [`ConversationStream::push_event`] to add an entry and learn the ID
    /// it was given.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EmptyEventId`] if `value` is empty.
    ///
    /// [`ConversationStream::push_event`]: crate::ConversationStream::push_event
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.is_empty() {
            return Err(Error::EmptyEventId);
        }

        Ok(Self(value))
    }

    /// Build a non-empty, readable ID for a test fixture.
    ///
    /// This CANNOT be used in release mode.
    ///
    /// # Panics
    ///
    /// Panics if `value` is empty, which in a fixture is a mistake in the test
    /// rather than a condition to handle.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    #[must_use]
    pub fn fixed(value: &str) -> Self {
        Self::new(value).expect("event ID must not be empty")
    }
}

impl FromStr for EventId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for EventId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// The entry IDs one conversation stream has handed out.
///
/// This is where "unique within its stream" is enforced: an ID handed out here
/// is never handed out again, not even after the entry holding it is removed.
/// A reference to a removed entry therefore fails to resolve instead of binding
/// to a later, unrelated entry.
///
/// The set is built from the entries a stream loaded and is never persisted, so
/// this covers one stream's lifetime rather than one conversation's history.
#[expect(
    clippy::redundant_pub_crate,
    reason = "the module is private today; `pub` would read as public API"
)]
#[derive(Debug, Clone, Default)]
pub(crate) struct EventIds(HashSet<EventId>);

impl EventIds {
    /// Take `preferred`, or a generated ID when this set already holds it.
    ///
    /// This is how an entry moving between streams keeps its ID: uniqueness is
    /// scoped to a single stream, so the receiving stream only has to replace
    /// an ID it has handed out itself.
    pub(crate) fn claim(&mut self, preferred: EventId) -> EventId {
        if self.0.insert(preferred.clone()) {
            return preferred;
        }

        self.fresh()
    }

    /// A generated ID this set has not handed out.
    pub(crate) fn fresh(&mut self) -> EventId {
        self.draw(EventId::random)
    }

    /// Whether this set has handed out `id`.
    #[cfg(test)]
    pub(crate) fn contains(&self, id: &EventId) -> bool {
        self.0.contains(id)
    }

    /// Reserve `ids`, so none of them is ever generated.
    ///
    /// Used to take in every ID a file carries before any entry is settled, so
    /// a generated ID cannot take one belonging to an entry further down the
    /// file.
    pub(crate) fn reserve(&mut self, ids: impl IntoIterator<Item = EventId>) {
        self.0.extend(ids);
    }

    /// Draw from `generate` until it produces an ID this set does not hold.
    ///
    /// Split out from [`Self::fresh`] so a test can script the generator and
    /// force the retry; nothing outside this module supplies one.
    fn draw(&mut self, mut generate: impl FnMut() -> EventId) -> EventId {
        loop {
            let id = generate();
            if self.0.insert(id.clone()) {
                return id;
            }
        }
    }
}

impl FromIterator<EventId> for EventIds {
    fn from_iter<T: IntoIterator<Item = EventId>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
#[path = "event_id_tests.rs"]
mod tests;
