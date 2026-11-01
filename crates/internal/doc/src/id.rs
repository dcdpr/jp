//! Epoch ids: the seven characters a document such as a ticket is named by.
//!
//! An id is seven Crockford base-32 characters: five encoding a five-second
//! bucket since its directory's [epoch], two carrying randomness.
//! The canonical form prefixes them with the letter of the document's [`Kind`]
//! and a dash, as `T-02wt0kx` names a ticket.
//!
//! Every position is fixed width and the alphabet's ASCII order matches its
//! digit order, so comparing two ids as bytes orders them by bucket, then tail.
//!
//! That is creation order for ids allocated in one directory, where the tail
//! continues from the highest id in the bucket.
//! Ids drawn in separate checkouts that share a bucket carry unrelated tails,
//! so between those two the order is arbitrary.
//!
//! The alphabet omits `i`, `l`, `o`, and `u` so an id read off a screen is
//! unambiguous; input maps `i` and `l` to `1` and `o` to `0` so a
//! mis-transcribed id still resolves.
//!
//! [epoch]: crate::epoch

use std::{
    cmp::Ordering,
    error, fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    str::FromStr,
};

use serde::{Serialize, Serializer};

/// Characters an id is written in, in ascending digit order.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Characters in an id body, excluding the prefix.
pub const WIDTH: usize = 7;

/// Characters of the body that encode the time bucket.
const BUCKET_WIDTH: usize = 5;

/// Distinct tails within one bucket: `32^2`.
pub const TAIL_SPACE: u16 = 1024;

/// Buckets the time component can express: `32^5`.
///
/// At five seconds each this runs out about 5.3 years after the epoch, at which
/// point allocation refuses rather than wrapping or widening.
pub const MAX_BUCKET: u32 = 33_554_432;

/// A kind of document named by an [`Id`].
///
/// Implemented by a marker type, one per kind, which is what keeps an id of one
/// kind from being read where another kind's is expected.
pub trait Kind {
    /// The letter the canonical form opens with: `T` in `T-02wt0kx`.
    const PREFIX: char;

    /// What an id of this kind names, the way a message says it: `ticket`.
    const NOUN: &'static str;

    /// The tool that writes the directory these documents live in, as the epoch
    /// file's preamble names it: `JP-TICKET`.
    const WRITER: &'static str;
}

/// An id naming a document of kind `K`.
///
/// The canonical form is the kind's prefix, a dash, and the body: `T-02wt0kx`.
/// Input additionally accepts the prefix without the dash (`T02wt0kx`), the
/// bare body, and any case.
pub struct Id<K> {
    body: [u8; WIDTH],
    kind: PhantomData<fn() -> K>,
}

impl<K> Id<K> {
    /// Build an id from its time bucket and tail.
    ///
    /// Returns `None` when either component is out of range, which is how the
    /// format's expiry surfaces.
    #[must_use]
    pub fn new(bucket: u32, tail: u16) -> Option<Self> {
        if bucket >= MAX_BUCKET || tail >= TAIL_SPACE {
            return None;
        }

        let mut body = [0_u8; WIDTH];
        encode(bucket, &mut body[..BUCKET_WIDTH]);
        encode(u32::from(tail), &mut body[BUCKET_WIDTH..]);

        Some(Self::from_body(body))
    }

    /// The five-second bucket this id was allocated in.
    #[must_use]
    pub fn bucket(self) -> u32 {
        decode(&self.body[..BUCKET_WIDTH])
    }

    /// The random or incremented component that separates ids in one bucket.
    #[must_use]
    pub fn tail(self) -> u16 {
        // Two base-32 digits reach 1,023, well inside `u16`.
        u16::try_from(decode(&self.body[BUCKET_WIDTH..])).unwrap_or_default()
    }

    /// The id body, without the prefix.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Every byte comes from `ALPHABET`, which is ASCII.
        std::str::from_utf8(&self.body).unwrap_or_default()
    }

    /// The filename prefix this id's file carries, e.g. `02wt0kx-`.
    #[must_use]
    pub fn file_prefix(self) -> String {
        format!("{}-", self.as_str())
    }

    fn from_body(body: [u8; WIDTH]) -> Self {
        Self {
            body,
            kind: PhantomData,
        }
    }
}

impl<K: Kind> Id<K> {
    /// Fold a user-supplied string onto the alphabet.
    ///
    /// Strips the kind's prefix, with or without its dash, lowercases, and maps
    /// the characters the alphabet omits onto the ones they are mistaken for.
    /// Returns `None` when a character has no alphabet equivalent, or when the
    /// result is empty or longer than an id; a shorter result is left to the
    /// caller to accept or reject.
    #[must_use]
    pub fn normalize(query: &str) -> Option<String> {
        let trimmed = query.trim();
        let prefixes = [
            K::PREFIX.to_ascii_uppercase(),
            K::PREFIX.to_ascii_lowercase(),
        ];
        let body = if let Some(rest) = trimmed
            .strip_prefix(prefixes)
            .filter(|rest| rest.starts_with('-') || rest.chars().count() == WIDTH)
        {
            rest.strip_prefix('-').unwrap_or(rest)
        } else {
            trimmed
        };

        if body.is_empty() || body.chars().count() > WIDTH {
            return None;
        }

        body.chars().map(fold).collect()
    }
}

// Written out rather than derived: a derive would hold `K` to each trait, and
// the marker types carry nothing to compare, hash, or copy.
impl<K> Clone for Id<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Id<K> {}

impl<K> PartialEq for Id<K> {
    fn eq(&self, other: &Self) -> bool {
        self.body == other.body
    }
}

impl<K> Eq for Id<K> {}

impl<K> PartialOrd for Id<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K> Ord for Id<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.body.cmp(&other.body)
    }
}

impl<K> Hash for Id<K> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.body.hash(state);
    }
}

impl<K> fmt::Debug for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Id").field(&self.as_str()).finish()
    }
}

impl<K: Kind> fmt::Display for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", K::PREFIX, self.as_str())
    }
}

impl<K: Kind> FromStr for Id<K> {
    type Err = InvalidId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::normalize(s)
            .filter(|body| body.len() == WIDTH)
            .map(|body| {
                let mut bytes = [0_u8; WIDTH];
                bytes.copy_from_slice(body.as_bytes());
                Self::from_body(bytes)
            })
            .ok_or_else(|| InvalidId {
                value: s.to_owned(),
                prefix: K::PREFIX,
                noun: K::NOUN,
            })
    }
}

impl<K: Kind> Serialize for Id<K> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// A string that isn't an id of the kind it was read as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidId {
    value: String,
    prefix: char,
    noun: &'static str,
}

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a {} id (try {}-02wt0kx).",
            self.value, self.noun, self.prefix
        )
    }
}

impl error::Error for InvalidId {}

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
