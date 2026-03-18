use std::{fmt, fs, io::BufReader};

use camino::{Utf8Path, Utf8PathBuf};
use jp_conversation::{Conversation, ConversationId};

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

    /// The per-conversation metadata file exists but contains invalid data.
    #[error("{METADATA_FILE}: {}", source.to_string())]
    CorruptMetadata {
        /// The underlying parse error.
        source: Box<dyn std::error::Error>,
    },

    /// The per-conversation events file is missing.
    #[error("missing {EVENTS_FILE}")]
    MissingEvents,

    /// The per-conversation events file exists but contains invalid data.
    #[error("{EVENTS_FILE}: {}", source.to_string())]
    CorruptEvents {
        /// The underlying parse error.
        source: Box<dyn std::error::Error>,
    },
}

impl fmt::Display for InvalidConversation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.dirname, self.error)
    }
}

impl Storage {
    /// Scan all conversation directories and validate each one.
    ///
    /// Checks every entry in the `conversations/` directory under both storage
    /// roots (workspace and user). For each directory entry:
    ///
    /// 1. The directory name must parse as a [`ConversationId`].
    /// 2. A `metadata.json` file must exist and deserialize as [`Conversation`].
    /// 3. An `events.json` file must be structurally valid JSON (array of
    ///    objects with `timestamp` fields).
    ///
    /// Files and dot-prefixed directories (e.g., `.trash/`) are silently
    /// skipped.
    #[must_use]
    pub fn validate_conversations(&self) -> ValidationResult {
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
    for entry in dir_entries(conversations_dir) {
        let dirname = entry.file_name().to_owned();

        if !entry.file_type().ok().is_some_and(|ft| ft.is_dir()) {
            continue;
        }

        if dirname.starts_with('.') {
            continue;
        }

        let Ok(id) = ConversationId::try_from_dirname(&dirname) else {
            result.invalid.push(InvalidConversation {
                conversations_dir: conversations_dir.to_path_buf(),
                error: ValidationError::InvalidDirname,
                dirname,
            });

            continue;
        };

        let entry_path = entry.into_path();

        // Validate metadata.json.
        let metadata_path = entry_path.join(METADATA_FILE);
        if !metadata_path.is_file() {
            result.invalid.push(InvalidConversation {
                conversations_dir: conversations_dir.to_path_buf(),
                error: ValidationError::MissingMetadata,
                dirname,
            });

            continue;
        }
        if let Err(source) = validate_metadata(&metadata_path) {
            result.invalid.push(InvalidConversation {
                conversations_dir: conversations_dir.to_path_buf(),
                error: ValidationError::CorruptMetadata { source },
                dirname,
            });

            continue;
        }

        // Validate events.json (lightweight structural check).
        let events_path = entry_path.join(EVENTS_FILE);
        if !events_path.is_file() {
            result.invalid.push(InvalidConversation {
                conversations_dir: conversations_dir.to_path_buf(),
                error: ValidationError::MissingEvents,
                dirname,
            });

            continue;
        }
        if let Err(source) = validate_events(&events_path) {
            result.invalid.push(InvalidConversation {
                conversations_dir: conversations_dir.to_path_buf(),
                error: ValidationError::CorruptEvents { source },
                dirname,
            });

            continue;
        }

        result.valid.push(ValidConversation { id, dirname });
    }
}

/// Validate that a metadata.json file deserializes as a [`Conversation`].
fn validate_metadata(path: &Utf8Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    serde_json::from_reader(reader)
        .map(|_: Conversation| ())
        .map_err(Into::into)
}

/// Lightweight structural validation for events.json.
///
/// Confirms the file is valid JSON, the top-level structure is an array, and
/// each element has a `timestamp` field. Does NOT fully deserialize event
/// variants — that's deferred to lazy loading.
fn validate_events(path: &Utf8Path) -> Result<(), Box<dyn std::error::Error>> {
    #[derive(serde::Deserialize)]
    struct RawEvent {
        #[expect(dead_code)]
        timestamp: Box<serde_json::value::RawValue>,
    }

    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    serde_json::from_reader(reader)
        .map(|_: Vec<RawEvent>| ())
        .map_err(Into::into)
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
