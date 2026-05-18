#![allow(clippy::result_large_err)]

pub mod helpers;

#[cfg(feature = "config")]
mod config;

/// Built-in `parse_env` functions.
#[cfg(all(feature = "config", feature = "env"))]
pub mod env;

#[cfg(feature = "config")]
#[doc(hidden)]
pub mod internal;

/// Built-in `merge` functions.
#[cfg(feature = "config")]
pub mod merge;

/// Generate schemas to render into outputs.
#[cfg(feature = "schema")]
pub mod schema;

#[doc(hidden)]
pub use ::serde;
#[cfg(feature = "config")]
pub use config::*;
pub use schematic_macros::*;
pub use schematic_types::{Schema, SchemaBuilder, SchemaType, Schematic};
// Re-export serde_content for use in macros
pub use serde_content;
