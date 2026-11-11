//! Diffing `rustqual` findings between a base revision and a target tree.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{Finding, Policy, Report};

/// File renames between the base revision and the target tree, mapping old path
/// to new path.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Renames(BTreeMap<String, String>);

/// Malformed `git diff --name-status -z` output.
#[derive(Debug, thiserror::Error)]
#[error("unexpected `git diff --name-status -z` output: {0}")]
pub struct RenamesError(String);

impl Renames {
    /// Parse file renames (`R*`) from `git diff -M --name-status -z` output,
    /// ignoring all other status entries.
    ///
    /// # Errors
    ///
    /// Returns an error if a status token is missing its path field(s).
    pub fn from_name_status(output: &str) -> Result<Self, RenamesError> {
        let mut renames = BTreeMap::new();
        let mut fields = output.split('\0').filter(|field| !field.is_empty());

        while let Some(status) = fields.next() {
            let missing = || RenamesError(format!("status {status:?} has no path"));
            let from = fields.next().ok_or_else(missing)?;

            // `R` (rename) and `C` (copy) entries carry two NUL-terminated
            // paths (source and destination); other statuses carry one.
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

    /// Return the path of `file` in the target tree, or `file` unchanged if it
    /// was not renamed.
    #[must_use]
    pub fn new_path<'a>(&'a self, file: &'a str) -> &'a str {
        self.0.get(file).map_or(file, String::as_str)
    }
}

/// Count increase for findings that share a `(rule, file, symbol)` key.
///
/// Because findings with the same key cannot be distinguished individually, all
/// instances in the target tree are reported when the count increases over the
/// base revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Shared {
    /// Net increase in findings for this key compared to the base revision,
    /// after subtracting findings that moved in from another file.
    pub new: usize,

    /// Total findings with this key in the target tree.
    pub of: usize,
}

/// A finding introduced since the base revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewFinding {
    #[serde(flatten)]
    pub finding: Finding,

    /// Present when the base revision accounts for some of the findings with
    /// the same [`Finding::key`]: findings it had under that key, or findings
    /// that moved in from another file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared: Option<Shared>,
}

/// Result of evaluating a target report under a [`Policy`], optionally compared
/// against a base report.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    /// New findings for non-enforced gating rules.
    pub new: Vec<NewFinding>,

    /// All findings in the target tree for enforced rules, whether or not they
    /// existed at the base revision.
    pub enforced: Vec<Finding>,

    /// Gating finding count per pass at the base revision, or `None` when
    /// evaluated without a base revision.
    pub base: Option<BTreeMap<String, usize>>,

    /// Gating finding count per pass in the target tree.
    pub head: BTreeMap<String, usize>,

    /// Map from rule ID to its short description.
    #[serde(skip)]
    pub titles: BTreeMap<String, String>,
}

impl Verdict {
    /// Return `true` if there are no new or enforced findings.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.new.is_empty() && self.enforced.is_empty()
    }

    /// Distinct rule IDs present in `new` and `enforced`.
    #[must_use]
    pub fn rules(&self) -> BTreeSet<&str> {
        self.new
            .iter()
            .map(|new| new.finding.rule.as_str())
            .chain(self.enforced.iter().map(|finding| finding.rule.as_str()))
            .collect()
    }
}

/// Compare `head` against `base` under `policy`.
///
/// Exempt rules are excluded, and base findings in renamed files are mapped to
/// their paths in `head` before comparison.
///
/// A finding follows its symbol to another file: when a `(rule, symbol)` pair
/// has fewer findings in one file than at the base, that many findings of the
/// same pair in other files are not new.
/// A function moved between files therefore keeps the findings it had.
/// Findings without a symbol are tied to their file and never follow.
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

    // How many findings of each `(rule, symbol)` pair left a file they were in
    // at the base. Empty symbols are skipped: a file-level finding that goes
    // away in one file says nothing about the same rule firing in another.
    let mut departed = BTreeMap::<_, usize>::new();
    for (&(rule, file, symbol), &had) in &was {
        let has = groups.get(&(rule, file, symbol)).map_or(0, Vec::len);
        if !symbol.is_empty() && had > has {
            *departed.entry((rule, symbol)).or_default() += had - has;
        }
    }

    let mut new = Vec::new();
    for (key, group) in groups {
        let mut had = was.get(&key).copied().unwrap_or(0);
        if group.len() > had
            && let Some(departed) = departed.get_mut(&(key.0, key.2))
        {
            let moved = (*departed).min(group.len() - had);
            *departed -= moved;
            had += moved;
        }
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

    Verdict {
        new,
        enforced: enforced(head, policy),
        base: Some(counts(base, policy)),
        head: counts(head, policy),
        titles: head.titles.clone(),
    }
}

/// Evaluate `head` under `policy` without a base revision.
///
/// Because there is no base report, [`Verdict::new`] is empty and
/// [`Verdict::base`] is `None`; only findings for enforced rules fail the
/// verdict.
#[must_use]
pub fn enforced_only(head: &Report, policy: &Policy) -> Verdict {
    Verdict {
        new: Vec::new(),
        enforced: enforced(head, policy),
        base: None,
        head: counts(head, policy),
        titles: head.titles.clone(),
    }
}

fn enforced(head: &Report, policy: &Policy) -> Vec<Finding> {
    head.findings
        .iter()
        .filter(|finding| policy.is_enforced(&finding.rule))
        .cloned()
        .collect()
}

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
