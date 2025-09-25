//! Configuration delta calculation.

use schematic::PartialConfig;

/// Calculate the delta between two partial configurations.
///
/// It takes `self`, and checks for any value in `next` that differs from
/// `self`. If a value differs, it is returned in the final [`PartialConfig`].
///
/// If no difference is found, the field is set to `None`.
///
/// If all values are equal, then the returned `PartialConfig` is the same as
/// [`PartialConfig::empty`].
pub trait PartialConfigDelta: PartialConfig {
    /// Calculate the delta between two partial configurations.
    ///
    /// It takes `self`, and checks for any value in `next` that differs from
    /// `self`. If a value differs, it is returned in the final
    /// [`PartialConfig`].
    ///
    /// If no difference is found, the field is set to `None`.
    ///
    /// If all values are equal, then the returned `PartialConfig` is the
    /// same as [`PartialConfig::empty`].
    fn delta(&self, next: Self) -> Self;
}

/// Calculate the delta between two optional values.
pub fn delta_opt<T: PartialEq>(prev: Option<&T>, next: Option<T>) -> Option<T> {
    match (prev, next) {
        (Some(prev), Some(next)) if prev != &next => Some(next),
        (None, next) => next,
        _ => None,
    }
}

/// Calculate the delta between two optional values.
pub fn delta_opt_partial<T: PartialConfigDelta + PartialEq>(
    prev: Option<&T>,
    next: Option<T>,
) -> Option<T> {
    match (prev, next) {
        (Some(prev), Some(next)) if prev != &next => Some(prev.delta(next)),
        (None, next) => next,
        _ => None,
    }
}

/// Calculate the delta between two optional vec-configurations.
pub fn delta_opt_vec<T: PartialEq>(prev: Option<&Vec<T>>, next: Option<Vec<T>>) -> Option<Vec<T>> {
    if prev.is_some_and(|prev| {
        prev.iter()
            .all(|v| next.as_ref().is_some_and(|next| next.contains(v)))
    }) {
        return None;
    }

    next.map(|v| {
        v.into_iter()
            .filter(|v| !prev.as_ref().is_some_and(|prev| prev.contains(v)))
            .collect()
    })
}

/// Calculate the delta between two vec-configurations.
pub fn delta_vec<T: PartialEq>(prev: &[T], next: Vec<T>) -> Vec<T> {
    next.into_iter().filter(|v| !prev.contains(v)).collect()
}
