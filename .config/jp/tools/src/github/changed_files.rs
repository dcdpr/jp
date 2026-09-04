//! Rendering for the changed-file lists that `github_commit` and
//! `github_pr_diff` both return.
//!
//! Both tools enumerate a page of changed files, and both are asked for patches
//! by filename.
//! [`format_changed_files`] renders the enumeration and [`format_not_found`]
//! reports the filenames a searched page did not hold.
//! The header each tool puts above the list is its own.

use std::fmt::{self, Write as _};

use jp_github::models::repos::DiffEntryStatus;

use crate::to_list_with_root;

/// What to do about a requested file that the searched page does not contain.
///
/// The same for every such file, so it is stated once below the list rather
/// than repeated per entry.
const NOT_FOUND_HINT: &str = "Not present on this page. Bump `page`, or call without `files` to \
                              enumerate the changed files and locate the right page.";

/// One changed file in an enumeration, rendered as a single list entry.
pub struct ChangedFile {
    pub filename: String,
    pub status: DiffEntryStatus,
    pub additions: u64,
    pub deletions: u64,
    /// The path this file was renamed or copied from, when it was.
    pub previous_filename: Option<String>,
}

impl fmt::Display for ChangedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut notes = vec![match &self.previous_filename {
            Some(previous) => format!("{} from {previous}", status_word(self.status)),
            None => status_word(self.status).to_owned(),
        }];

        let mut stat = String::new();
        if self.additions > 0 {
            write!(stat, "+{}", self.additions)?;
        }
        if self.deletions > 0 {
            if !stat.is_empty() {
                stat.push(',');
            }
            write!(stat, "-{}", self.deletions)?;
        }
        if !stat.is_empty() {
            notes.push(stat);
        }

        write!(f, "{} ({})", self.filename, notes.join(", "))
    }
}

fn status_word(status: DiffEntryStatus) -> &'static str {
    match status {
        DiffEntryStatus::Added => "added",
        DiffEntryStatus::Removed => "removed",
        DiffEntryStatus::Modified => "modified",
        DiffEntryStatus::Renamed => "renamed",
        DiffEntryStatus::Copied => "copied",
        DiffEntryStatus::Changed => "changed",
        DiffEntryStatus::Unchanged => "unchanged",
        DiffEntryStatus::Unknown => "unknown",
    }
}

/// Render a page of changed files as a bulleted block.
///
/// A page past the end of the list holds nothing and gets a sentence instead,
/// so an exhausted walk is not an empty block.
pub fn format_changed_files(files: &[ChangedFile]) -> String {
    if files.is_empty() {
        return "No changed files on this page.".to_owned();
    }

    to_list_with_root(files, "files")
}

/// Render the requested filenames the searched page did not contain.
pub fn format_not_found(files: &[String]) -> String {
    format!(
        "{}\n\n{NOT_FOUND_HINT}",
        to_list_with_root(files, "not_found")
    )
}

#[cfg(test)]
#[path = "changed_files_tests.rs"]
mod tests;
