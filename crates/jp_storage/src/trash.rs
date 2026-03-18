use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use chrono::Utc;

use crate::error::Error;

/// The name of the trash directory within the conversations directory.
pub const TRASH_DIR: &str = ".trash";

/// The name of the file describing why a conversation was trashed.
const TRASHED_FILE: &str = "TRASHED.md";

/// Move a conversation directory to the trash, preserving its contents.
///
/// The conversation directory at `{conversations_dir}/{dirname}` is moved to
/// `{conversations_dir}/.trash/{dirname}/`. A `TRASHED.md` file is written into
/// the trashed directory explaining why it was trashed.
///
/// If a directory with the same name already exists in `.trash/`, an integer
/// suffix is appended (e.g., `{dirname}-1`, `{dirname}-2`).
pub fn trash_conversation(
    conversations_dir: &Utf8Path,
    dirname: &str,
    error: &str,
) -> Result<(), Error> {
    let source = conversations_dir.join(dirname);
    let trash_dir = conversations_dir.join(TRASH_DIR);

    fs::create_dir_all(&trash_dir)?;

    let target = resolve_trash_target(&trash_dir, dirname);
    fs::rename(&source, &target)?;

    let content = format_trashed_md(error);
    fs::write(target.join(TRASHED_FILE), content)?;

    Ok(())
}

/// Find a non-colliding path in the trash directory.
///
/// If `{trash_dir}/{dirname}` doesn't exist, returns it directly.
/// Otherwise appends `-1`, `-2`, etc. until an available path is found.
fn resolve_trash_target(trash_dir: &Utf8Path, dirname: &str) -> Utf8PathBuf {
    let candidate = trash_dir.join(dirname);
    if !candidate.exists() {
        return candidate;
    }

    for suffix in 1.. {
        let candidate = trash_dir.join(format!("{dirname}-{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!()
}

/// Format a `TRASHED.md` file for a trashed conversation.
fn format_trashed_md(error: &str) -> String {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    format!(
        "\
# Trashed Conversation

This conversation was moved here because it failed workspace sanitization.

**Error:** {error}
**Date:** {now}

The original conversation files are preserved alongside this file.
If the data is recoverable, you can fix the issue and move the
directory back to `.jp/conversations/`.
"
    )
}

#[cfg(test)]
#[path = "trash_tests.rs"]
mod tests;
