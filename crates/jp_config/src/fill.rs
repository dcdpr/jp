//! Gap-filling for partial configurations.
//!
//! [`FillDefaults`] fills `None` fields from a defaults partial without
//! applying merge strategies.
//! Unlike `PartialConfig::merge` — which dispatches per-field strategies like
//! `append_vec` — `fill_from` unconditionally preserves existing values and
//! only fills gaps.
//!
//! This is the correct operation for applying schematic defaults to a partial,
//! and for applying a resolved snapshot over the configuration it was resolved
//! from: in both cases the intent is gap-filling, not layer-merging.

use indexmap::IndexMap;

/// Fill `None` fields from defaults without applying merge strategies.
///
/// For `Option<T>` fields, this is `self.or(defaults)`.
/// For nested partial structs, this recurses.
/// For lists, the existing value is kept as-is: a list has no `None` state to
/// fill.
/// For maps, see [`fill_map`] — a missing key is a gap.
pub trait FillDefaults {
    /// Fill `None` fields from `defaults`, keeping all `Some` values.
    #[must_use]
    fn fill_from(self, defaults: Self) -> Self;
}

/// Fill a map from defaults, key by key.
///
/// A key only `defaults` holds is added, keeping the order it arrived in.
/// A key both hold keeps its value from `map` whole, since a value that is
/// already there is not a gap.
pub fn fill_map<V>(
    mut map: IndexMap<String, V>,
    defaults: IndexMap<String, V>,
) -> IndexMap<String, V> {
    for (key, default) in defaults {
        map.entry(key).or_insert(default);
    }

    map
}

/// Fill an optional nested partial from defaults.
///
/// When both are `Some`, recurses into [`FillDefaults::fill_from`].
/// Otherwise uses `Option::or` semantics.
pub fn fill_opt<T: FillDefaults>(value: Option<T>, defaults: Option<T>) -> Option<T> {
    match (value, defaults) {
        (Some(v), Some(d)) => Some(v.fill_from(d)),
        (value, defaults) => value.or(defaults),
    }
}
