use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
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

impl<T> OneOrMany<T> {
    pub fn into_vec(self) -> Vec<T> {
        match self {
            OneOrMany::One(v) => vec![v],
            OneOrMany::Many(v) => v,
        }
    }

    pub fn as_slice(&self) -> &[T] {
        match self {
            OneOrMany::One(v) => std::slice::from_ref(v),
            OneOrMany::Many(v) => v,
        }
    }
}

impl<T: PartialEq> OneOrMany<T> {
    pub fn contains(&self, value: &T) -> bool {
        match self {
            OneOrMany::One(v) => v == value,
            OneOrMany::Many(v) => v.contains(value),
        }
    }
}

impl<T> FromIterator<T> for OneOrMany<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::Many(iter.into_iter().collect())
    }
}

impl<T> From<T> for OneOrMany<T> {
    fn from(v: T) -> Self {
        Self::One(v)
    }
}

impl<T> From<Vec<T>> for OneOrMany<T> {
    fn from(v: Vec<T>) -> Self {
        Self::Many(v)
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
