//! Configuration error.

use std::path::PathBuf;

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

    /// An `extends` chain forms a cycle.
    ///
    /// `chain` is the full sequence of files that produced the cycle. The
    /// first and last entries refer to the same (canonicalized) file.
    #[error("configuration `extends` cycle detected: {}", format_chain(chain))]
    ExtendsCycle {
        /// The files that form the cycle, in traversal order.
        chain: Vec<PathBuf>,
    },

    /// An `extends` chain exceeded the maximum supported nesting depth.
    ///
    /// This is a safety net for the unlikely case where cycle detection fails
    /// to catch a genuine cycle (e.g. path canonicalization fails and lets two
    /// logically identical paths compare unequal).
    #[error(
        "configuration `extends` chain exceeded maximum depth of {limit}: {}",
        format_chain(chain)
    )]
    ExtendsDepthExceeded {
        /// The configured depth cap.
        limit: u8,
        /// The ancestor chain at the point the cap was hit.
        chain: Vec<PathBuf>,
    },

    /// A custom configuration error.
    // TODO: Remove this once we can enable the `validation` feature for
    // `schematic` (currently broken in our own fork).
    #[error(transparent)]
    Custom(Box<dyn std::error::Error + Send + Sync>),
}

/// Render a path chain as `a -> b -> c` for error messages.
fn format_chain(chain: &[PathBuf]) -> String {
    chain
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" -> ")
}
