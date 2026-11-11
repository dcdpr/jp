//! Compare `rustqual` findings across git revisions.
//!
//! This crate runs every pass configured in [`policy`] over both the working
//! tree and a base revision, keys each finding by `(rule, file, symbol)`, and
//! reports findings introduced since the base revision along with any findings
//! for enforced rules.
//! A finding whose symbol moved to another file is followed there, so moving a
//! function introduces nothing.
//!
//! [`Workspace`] is the entry point.
//! The core modules are pure: [`sarif`] parses `rustqual` SARIF output,
//! [`symbol`] resolves source lines to their enclosing Rust items, [`diff`]
//! compares two reports, and [`render`] formats the output.
//! [`run`] is the I/O shell that invokes `rustqual` and `git`.
//!
//! Analysis fails closed: an unreadable report, a failed `rustqual` run, a
//! skipped file, or an unparseable Rust source file returns an error rather
//! than passing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod diff;
pub mod policy;
pub mod render;
pub mod run;
pub mod sarif;
pub mod symbol;

pub use diff::{NewFinding, Renames, Shared, Verdict};
pub use policy::Policy;
pub use render::{Format, Guidance};
pub use run::{Error, Workspace};

/// A `rustqual` finding resolved to its enclosing Rust item.
///
/// Fields are ordered to define the [`Ord`] sort order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Finding {
    /// Workspace-relative file path with `/` separators, or empty for
    /// workspace-wide findings.
    pub file: String,

    /// 1-based line number, or `0` when the finding has no line location.
    ///
    /// Excluded from [`Finding::key`] so line shifts do not change the
    /// finding's identity.
    pub line: usize,

    /// SARIF rule ID (e.g. `CX-001`, `DRY-002`, `iosp/violation`).
    pub rule: String,

    /// File-relative path of the enclosing item (e.g. `Parser::run`), or empty
    /// for file-level and top-level findings.
    pub symbol: String,

    pub message: String,

    pub pass: String,
}

impl Finding {
    /// Identity key across revisions: `(rule, file, symbol)`.
    ///
    /// Multiple findings with the same key (such as two magic numbers in the
    /// same function) are compared by count.
    #[must_use]
    pub fn key(&self) -> (&str, &str, &str) {
        (&self.rule, &self.file, &self.symbol)
    }
}

/// Aggregated findings across all configured passes for a single tree, sorted
/// by [`Finding`]'s [`Ord`] implementation.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub findings: Vec<Finding>,

    /// Map from rule ID to its short description from the SARIF rules table.
    pub titles: BTreeMap<String, String>,
}
