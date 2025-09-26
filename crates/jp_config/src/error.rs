//! Configuration error.

/// Configuration error.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Configuration error.
    #[error(transparent)]
    Schematic(#[from] schematic::ConfigError),

    /// A glob iteration error.
    #[error(transparent)]
    Glob(#[from] glob::GlobError),

    /// A glob pattern parsing error.
    #[error(transparent)]
    Pattern(#[from] glob::PatternError),

    /// A custom configuration error.
    // TODO: Remove this once we can enable the `validation` feature for
    // `schematic` (currently broken in our own fork).
    #[error(transparent)]
    Custom(Box<dyn std::error::Error + Send + Sync>),
}
