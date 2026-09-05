//! The labels attached to a conversation.
//!
//! [`Labels`] maps a key to an ordered set of values, and owns the on-disk
//! contract for the `labels` field of `metadata.json`.

use std::{
    collections::{BTreeMap, btree_map},
    fmt,
};

use indexmap::IndexSet;
use serde::{Deserialize, Deserializer, Serialize, de};

/// Key-value annotations attached to a conversation.
///
/// A key maps to a set of values: `crate=jp_config` and `crate=jp_llm` coexist
/// under the same key.
/// A key never maps to an empty set — removing the last value removes the key.
///
/// The empty value is how a *bare* label records that its key is present.
/// Holding any real value records the same thing, so the two never coexist:
/// adding a real value drops the marker, and adding the marker to a key that
/// already holds a value changes nothing.
///
/// A value holds no line break: each one becomes a space as the value is
/// stored, so one label is always one line of text.
///
/// Keys are sorted; values keep the order they were added in, and that order is
/// part of the on-disk contract.
///
/// Values are read as either a single string or an array of strings, and are
/// always written as an array:
///
/// ```json
/// { "branch": ["main"], "crate": ["jp_config", "jp_llm"] }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Labels(BTreeMap<String, IndexSet<String>>);

impl Labels {
    /// Whether no labels are set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The values held under `key`, or `None` when the key is absent.
    ///
    /// The returned set is never empty.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&IndexSet<String>> {
        self.0.get(key)
    }

    /// Whether `key` holds `value`.
    ///
    /// `value` is folded onto one line before the lookup, the same way storing
    /// it would be, so a probe written with a line break finds the value it
    /// names.
    #[must_use]
    pub fn contains(&self, key: &str, value: &str) -> bool {
        self.0
            .get(key)
            .is_some_and(|values| values.contains(&single_line(value)))
    }

    /// Iterate over every key and the values it holds, sorted by key.
    pub fn iter(&self) -> btree_map::Iter<'_, String, IndexSet<String>> {
        self.0.iter()
    }

    /// Add `value` to the set held under `key`, creating the key when absent.
    ///
    /// Returns `false` when nothing changed: either the key already held that
    /// value, or `value` is the presence marker and the key already holds a
    /// real one.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> bool {
        let value = single_line(&value.into());
        let values = self.0.entry(key.into()).or_default();

        if value.is_empty() {
            // A key holding a real value is already present, so the marker has
            // nothing to add. The entry above is fresh when the set is empty,
            // so this leaves no empty set behind.
            return values.is_empty() && values.insert(value);
        }

        values.shift_remove("");
        values.insert(value)
    }

    /// Replace the set held under `key`, returning the values it displaced.
    ///
    /// Empty `values` removes the key.
    /// Duplicates are dropped, the first occurrence winning, and the presence
    /// marker is dropped when a real value is present.
    pub fn set(
        &mut self,
        key: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> IndexSet<String> {
        let key = key.into();
        let values = canonical(values.into_iter().map(Into::into).collect());

        let displaced = if values.is_empty() {
            self.0.remove(&key)
        } else {
            self.0.insert(key, values)
        };

        displaced.unwrap_or_default()
    }

    /// Remove `key` and return the values it held.
    pub fn remove_key(&mut self, key: &str) -> Option<IndexSet<String>> {
        self.0.remove(key)
    }

    /// Remove `value` from the set held under `key`, dropping the key when that
    /// leaves it empty.
    ///
    /// Returns `false` when the key did not hold that value.
    /// `value` is folded onto one line before the lookup, the same way storing
    /// it would be.
    pub fn remove_value(&mut self, key: &str, value: &str) -> bool {
        let Some(values) = self.0.get_mut(key) else {
            return false;
        };

        // Shift rather than swap: the remaining values keep their order.
        let removed = values.shift_remove(&single_line(value));
        if values.is_empty() {
            self.0.remove(key);
        }

        removed
    }
}

impl<K, V, I> FromIterator<(K, I)> for Labels
where
    K: Into<String>,
    V: Into<String>,
    I: IntoIterator<Item = V>,
{
    fn from_iter<T: IntoIterator<Item = (K, I)>>(iter: T) -> Self {
        let mut labels = Self::default();
        for (key, values) in iter {
            labels.set(key, values);
        }

        labels
    }
}

impl IntoIterator for Labels {
    type IntoIter = btree_map::IntoIter<String, IndexSet<String>>;
    type Item = (String, IndexSet<String>);

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Labels {
    type IntoIter = btree_map::Iter<'a, String, IndexSet<String>>;
    type Item = (&'a String, &'a IndexSet<String>);

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'de> Deserialize<'de> for Labels {
    /// Read labels through a validating conversion, so the empty-set invariant
    /// holds for hand-edited files as well as for ones JP wrote.
    ///
    /// A small mistake normalizes rather than failing the whole load: a scalar
    /// becomes a one-element set, repeated values collapse, and an empty array
    /// drops the key.
    /// A value that is neither a string nor an array of strings is an error,
    /// because there is no sane reading of it.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = BTreeMap::<String, ValueSet>::deserialize(deserializer)?;

        Ok(raw.into_iter().map(|(key, set)| (key, set.0)).collect())
    }
}

/// Fold every value onto one line, and drop the presence marker from a set that
/// carries a real value.
///
/// More than one value means at least one of them is non-empty, so the marker
/// is redundant.
fn canonical(values: IndexSet<String>) -> IndexSet<String> {
    // Folding can make two values equal, so the set is rebuilt rather than
    // edited in place; the first occurrence wins, as it does on the way in.
    let mut values: IndexSet<String> = values
        .into_iter()
        .map(|value| single_line(&value))
        .collect();

    if values.len() > 1 {
        values.shift_remove("");
    }

    values
}

/// Fold `value` onto a single line, replacing each line break with a space.
///
/// A label is rendered as one line of text wherever it is shown or searched, so
/// a value carrying a break would produce a second line that nothing identifies
/// as part of the label.
fn single_line(value: &str) -> String {
    // `\r\n` first, so the pair collapses to one space rather than two. A lone
    // `\r` matters on its own: it returns the cursor to the start of the line a
    // terminal is drawing.
    value.replace("\r\n", "\n").replace(['\n', '\r'], " ")
}

/// One key's values as written on disk: a single string, or an array of them.
struct ValueSet(Vec<String>);

impl<'de> Deserialize<'de> for ValueSet {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueSetVisitor)
    }
}

/// Accepts both on-disk shapes of a label value.
struct ValueSetVisitor;

impl<'de> de::Visitor<'de> for ValueSetVisitor {
    type Value = ValueSet;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string or an array of strings")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(ValueSet(vec![v.to_owned()]))
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::with_capacity(seq.size_hint().unwrap_or_default());
        while let Some(value) = seq.next_element::<String>()? {
            values.push(value);
        }

        Ok(ValueSet(values))
    }
}

#[cfg(test)]
#[path = "labels_tests.rs"]
mod tests;
