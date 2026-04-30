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

/// Parsed counts and start positions extracted from a `@@ ... @@` header.
#[derive(Debug, Clone, Copy)]
pub struct HunkCounts {
    pub old_start: usize,
    pub old_count: usize,
    pub new_count: usize,
}

/// Parse the `-OS,N` and `+Y,M` ranges from a hunk header line.
///
/// Accepts both forms with explicit count (`-5,2`) and the implicit
/// single-line form (`-5`, equivalent to `-5,1`). Anything after the second
/// `@@` is ignored.
pub fn parse_hunk_counts(header_line: &str) -> Option<HunkCounts> {
    let mut parts = header_line.split_whitespace();
    parts.next()?; // leading `@@`
    let old_part = parts.next()?.strip_prefix('-')?;
    let new_part = parts.next()?.strip_prefix('+')?;

    let (old_start, old_count) = parse_range(old_part)?;
    let (_new_start, new_count) = parse_range(new_part)?;

    Some(HunkCounts {
        old_start,
        old_count,
        new_count,
    })
}

fn parse_range(s: &str) -> Option<(usize, usize)> {
    let mut it = s.split(',');
    let start: usize = it.next()?.parse().ok()?;
    let count: usize = it.next().map_or(Some(1), |c| c.parse().ok())?;
    Some((start, count))
}

/// Rewrite a hunk's header to use a canonical `+Y` line number.
///
/// Hunks emitted by `git diff-files --unified=0` carry `+Y` values that
/// reflect the cumulative line shift of every preceding unstaged hunk in
/// the same file. When staging only a subset of those hunks, the `+Y` of
/// each selected hunk must be recomputed: `git apply --cached
/// --unidiff-zero` positions changes by `+Y`, so a stale offset places
/// the patch at the wrong line.
///
/// `cumulative_offset` is the net `(additions - removals)` line change
/// produced by preceding *selected* hunks in the assembled patch, so that
/// the rewritten `+Y` reflects the source state at the point this hunk
/// is applied.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
pub fn rewrite_hunk_y(hunk: &str, cumulative_offset: isize) -> Option<(String, HunkCounts)> {
    let (header_line, body) = hunk.split_once('\n')?;
    let counts = parse_hunk_counts(header_line)?;

    // Canonical `+Y` assuming the hunk applies at its old position to the
    // source state implied by `cumulative_offset`.
    let canonical_y = if counts.old_count > 0 {
        counts.old_start as isize
    } else {
        counts.old_start as isize + 1
    };
    let new_y = (canonical_y + cumulative_offset).max(0) as usize;

    // Preserve any trailing context after the second `@@`.
    let trailing = header_line
        .splitn(4, ' ')
        .nth(3)
        .map_or(String::new(), |t| format!(" {t}"));

    let new_header = format!(
        "@@ -{},{} +{},{} @@{}",
        counts.old_start, counts.old_count, new_y, counts.new_count, trailing
    );

    Some((format!("{new_header}\n{body}"), counts))
}

#[cfg(test)]
#[path = "hunk_tests.rs"]
mod tests;
