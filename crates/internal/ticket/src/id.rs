//! Ticket identifiers.
//!
//! An id is seven Crockford base-32 characters: five encoding a five-second
//! bucket since the epoch, two carrying randomness.
//! The canonical form prefixes them with `T-`, as in `T-02wt0kx`.
//!
//! Every position is fixed width and the alphabet's ASCII order matches its
//! digit order, so comparing two ids as bytes orders them by creation time.
//!
//! The alphabet omits `i`, `l`, `o`, and `u` so an id read off a screen is
//! unambiguous; input maps `i` and `l` to `1` and `o` to `0` so a
//! mis-transcribed id still resolves.

use std::{fmt, str::FromStr};

use serde::{Serialize, Serializer};

use crate::ParseError;

/// Characters an id is written in, in ascending digit order.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Characters in an id body, excluding the `T-` prefix.
pub const ID_WIDTH: usize = 7;

/// Characters of the body that encode the time bucket.
const BUCKET_WIDTH: usize = 5;

/// Distinct tails within one bucket: `32^2`.
pub const TAIL_SPACE: u16 = 1024;

/// Buckets the time component can express: `32^5`.
///
/// At five seconds each this runs out in December 2031, at which point
/// allocation refuses rather than wrapping or widening.
pub const MAX_BUCKET: u32 = 33_554_432;

/// A ticket id.
///
/// The canonical form is `T-02wt0kx`.
/// Input additionally accepts `T02wt0kx`, the bare body, and any case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TicketId([u8; ID_WIDTH]);

impl TicketId {
    /// Build an id from its time bucket and tail.
    ///
    /// Returns `None` when either component is out of range, which is how the
    /// format's expiry surfaces.
    #[must_use]
    pub fn new(bucket: u32, tail: u16) -> Option<Self> {
        if bucket >= MAX_BUCKET || tail >= TAIL_SPACE {
            return None;
        }

        let mut body = [0_u8; ID_WIDTH];
        encode(bucket, &mut body[..BUCKET_WIDTH]);
        encode(u32::from(tail), &mut body[BUCKET_WIDTH..]);

        Some(Self(body))
    }

    /// The five-second bucket this id was allocated in.
    #[must_use]
    pub fn bucket(self) -> u32 {
        decode(&self.0[..BUCKET_WIDTH])
    }

    /// The random or incremented component that separates ids in one bucket.
    #[must_use]
    pub fn tail(self) -> u16 {
        // Two base-32 digits reach 1,023, well inside `u16`.
        u16::try_from(decode(&self.0[BUCKET_WIDTH..])).unwrap_or_default()
    }

    /// The id body, without the `T-` prefix.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Every byte comes from `ALPHABET`, which is ASCII.
        std::str::from_utf8(&self.0).unwrap_or_default()
    }

    /// The filename prefix this id's file carries, e.g. `02wt0kx-`.
    #[must_use]
    pub fn file_prefix(self) -> String {
        format!("{}-", self.as_str())
    }

    /// Fold a user-supplied string onto the alphabet.
    ///
    /// Strips the `T-` or `T` prefix, lowercases, and maps the characters the
    /// alphabet omits onto the ones they are mistaken for.
    /// Returns `None` when a character has no alphabet equivalent, or when the
    /// result is empty or longer than an id; a shorter result is left to the
    /// caller to accept or reject.
    #[must_use]
    pub fn normalize(query: &str) -> Option<String> {
        let trimmed = query.trim();
        let body = if let Some(rest) = trimmed
            .strip_prefix(['T', 't'])
            .filter(|rest| rest.starts_with('-') || rest.chars().count() == ID_WIDTH)
        {
            rest.strip_prefix('-').unwrap_or(rest)
        } else {
            trimmed
        };

        if body.is_empty() || body.chars().count() > ID_WIDTH {
            return None;
        }

        body.chars().map(fold).collect()
    }
}

impl fmt::Display for TicketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "T-{}", self.as_str())
    }
}

impl FromStr for TicketId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        TicketId::normalize(s)
            .filter(|body| body.len() == ID_WIDTH)
            .map(|body| {
                let mut bytes = [0_u8; ID_WIDTH];
                bytes.copy_from_slice(body.as_bytes());
                Self(bytes)
            })
            .ok_or_else(|| ParseError::Id(s.to_owned()))
    }
}

impl Serialize for TicketId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// Write `value` across `out` as base-32, most significant digit first.
fn encode(mut value: u32, out: &mut [u8]) {
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[(value % 32) as usize];
        value /= 32;
    }
}

/// Read base-32 `chars` as a number, most significant digit first.
///
/// Five digits reach `32^5 - 1`, which is the widest an id encodes and fits
/// `u32`.
fn decode(chars: &[u8]) -> u32 {
    chars.iter().fold(0, |acc, byte| {
        let digit = ALPHABET.iter().position(|a| a == byte).unwrap_or(0);
        acc * 32 + u32::try_from(digit).unwrap_or_default()
    })
}

/// Map one input character onto the alphabet, or reject it.
fn fold(char_: char) -> Option<char> {
    let lowered = char_.to_ascii_lowercase();
    let folded = match lowered {
        'i' | 'l' => '1',
        'o' => '0',
        other => other,
    };

    ALPHABET
        .contains(&u8::try_from(folded).ok()?)
        .then_some(folded)
}

#[cfg(test)]
#[path = "id_tests.rs"]
mod tests;
