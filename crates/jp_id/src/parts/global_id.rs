use std::{borrow::Cow, fmt, ops::Deref};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GlobalId(Cow<'static, str>);

impl GlobalId {
    /// Creates a new global ID from the given [`str`] or [`String`].
    // TODO: Use `TryFrom` and enforce correctness.
    #[must_use]
    pub fn new(id: impl Into<Cow<'static, str>>) -> Self {
        Self(id.into())
    }

    /// Returns `true` if the variant is valid.
    // TODO: Use `TryFrom` and enforce correctness.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self
            .0
            .chars()
            .any(|c| !(c.is_numeric() || (c.is_ascii_alphabetic() && c.is_ascii_lowercase())))
    }
}

impl Deref for GlobalId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<String> for GlobalId {
    fn from(c: String) -> Self {
        Self(c.into())
    }
}

impl From<&'static str> for GlobalId {
    fn from(c: &'static str) -> Self {
        Self(c.into())
    }
}

impl fmt::Display for GlobalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
