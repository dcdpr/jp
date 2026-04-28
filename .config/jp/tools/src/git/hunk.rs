//! Content-addressed identification for diff hunks.
//!
//! Patch IDs are derived from the hunk text itself rather than from the
//! position of the hunk in a `git diff-files` snapshot. A positional ID
//! shifts whenever the index is mutated (a successful stage removes a
//! hunk and renumbers its peers), which silently aliases stale IDs to the
//! wrong content. Hashing the hunk text decouples the ID from snapshot
//! ordering: the same logical change always has the same ID, and a hunk
//! that's been staged simply disappears from the listing instead of
//! shifting indices around it.
//!
//! On collisions: 12 hex chars (48 bits) gives a collision probability
//! around 1e-12 for typical per-file diff sizes, well below the noise
//! floor for what this tool is used for. Cross-file collisions are
//! impossible because IDs are scoped per file at the API layer.

use sha1::{Digest, Sha1};

const HUNK_ID_HEX_LEN: usize = 12;

/// Compute a stable identifier for a hunk from its raw text.
///
/// The hunk text should be the full `@@ ...` form including the header.
/// Trailing newlines are stripped before hashing so that hunks split out
/// of a multi-hunk diff hash identically regardless of position.
pub fn hunk_id(hunk_text: &str) -> String {
    let canonical = hunk_text.trim_end_matches('\n');
    let digest = Sha1::digest(canonical.as_bytes());

    let mut id = String::with_capacity(HUNK_ID_HEX_LEN);
    for byte in &digest {
        if id.len() >= HUNK_ID_HEX_LEN {
            break;
        }
        id.push_str(&format!("{byte:02x}"));
    }
    id.truncate(HUNK_ID_HEX_LEN);
    id
}

/// Split a `git diff-files -p --unified=0` stdout into individual hunks.
///
/// Each returned string includes the `@@ ...` header. The diff header
/// (everything before the first hunk) is dropped. Hunks preserve their
/// file order, which matters for `git apply` since hunks in a single
/// patch must appear in increasing line-number order.
pub fn split_hunks(diff_stdout: &str) -> Vec<String> {
    diff_stdout
        .split("\n@@ ")
        .skip(1)
        .map(|h| format!("@@ {h}"))
        .collect()
}

/// Extract the diff header (everything before the first hunk).
///
/// Preserves headers like `deleted file mode`, `index ...`, `--- a/...`,
/// `+++ b/...` that `git apply` needs to correctly stage deletions and
/// renames.
pub fn diff_header(diff_stdout: &str) -> Option<&str> {
    diff_stdout.split_once("\n@@ ").map(|(header, _)| header)
}

#[cfg(test)]
#[path = "hunk_tests.rs"]
mod tests;
