use std::{borrow::Cow, fmt, ops::Deref};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TargetId(Cow<'static, str>);

impl TargetId {
    /// Creates a new target ID from the given [`str`] or [`String`].
    #[must_use]
    pub fn new(id: impl Into<Cow<'static, str>>) -> Self {
        Self(id.into())
    }

    /// Returns `true` if the variant is valid.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self
            .0
            .chars()
            .any(|c| !(c.is_numeric() || (c.is_ascii_alphabetic() && c.is_ascii_lowercase())))
    }
}

impl Deref for TargetId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for TargetId {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl From<String> for TargetId {
    fn from(c: String) -> Self {
        Self(c.into())
    }
}

impl From<&'static str> for TargetId {
    fn from(c: &'static str) -> Self {
        Self(c.into())
    }
}

impl fmt::Display for TargetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
