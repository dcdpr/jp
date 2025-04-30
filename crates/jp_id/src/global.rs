//! Globally unique ID.
//!
//! This module provides functionality for managing a globally unique ID.

use std::sync::OnceLock;

static ID: OnceLock<&'static str> = OnceLock::new();

/// Get a globally unique ID.
///
/// # Panics
///
/// Panics if [`set`] has not been called first.
pub fn get() -> &'static str {
    ID.get().expect("Global ID has not been initialized")
}

/// Initialize a globally unique ID.
pub fn set(id: String) {
    let id: &'static str = Box::leak(id.into_boxed_str());
    let _ = ID.set(id);
}
