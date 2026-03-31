use std::fmt;

use camino::{Utf8Path, Utf8PathBuf};
use jp_conversation::ConversationId;
use rayon::prelude::*;
use tracing::{debug, trace};

use crate::{CONVERSATIONS_DIR, EVENTS_FILE, METADATA_FILE, Storage, dir_entries};

/// Result of validating all conversation directories across storage roots.
#[derive(Debug, Default)]
pub struct ValidationResult {
    /// Conversations that passed all validation checks.
    pub valid: Vec<ValidConversation>,

    /// Conversations that failed one or more validation checks.
    pub invalid: Vec<InvalidConversation>,
}

/// A conversation directory that passed validation.
#[derive(Debug)]
pub struct ValidConversation {
    /// The parsed conversation ID.
    pub id: ConversationId,

    /// The directory name on disk.
    pub dirname: String,
}

/// A conversation directory that failed validation.
#[derive(Debug)]
pub struct InvalidConversation {
    /// The conversations root this entry lives under.
    /// Callers pass this back to [`Storage::trash_conversation`].
    pub(crate) conversations_dir: Utf8PathBuf,

    /// What went wrong.
    pub error: ValidationError,

    /// The directory name on disk.
    pub dirname: String,
}

/// The reason a conversation directory failed validation.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// The directory name could not be parsed as a [`ConversationId`].
    #[error("invalid directory name")]
    InvalidDirname,

    /// The per-conversation metadata file is missing.
    #[error("missing {METADATA_FILE}")]
    MissingMetadata,

    /// The per-conversation metadata file exists but is not valid JSON or
    /// is not a JSON object.
    #[error("{METADATA_FILE}: {source}")]
    CorruptMetadata {
        /// The underlying parse error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The per-conversation events file is missing.
    #[error("missing {EVENTS_FILE}")]
    MissingEvents,

    /// The per-conversation events file exists but is not a valid JSON array
    /// or its elements are missing required structural fields.
    #[error("{EVENTS_FILE}: {source}")]
    CorruptEvents {
        /// The underlying parse error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl fmt::Display for InvalidConversation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.dirname, self.error)
    }
}

impl Storage {
    /// Scan all conversation directories and check structural integrity.
    ///
    /// This is a lightweight check that runs on every startup. For each
    /// directory entry it verifies:
    ///
    /// 1. The directory name parses as a [`ConversationId`].
    /// 2. A `metadata.json` file exists and is a valid JSON object.
    /// 3. An `events.json` file exists and is a JSON array where each
    ///    element contains `timestamp` and `kind` fields.
    ///
    /// No field values are materialized — the check uses [`IgnoredAny`] to
    /// skip values without allocating. Content-level issues (bad field
    /// values, missing optional fields, schema mismatches) are handled at
    /// load time by [`ConversationStream::sanitize`].
    ///
    /// Files and dot-prefixed directories (e.g., `.trash/`) are silently
    /// skipped.
    ///
    /// [`IgnoredAny`]: serde::de::IgnoredAny
    /// [`ConversationStream::sanitize`]: jp_conversation::ConversationStream::sanitize
    #[must_use]
    pub fn validate_conversations(&self) -> ValidationResult {
        trace!("Validating conversations.");

        let mut result = ValidationResult::default();

        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let conversations_dir = root.join(CONVERSATIONS_DIR);
            if !conversations_dir.is_dir() {
                continue;
            }

            validate_root(&conversations_dir, &mut result);
        }

        debug!(
            valid = result.valid.len(),
            invalid = result.invalid.len(),
            "Validated conversations.",
        );

        result
    }

    /// Trash a conversation that failed validation.
    ///
    /// Moves the conversation directory to `.trash/` and writes a `TRASHED.md`
    /// explaining the error. Uses the path information captured during
    /// validation.
    pub fn trash_conversation(&self, entry: &InvalidConversation) -> crate::error::Result<()> {
        let error_msg = entry.error.to_string();
        crate::trash::trash_conversation(&entry.conversations_dir, &entry.dirname, &error_msg)
    }
}

