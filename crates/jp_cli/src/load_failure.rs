//! The report of conversations that failed to load.
//!
//! Startup validation finds conversation directories JP cannot load and leaves
//! them where they are.
//! [`render`] turns what it found into the text printed on stderr after the
//! command's own output.

use camino::Utf8PathBuf;
use jp_storage::validate::InvalidConversation;

/// The most failures the report lists one by one; above it, it gives a count.
const LISTED_FAILURES: usize = 3;

/// A conversation directory that failed to load, as the report shows it.
#[derive(Debug)]
pub(crate) struct LoadFailure {
    /// The conversation ID, or the directory name when it is not one.
    pub name: String,
    pub path: Utf8PathBuf,
    pub error: String,
}

impl From<&InvalidConversation> for LoadFailure {
    fn from(invalid: &InvalidConversation) -> Self {
        Self {
            name: invalid
                .id()
                .map_or_else(|| invalid.dirname.clone(), |id| id.to_string()),
            path: invalid.path(),
            error: invalid.error.to_string(),
        }
    }
}

/// Render the report, or `None` when nothing failed to load.
///
/// Up to three failures are listed with their name, error, and path; more are
/// summarized as a count.
pub(crate) fn render(failures: &[LoadFailure]) -> Option<String> {
    let count = failures.len();
    let noun = if count == 1 {
        "conversation"
    } else {
        "conversations"
    };

    match count {
        0 => None,
        1..=LISTED_FAILURES => {
            let entries = failures
                .iter()
                .map(|failure| {
                    format!(
                        "  {}: {}\n    {}",
                        failure.name, failure.error, failure.path
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Some(format!("{count} {noun} failed to load:\n{entries}"))
        }
        _ => Some(format!(
            "{count} {noun} failed to load. Run with `-v` to list them."
        )),
    }
}

#[cfg(test)]
#[path = "load_failure_tests.rs"]
mod tests;
