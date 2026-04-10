//! Gap-filling for partial configurations.
//!
//! [`FillDefaults`] fills `None` fields from a defaults partial without
//! applying merge strategies. Unlike `PartialConfig::merge` — which
//! dispatches per-field strategies like `append_vec` — `fill_from`
//! unconditionally preserves existing values and only fills gaps.
//!
//! This is the correct operation for applying schematic defaults to a
//! partial, where the intent is gap-filling, not layer-merging.

/// Fill `None` fields from defaults without applying merge strategies.
///
/// For `Option<T>` fields, this is `self.or(defaults)`. For nested partial
/// structs, this recurses. For collections (`Vec`, `IndexMap`), the existing
/// value is kept as-is (collections have no `None` state to fill).
pub trait FillDefaults {
    /// Fill `None` fields from `defaults`, keeping all `Some` values.
    #[must_use]
    fn fill_from(self, defaults: Self) -> Self;
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
