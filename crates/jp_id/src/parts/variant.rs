use std::{fmt, ops::Deref};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Variant(char);

impl Variant {
    /// Creates a new variant from the given [`char`].
    #[must_use]
    pub fn new(c: char) -> Self {
        Self(c)
    }

    /// Returns the inner variant [`char`].
    #[must_use]
    pub fn into_inner(self) -> char {
        self.0
    }

    /// Returns `true` if the variant is valid.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.0.is_ascii_alphabetic() && self.0.is_ascii_lowercase()
    }
}

impl Deref for Variant {
    type Target = char;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<char> for Variant {
    fn from(c: char) -> Self {
        Self(c)
    }
}

impl fmt::Display for Variant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
