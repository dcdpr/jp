//! Editor invocation backends.
//!
//! [`EditorBackend`] is the frontend seam for running the user's configured
//! editor.
//! It offers two shapes of edit: [`edit_text`] for ephemeral
//! string-in/string-out editing, and [`edit_file`] for opening the editor on
//! caller-owned paths.
//!
//! [`TerminalEditorBackend`] spawns the editor as a local process from a
//! [`duct::Expression`], so it honours every flag the user attached to their
//! editor command.
//! [`MockEditorBackend`] scripts edited text for tests without spawning
//! anything.
//!
//! [`edit_file`]: EditorBackend::edit_file
//! [`edit_text`]: EditorBackend::edit_text

use std::{fs, io, sync::Mutex};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::NamedUtf8TempFile;
use duct::Expression;
use serde::Serialize;

/// Backend for invoking the user's configured editor.
///
/// Each frontend (terminal, web, native, mock) provides one implementation.
pub trait EditorBackend: Send + Sync {
    /// Edit `content` in the editor and return the edited text.
    ///
    /// On [`EditOutcome::Cancelled`] the returned string is meaningless and
    /// callers should ignore it.
    fn edit_text(&self, content: &str) -> Result<(EditOutcome, String), EditorError>;

    /// Open the editor on the requested path(s) and block until it exits.
    ///
    /// The edited content is read back from disk by the caller.
    fn edit_file(&self, req: EditRequest<'_>) -> Result<EditOutcome, EditorError>;
}

/// Frontend-agnostic request data for [`EditorBackend::edit_file`].
pub struct EditRequest<'a> {
    /// The path(s) to open in the editor.
    pub paths: &'a [Utf8PathBuf],

    /// Working directory for a spawned editor.
    ///
    /// Frontends that don't spawn a local process ignore it.
    pub cwd: Option<&'a Utf8Path>,
}

/// The interaction outcome of an editor session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    /// The user saved and closed (terminal editor exited zero).
    Saved,

    /// The user aborted (terminal editor exited non-zero).
    Cancelled,
}

/// An error from invoking the editor.
#[derive(Debug, thiserror::Error)]
pub enum EditorError {
    /// The editor process could not be spawned (e.g. the binary was not found).
    #[error("failed to spawn editor")]
    Spawn(#[source] std::io::Error),

    /// Reading or writing the file being edited failed.
    #[error("editor file I/O failed")]
    Io(#[source] std::io::Error),
}

/// Terminal editor backend: spawns the editor as a local process.
///
/// The path(s) being edited are appended as trailing arguments to the
/// configured command, preserving any flags the user attached (e.g.
/// `code --wait`).
pub struct TerminalEditorBackend {
    cmd: Expression,
}

impl TerminalEditorBackend {
    /// Create a backend that runs `cmd`, appending the edited path(s) as
    /// trailing arguments.
    #[must_use]
    pub fn new(cmd: Expression) -> Self {
        Self { cmd }
    }

    /// Spawn the editor on `paths`, mapping the exit status to an
    /// [`EditOutcome`].
    fn run(
        &self,
        paths: &[Utf8PathBuf],
        cwd: Option<&Utf8Path>,
    ) -> Result<EditOutcome, EditorError> {
        let args: Vec<String> = paths.iter().map(|p| p.as_str().to_owned()).collect();
        let cwd = cwd.map(ToOwned::to_owned);

        let output = self
            .cmd
            .clone()
            .before_spawn(move |cmd| {
                for arg in &args {
                    cmd.arg(arg);
                }
                if let Some(cwd) = &cwd {
                    cmd.current_dir(cwd);
                }
                Ok(())
            })
            .unchecked()
            .run()
            .map_err(EditorError::Spawn)?;

        Ok(if output.status.success() {
            EditOutcome::Saved
        } else {
            EditOutcome::Cancelled
        })
    }
}

impl EditorBackend for TerminalEditorBackend {
    fn edit_text(&self, content: &str) -> Result<(EditOutcome, String), EditorError> {
        let tmp = NamedUtf8TempFile::new().map_err(EditorError::Io)?;
        let path = tmp.path().to_owned();
        fs::write(&path, content).map_err(EditorError::Io)?;

        let outcome = self.run(std::slice::from_ref(&path), None)?;
        let edited = fs::read_to_string(&path).map_err(EditorError::Io)?;

        Ok((outcome, edited))
    }

    fn edit_file(&self, req: EditRequest<'_>) -> Result<EditOutcome, EditorError> {
        self.run(req.paths, req.cwd)
    }
}

/// Mock editor backend for testing.
///
/// Scripts the text returned by [`edit_text`] without spawning a process.
/// Every interaction reports [`EditOutcome::Saved`].
///
/// [`edit_text`]: EditorBackend::edit_text
pub struct MockEditorBackend {
    responses: Mutex<Vec<String>>,

    /// When set, every call fails with a spawn error instead of returning
    /// scripted text (simulates a broken or missing editor).
    fail: bool,
}

impl MockEditorBackend {
    /// Creates a mock that returns the given responses in sequence.
    ///
    /// Each call to `edit_text` consumes one response.
    /// If all responses are exhausted, subsequent calls return an empty string.
    #[must_use]
    pub fn with_responses(responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(Into::into).collect()),
            fail: false,
        }
    }

    /// Creates a mock whose `edit_text` / `edit_file` calls fail with a spawn
    /// error, simulating a broken or missing editor.
    #[must_use]
    pub fn failing() -> Self {
        Self {
            responses: Mutex::new(vec![]),
            fail: true,
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
    fn edit_text(&self, _content: &str) -> Result<(EditOutcome, String), EditorError> {
        if self.fail {
            return Err(mock_failure());
        }

        let mut responses = self.responses.lock().unwrap();
        let text = if responses.is_empty() {
            // If no more responses, return empty (simulates user clearing
            // content).
            String::new()
        } else {
            responses.remove(0)
        };

        Ok((EditOutcome::Saved, text))
    }

    fn edit_file(&self, _req: EditRequest<'_>) -> Result<EditOutcome, EditorError> {
        if self.fail {
            return Err(mock_failure());
        }

        Ok(EditOutcome::Saved)
    }
}

/// A spawn error used by [`MockEditorBackend::failing`] to simulate an editor
/// that can't be started.
fn mock_failure() -> EditorError {
    EditorError::Spawn(io::Error::new(
        io::ErrorKind::NotFound,
        "mock editor failure",
    ))
}
