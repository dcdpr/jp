use std::sync::Mutex;

use camino::Utf8PathBuf;
use open_editor::{Editor, EditorCallBuilder, errors::OpenEditorError};
use serde::Serialize;

/// Backend for opening an editor to modify text.
///
/// This trait abstracts the editor interaction, allowing tests to mock the
/// editor without actually opening an external process.
pub trait EditorBackend: Send + Sync {
    /// Opens an editor with the given content and returns the modified content.
    fn edit(&self, content: &str) -> Result<String, OpenEditorError>;
}

/// Terminal editor implementation using the `open-editor` crate.
pub struct TerminalEditorBackend {
    pub path: Utf8PathBuf,
}

impl EditorBackend for TerminalEditorBackend {
    fn edit(&self, content: &str) -> Result<String, OpenEditorError> {
        EditorCallBuilder::new()
            .with_editor(Editor::from_bin_path(self.path.as_std_path().into()))
            .edit_string(content)
    }
}

/// Mock editor backend for testing.
///
/// Returns pre-configured responses without opening an actual editor.
pub struct MockEditorBackend {
    responses: Mutex<Vec<String>>,
}

impl MockEditorBackend {
    /// Creates a mock that returns the given responses in sequence.
    ///
    /// Each call to `edit()` consumes one response. If all responses are
    /// exhausted, subsequent calls return an empty string.
    #[must_use]
    pub fn with_responses(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(Into::into).collect()),
        }
    }

    /// Creates a mock that always returns the same response.
    #[must_use]
    pub fn always(response: impl Into<String>) -> Self {
        Self::with_responses([response.into()])
    }

    /// Creates a mock that returns empty content (triggers fallback to Ask).
    #[must_use]
    pub fn empty() -> Self {
        Self::always("")
    }

    /// Creates a mock that returns invalid JSON (triggers retry prompt).
    #[must_use]
    pub fn invalid_json() -> Self {
        Self::always("{ invalid json }")
    }

    /// Creates a mock that returns valid JSON with the given value.
    ///
    /// # Panics
    ///
    /// This function panics if the value cannot be serialized to JSON.
    pub fn json<T: Serialize>(value: &T) -> Self {
        Self::always(serde_json::to_string_pretty(value).unwrap())
    }
}

impl EditorBackend for MockEditorBackend {
    fn edit(&self, _content: &str) -> Result<String, OpenEditorError> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            // If no more responses, return empty (simulates user clearing
            // content)
            Ok(String::new())
        } else {
            Ok(responses.remove(0))
        }
    }
}
