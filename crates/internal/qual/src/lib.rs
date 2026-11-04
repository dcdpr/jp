//! Compare rustqual findings between two revisions, finding by finding.
//!
//! rustqual reports what it finds in one tree.
//! This crate runs every pass of the [`policy`] over the working tree and over
//! a base revision, identifies each finding by its rule, file and symbol, and
//! keeps the ones the base doesn't have.
//! CI, `cargo_check` and the burn-down loop all call it, so they reach the same
//! verdict and print the same messages.
//!
//! [`Workspace`] is the entry point.
//! The core is pure: [`sarif`] reads a rustqual report, [`symbol`] names the
//! item a line sits in, [`diff`] compares two reports, and [`render`] prints
//! the result.
//! [`run`] is the shell around them that runs rustqual and git.
//!
//! Every failure is an error, never a pass: a report that can't be read, a
//! rustqual run that fails, or a source file that doesn't parse stops the
//! comparison.

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

/// One rustqual finding, located by the item it sits in.
///
/// Fields are declared in the order findings sort by.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Finding {
    /// The file, relative to the workspace root, with `/` separators.
    ///
    /// Empty for a finding about the whole workspace.
    pub file: String,

    /// The 1-based line, or 0 when the finding has no location.
    ///
    /// Not part of the finding's identity, so editing code above it changes
    /// nothing.
    pub line: usize,

    /// The rule ID, as rustqual names it: `CX-001`, `DRY-002`,
    /// `iosp/violation`.
    pub rule: String,

    /// The path of the item the finding sits in, such as `Parser::run`.
    ///
    /// Empty for a finding about a whole file, or outside every item.
    pub symbol: String,

    /// rustqual's description of the finding.
    pub message: String,

    /// The name of the pass that reported it.
    pub pass: String,
}

impl Finding {
    /// What identifies the finding across revisions: its rule, file and symbol.
    ///
    /// Two findings can share a key, such as two magic numbers in one function;
    /// they are then compared by how many there are.
    #[must_use]
    pub fn key(&self) -> (&str, &str, &str) {
        (&self.rule, &self.file, &self.symbol)
    }
}

/// Every finding in one tree, across all passes, sorted.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub findings: Vec<Finding>,

    /// Rule ID to its one-line title, for the rules rustqual described.
    pub titles: BTreeMap<String, String>,
}
