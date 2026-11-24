//! What the reviewer has approved and read of a review's files, which a pass
//! tells the assistant beside the comments.
//!
//! An [`Approval`] says a passage is fine as it stands.
//! It holds while the lines of the passage are unchanged: once one changes, the
//! anchor no longer carries to the latest version, and the passage is not
//! approved any more.
//!
//! A [`ReadUntil`] says how far down a file the reviewer has read.
//! The lines after it are unread, so that they carry no comment says nothing
//! about them.
//! It says nothing about the lines before it either: only an approval approves.
//!
//! [`Progress`] holds both, and changes them without asking whether the review
//! takes changes: a caller checks `Review::writable` first, as it does before
//! recording a version to read an anchor in.
//!
//! In `review.json`:
//!
//! ```json
//! "approvals": [{
//!   "number": 1,
//!   "author": "Jane Doe <jane@example.com>",
//!   "created_at": "2026-11-17T10:01:00Z",
//!   "anchor": { "version": 2, "path": "docs/a.md", "range": "12-18" }
//! }],
//! "read_until": {
//!   "docs/a.md": {
//!     "version": 2,
//!     "line": 120,
//!     "author": "Jane Doe <jane@example.com>",
//!     "created_at": "2026-11-17T10:02:00Z"
//!   }
//! }
//! ```

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    anchor::{Anchor, Target},
    model::Refusal,
};

/// What the reviewer has approved and read of a review's files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<Approval>,

    /// How far the reviewer has read each file, by its path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub read_until: BTreeMap<String, ReadUntil>,
}

/// A passage the reviewer approved: a range of a file, in the version it was
/// read in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub number: u32,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub anchor: Anchor,
}

/// How far down a file the reviewer has read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadUntil {
    /// The version `line` is counted in.
    pub version: u32,

    /// The last line read.
    pub line: usize,

    pub author: String,
    pub created_at: DateTime<Utc>,
}

impl Progress {
    /// Approve the range `anchor` names, returning the approval's number.
    ///
    /// A range approved already keeps the approval it has.
    pub fn approve(
        &mut self,
        anchor: Anchor,
        author: String,
        at: DateTime<Utc>,
    ) -> Result<u32, Refusal> {
        if !matches!(anchor.target, Target::Range(..)) {
            return Err(Refusal::ApprovalWithoutRange);
        }
        if let Some(standing) = self.approvals.iter().find(|found| found.anchor == anchor) {
            return Ok(standing.number);
        }

        let highest = self.approvals.iter().map(|found| found.number).max();
        let number = highest.unwrap_or(0) + 1;
        self.approvals.push(Approval {
            number,
            author,
            created_at: at,
            anchor,
        });

        Ok(number)
    }

    /// Take back the approval numbered `number`.
    pub fn unapprove(&mut self, number: u32) -> Result<(), Refusal> {
        let index = self
            .approvals
            .iter()
            .position(|found| found.number == number)
            .ok_or(Refusal::Approval(number))?;
        self.approvals.remove(index);

        Ok(())
    }

    /// Record how far the file `path` was read, in place of what was recorded
    /// for it before.
    pub fn read(&mut self, path: String, until: ReadUntil) {
        self.read_until.insert(path, until);
    }

    /// Forget how far the file `path` was read, returning `false` when nothing
    /// was recorded for it.
    pub fn unread(&mut self, path: &str) -> bool {
        self.read_until.remove(path).is_some()
    }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
