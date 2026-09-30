//! Hand-maintained model tables, for providers whose API reports too little.
//!
//! A [`Catalog`] holds one [`Entry`] per model.
//! An entry is found under the model's canonical id or any of its aliases, such
//! as a dated snapshot or a `-latest` pointer.
//! What an entry carries is up to the provider: the full [`ModelDetails`] when
//! the API reports nothing, or only the facts the API leaves out.

use std::iter;

use crate::model::ModelDetails;

/// A value a [`Catalog`] can hold: something that names its canonical model id.
pub(crate) trait Cataloged {
    /// The id the model is cataloged under, without its provider prefix.
    fn catalog_id(&self) -> &str;
}

impl Cataloged for ModelDetails {
    fn catalog_id(&self) -> &str {
        self.id.name.as_ref()
    }
}

/// One model in a [`Catalog`].
pub(crate) struct Entry<T> {
    /// Other ids the provider serves the model under.
    pub aliases: &'static [&'static str],

    /// What the provider knows about the model.
    pub value: T,
}

/// A provider's table of models, looked up by canonical id or alias.
pub(crate) struct Catalog<T>(Vec<Entry<T>>);

impl<T: Cataloged> Catalog<T> {
    /// Build a catalog from its entries, in the order they are listed.
    ///
    /// # Panics
    ///
    /// In debug builds, if two entries claim the same id: a lookup would
    /// silently return whichever comes first.
    pub(crate) fn new(entries: Vec<Entry<T>>) -> Self {
        let catalog = Self(entries);
        if let Some(id) = catalog.duplicate_id() {
            debug_assert!(false, "{id} is cataloged twice");
        }

        catalog
    }

    /// The value cataloged under `id`, as its canonical id or an alias.
    pub(crate) fn get(&self, id: &str) -> Option<&T> {
        self.0
            .iter()
            .find(|entry| entry.value.catalog_id() == id || entry.aliases.contains(&id))
            .map(|entry| &entry.value)
    }

    /// Every value, in the order the entries were listed.
    pub(crate) fn values(&self) -> impl Iterator<Item = &T> {
        self.0.iter().map(|entry| &entry.value)
    }

    /// The first id, canonical or alias, that more than one entry claims.
    pub(crate) fn duplicate_id(&self) -> Option<&str> {
        let mut seen = vec![];
        self.0
            .iter()
            .flat_map(|entry| {
                let aliases = entry.aliases.iter().copied();
                iter::once(entry.value.catalog_id()).chain(aliases)
            })
            .find(|id| {
                let duplicate = seen.contains(id);
                seen.push(*id);
                duplicate
            })
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
