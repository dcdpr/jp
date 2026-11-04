//! Comparing the findings of a base revision with those of the tree being
//! measured.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{Finding, Policy, Report};

/// Files renamed between the base and the measured tree, old path to new.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Renames(BTreeMap<String, String>);

/// `git diff --name-status -z` output that doesn't have the expected shape.
#[derive(Debug, thiserror::Error)]
#[error("unexpected `git diff --name-status -z` output: {0}")]
pub struct RenamesError(String);

impl Renames {
    /// Read the renames from `git diff -M --name-status -z` output.
    ///
    /// Every other status is skipped.
    ///
    /// # Errors
    ///
    /// Returns an error when a status isn't followed by the paths it needs.
    pub fn from_name_status(output: &str) -> Result<Self, RenamesError> {
        let mut renames = BTreeMap::new();
        let mut fields = output.split('\0').filter(|field| !field.is_empty());

        while let Some(status) = fields.next() {
            let missing = || RenamesError(format!("status {status:?} has no path"));
            let from = fields.next().ok_or_else(missing)?;

            // A rename or a copy names its source and then its destination.
            if !status.starts_with(['R', 'C']) {
                continue;
            }
            let to = fields.next().ok_or_else(missing)?;
            if status.starts_with('R') {
                renames.insert(from.to_owned(), to.to_owned());
            }
        }

        Ok(Self(renames))
    }

    /// Where `file` lives in the measured tree.
    #[must_use]
    pub fn new_path<'a>(&'a self, file: &'a str) -> &'a str {
        self.0.get(file).map_or(file, String::as_str)
    }
}

/// How many findings share a key with a new one.
///
/// Findings that share a rule, file and symbol can't be told apart, so when the
/// base had some and the tree has more, every one of them is listed and this
/// says how many are new.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Shared {
    /// How many more there are than at the base.
    pub new: usize,

    /// How many there are in the measured tree.
    pub of: usize,
}

/// A finding the base doesn't have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewFinding {
    #[serde(flatten)]
    pub finding: Finding,

    /// Set when the finding shares its key with findings the base has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared: Option<Shared>,
}

/// What the measured tree adds, and what fails regardless.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    /// Findings the base doesn't have, in rules that aren't enforced.
    pub new: Vec<NewFinding>,

    /// Every finding of an enforced rule, new or not.
    pub enforced: Vec<Finding>,

    /// How many gating findings each pass reported at the base.
    pub base: BTreeMap<String, usize>,

    /// How many gating findings each pass reported in the measured tree.
    pub head: BTreeMap<String, usize>,

    /// Rule ID to its one-line title.
    #[serde(skip)]
    pub titles: BTreeMap<String, String>,
}

impl Verdict {
    /// Whether the measured tree passes the gate.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.new.is_empty() && self.enforced.is_empty()
    }

    /// The rules of every finding in the verdict.
    #[must_use]
    pub fn rules(&self) -> BTreeSet<&str> {
        self.new
            .iter()
            .map(|new| new.finding.rule.as_str())
            .chain(self.enforced.iter().map(|finding| finding.rule.as_str()))
            .collect()
    }
}

/// Compare `head` against `base`.
///
/// Exempt rules are left out.
/// A base finding in a renamed file is compared under the file's new path.
#[must_use]
pub fn verdict(base: &Report, head: &Report, renames: &Renames, policy: &Policy) -> Verdict {
    let gating = |finding: &&Finding| policy.gates(&finding.rule);
    let compared = |finding: &&Finding| !policy.is_enforced(&finding.rule);

    let mut was = BTreeMap::<_, usize>::new();
    for finding in base.findings.iter().filter(gating).filter(compared) {
        let file = renames.new_path(&finding.file);
        *was.entry((finding.rule.as_str(), file, finding.symbol.as_str()))
            .or_default() += 1;
    }

    let mut groups = BTreeMap::<_, Vec<&Finding>>::new();
    for finding in head.findings.iter().filter(gating).filter(compared) {
        groups.entry(finding.key()).or_default().push(finding);
    }

    let mut new = Vec::new();
    for (key, group) in groups {
        let had = was.get(&key).copied().unwrap_or(0);
        if group.len() <= had {
            continue;
        }

        let shared = (had > 0).then_some(Shared {
            new: group.len() - had,
            of: group.len(),
        });
        new.extend(group.into_iter().map(|finding| NewFinding {
            finding: finding.clone(),
            shared,
        }));
    }
    new.sort_by(|a, b| a.finding.cmp(&b.finding));

    let enforced = head
        .findings
        .iter()
        .filter(|finding| policy.is_enforced(&finding.rule))
        .cloned()
        .collect();

    Verdict {
        new,
        enforced,
        base: counts(base, policy),
        head: counts(head, policy),
        titles: head.titles.clone(),
    }
}

/// How many gating findings each pass in `policy` contributed to `report`.
fn counts(report: &Report, policy: &Policy) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<_, _> = policy
        .passes
        .iter()
        .map(|pass| (pass.name.clone(), 0))
        .collect();
    for finding in report.findings.iter().filter(|f| policy.gates(&f.rule)) {
        *counts.entry(finding.pass.clone()).or_default() += 1;
    }
    counts
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
