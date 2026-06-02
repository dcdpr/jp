//! Cross-field validation for resolved configuration types.
//!
//! [`Validator`] is implemented by [`AppConfig`] and the nested sections it
//! owns.
//! The default returns `Ok(())`, so a section only needs a custom
//! implementation when it has an invariant to enforce.
//! Composite sections override it to recurse into their children, so a leaf
//! validator (such as the one on [`RequestConfig`]) is reached from the top via
//! that chain.
//!
//! [`AppConfig`]: crate::AppConfig
//! [`RequestConfig`]: crate::assistant::request::RequestConfig

use schematic::ConfigError;

/// Validate cross-field invariants on a resolved configuration value.
///
/// These are values that are individually well-typed but would break behavior
/// in practice, so they are rejected once the configuration is finalized rather
/// than silently honored.
pub trait Validator {
    /// Validate this configuration value.
    ///
    /// The default accepts everything; types with an invariant override it, and
    /// composite types override it to recurse into their children.
    ///
    /// # Errors
    ///
    /// Returns an error if the value (or one of its children) violates an
    /// invariant.
    fn validate(&self) -> Result<(), ConfigError> {
        Ok(())
    }
}
