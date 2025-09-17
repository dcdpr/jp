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
}
