use jp_tool::Outcome;
use serde::{Deserialize, Serialize};

pub type ToolResult = std::result::Result<Outcome, Box<dyn std::error::Error + Send + Sync>>;

#[expect(clippy::unnecessary_wraps)]
pub fn error(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> ToolResult {
    Ok(Outcome::error(error.into().as_ref()))
}

#[expect(clippy::unnecessary_wraps)]
pub fn fail(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> ToolResult {
    Ok(Outcome::fail(error.into().as_ref()))
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    /// Returns the inner value as a `Vec`, consuming the `OneOrMany`.
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        match self {
            OneOrMany::One(v) => vec![v],
            OneOrMany::Many(v) => v,
        }
    }

    /// Returns the inner value as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        match self {
            OneOrMany::One(v) => std::slice::from_ref(v),
            OneOrMany::Many(v) => v,
        }
    }
}

impl<T: PartialEq> PartialEq for OneOrMany<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::One(v1), Self::One(v2)) => v1 == v2,
            (Self::Many(v1), Self::Many(v2)) => v1 == v2,
            _ => false,
        }
    }
}

impl<T: Clone> Clone for OneOrMany<T> {
    fn clone(&self) -> Self {
        match self {
            Self::One(v) => Self::One(v.clone()),
            Self::Many(v) => Self::Many(v.clone()),
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for OneOrMany<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::One(v) => std::fmt::Debug::fmt(v, f),
            Self::Many(v) => std::fmt::Debug::fmt(v, f),
        }
    }
}

impl<T> Default for OneOrMany<T> {
    fn default() -> Self {
        Self::Many(vec![])
    }
}

impl<T> std::ops::Deref for OneOrMany<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        match self {
            OneOrMany::One(v) => std::slice::from_ref(v),
            OneOrMany::Many(v) => v,
        }
    }
}

impl<T> std::ops::DerefMut for OneOrMany<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            OneOrMany::One(v) => std::slice::from_mut(v),
            OneOrMany::Many(v) => v,
        }
    }
}

impl<T> FromIterator<T> for OneOrMany<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut items = iter.into_iter().collect::<Vec<_>>();

        if items.len() == 1 {
            Self::One(items.remove(0))
        } else {
            Self::Many(items)
        }
    }
}

impl<T> IntoIterator for OneOrMany<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            OneOrMany::One(v) => vec![v].into_iter(),
            OneOrMany::Many(v) => v.into_iter(),
        }
    }
}

impl<T> From<T> for OneOrMany<T> {
    fn from(v: T) -> Self {
        Self::One(v)
    }
}

impl<T> From<Vec<T>> for OneOrMany<T> {
    fn from(mut v: Vec<T>) -> Self {
        if v.len() == 1 {
            Self::One(v.remove(0))
        } else {
            Self::Many(v)
        }
    }
}

impl<T> From<OneOrMany<T>> for Vec<T> {
    fn from(v: OneOrMany<T>) -> Self {
        match v {
            OneOrMany::One(v) => vec![v],
            OneOrMany::Many(v) => v,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_one_or_many_one() {
        let mut v = OneOrMany::One(1);

        assert_eq!(v.clone().into_vec(), vec![1]);
        assert_eq!(v.as_slice(), &[1]);
        assert_eq!(v, OneOrMany::One(1));
        assert_eq!(format!("{v:?}"), "1");
        assert_eq!(OneOrMany::<()>::default(), OneOrMany::Many(vec![]));
        assert_eq!(v.first(), Some(&1));
        assert_eq!(v.first_mut(), Some(&mut 1));
        assert_eq!(OneOrMany::from_iter(vec![1]), OneOrMany::One(1));
        assert_eq!(v.clone().into_iter().collect::<Vec<_>>(), vec![1]);
        assert_eq!(OneOrMany::from(1), OneOrMany::One(1));
        assert_eq!(OneOrMany::from(vec![1]), OneOrMany::One(1));
        assert_eq!(Vec::from(v), vec![1]);
    }

    #[test]
    fn test_one_or_many_many() {
        let mut v = OneOrMany::Many(vec![1, 2, 3]);

        assert_eq!(v.clone().into_vec(), vec![1, 2, 3]);
        assert_eq!(v.as_slice(), &[1, 2, 3]);
        assert_eq!(v, OneOrMany::Many(vec![1, 2, 3]));
        assert_eq!(format!("{v:?}"), "[1, 2, 3]");
        assert_eq!(OneOrMany::<()>::default(), OneOrMany::Many(vec![]));
        assert_eq!(v.last(), Some(&3));
        assert_eq!(v.last_mut(), Some(&mut 3));
        assert_eq!(
            OneOrMany::from_iter(vec![1, 2, 3]),
            OneOrMany::Many(vec![1, 2, 3])
        );
        assert_eq!(v.clone().into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(
            OneOrMany::from(vec![1, 2, 3]),
            OneOrMany::Many(vec![1, 2, 3])
        );
        assert_eq!(Vec::from(v), vec![1, 2, 3]);
    }
}
