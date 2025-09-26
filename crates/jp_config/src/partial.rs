//! Configuration partial utilities.

use schematic::Config;

/// Convert a configuration into a partial configuration.
pub trait ToPartial: Config {
    /// Convert a configuration into a partial configuration.
    fn to_partial(&self) -> Self::Partial;
}

/// Get the current or default value, if any.
pub fn partial_opt<T: PartialEq + Clone>(current: &T, default: Option<T>) -> Option<T> {
    default
        .is_none_or(|v| &v != current)
        .then(|| current.clone())
}

/// Get the current or default value, if any.
pub fn partial_opts<T: PartialEq + Clone>(current: Option<&T>, default: Option<T>) -> Option<T> {
    match (current, default) {
        (Some(current), defaults) => partial_opt(current, defaults),
        (None, default) => default,
    }
}

/// Calculate the delta between two optional values.
pub fn partial_opt_config<T: ToPartial>(
    current: Option<&T>,
    default: Option<T::Partial>,
) -> Option<T::Partial>
where
    <T as Config>::Partial: PartialEq,
{
    match (current, default) {
        (None, default) => default,
        (Some(current), default) => {
            let current = current.to_partial();

            default.is_none_or(|v| v != current).then_some(current)
        }
    }
}