fn validate_root(conversations_dir: &Utf8Path, result: &mut ValidationResult) {
    trace!(root = %conversations_dir, "Validating conversation root.");

    let entries: Vec<_> = dir_entries(conversations_dir)
        .filter(|entry| {
            entry.file_type().ok().is_some_and(|ft| ft.is_dir())
                && !entry.file_name().starts_with('.')
        })
        .collect();

    let outcomes: Vec<_> = entries
        .par_iter()
        .map(|entry| validate_entry(conversations_dir, entry.file_name(), entry.path()))
        .collect();

    for outcome in outcomes {
        match outcome {
            Ok(v) => result.valid.push(v),
            Err(e) => result.invalid.push(e),
        }
    }
}

/// Validate a single conversation directory entry.
fn validate_entry(
    conversations_dir: &Utf8Path,
    dirname: &str,
    entry_path: &Utf8Path,
) -> Result<ValidConversation, InvalidConversation> {
    let id = ConversationId::try_from_dirname(dirname).map_err(|_| InvalidConversation {
        conversations_dir: conversations_dir.to_path_buf(),
        error: ValidationError::InvalidDirname,
        dirname: dirname.to_owned(),
    })?;

    let metadata_path = entry_path.join(METADATA_FILE);
    if !metadata_path.is_file() {
        return Err(InvalidConversation {
            conversations_dir: conversations_dir.to_path_buf(),
            error: ValidationError::MissingMetadata,
            dirname: dirname.to_owned(),
        });
    }
    validate_metadata(&metadata_path).map_err(|source| InvalidConversation {
        conversations_dir: conversations_dir.to_path_buf(),
        error: ValidationError::CorruptMetadata { source },
        dirname: dirname.to_owned(),
    })?;

    let events_path = entry_path.join(EVENTS_FILE);
    if !events_path.is_file() {
        return Err(InvalidConversation {
            conversations_dir: conversations_dir.to_path_buf(),
            error: ValidationError::MissingEvents,
            dirname: dirname.to_owned(),
        });
    }
    validate_events(&events_path).map_err(|source| InvalidConversation {
        conversations_dir: conversations_dir.to_path_buf(),
        error: ValidationError::CorruptEvents { source },
        dirname: dirname.to_owned(),
    })?;

    Ok(ValidConversation {
        id,
        dirname: dirname.to_owned(),
    })
}

/// Confirm `metadata.json` is a valid JSON object.
///
/// Uses [`IgnoredAny`] to skip all field values — no allocations, no
/// schema checks. Content validation happens at load time.
///
/// [`IgnoredAny`]: serde::de::IgnoredAny
fn validate_metadata(path: &Utf8Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use serde::de::IgnoredAny;

    // Confirm the file is valid JSON. IgnoredAny accepts any valid JSON
    // value (object, array, string, etc.) without allocating.
    let buf = std::fs::read(path)?;
    serde_json::from_slice::<IgnoredAny>(&buf)?;
    Ok(())
}

/// Confirm `events.json` is a JSON array of objects with `timestamp` and
/// `type` fields.
///
/// All elements in the array — both `ConfigDelta` and `ConversationEvent` —
/// serialize with `timestamp` and `type` as top-level fields. `type` is the
/// tag for the flattened `EventKind` enum on events, and an explicit
/// `"config_delta"` tag on config deltas.
///
/// Uses [`IgnoredAny`] for field values — the parser skips over them without
/// allocating. This confirms the structural shape without materializing any
/// event data.
///
/// [`IgnoredAny`]: serde::de::IgnoredAny
fn validate_events(path: &Utf8Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use serde::de::IgnoredAny;

    #[derive(serde::Deserialize)]
    struct EventProbe {
        #[expect(dead_code)]
        timestamp: IgnoredAny,
        #[expect(dead_code)]
        r#type: IgnoredAny,
    }

    let buf = std::fs::read(path)?;
    serde_json::from_slice::<Vec<EventProbe>>(&buf)?;
    Ok(())
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
