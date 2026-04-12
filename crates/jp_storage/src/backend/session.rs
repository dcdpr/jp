//! Session backend trait for session-to-conversation mapping storage.

use std::fmt::Debug;

use serde_json::Value;

/// Session-to-conversation mapping storage.
///
/// Methods use [`serde_json::Value`] instead of generic `T` to keep the trait
/// dyn-compatible (object-safe). Callers serialize/deserialize at the call
/// site.
pub trait SessionBackend: Send + Sync + Debug {
    /// Load a session mapping as a JSON value.
    ///
    /// Returns `Ok(None)` if no mapping exists for the given key.
    fn load_session(&self, session_key: &str) -> crate::error::Result<Option<Value>>;

    /// Save a session mapping from a JSON value.
    fn save_session(&self, session_key: &str, data: &Value) -> crate::error::Result<()>;

    /// List all session mapping keys.
    fn list_session_keys(&self) -> Vec<String>;
}
