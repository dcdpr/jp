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

    /// Return the path `file` had at the base revision, or `file` unchanged if
    /// it was not renamed.
    #[must_use]
    pub fn old_path<'a>(&'a self, file: &'a str) -> &'a str {
        self.0
            .iter()
            .find(|(_, new)| new.as_str() == file)
            .map_or(file, |(old, _)| old.as_str())
    }
}

/// An item that moved from one file to another between the base revision and
/// the target tree.
///
/// Symbols are file-relative, so unrelated items in different files share one
/// (`create_request`, `Ls::run`).
/// Matching findings by symbol alone would let a finding fixed in one of them
/// pay for a finding added to another, so [`verdict`] carries a baseline across
/// files only for the relocations it is given.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Relocation {
    /// File-relative path of the item, as in [`Finding::symbol`].
    pub symbol: String,

    /// The file the item left, as a path in the target tree.
    pub from: String,

    /// The file the item is in now.
    pub to: String,
}

impl Relocation {
    #[must_use]
    pub fn new(symbol: &str, from: &str, to: &str) -> Self {
        Self {
            symbol: symbol.to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
        }
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
    /// after subtracting findings that arrived with a relocated item.
    pub new: usize,

    /// Total findings with this key in the target tree.
    pub of: usize,
}

/// A finding introduced since the base revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewFinding {
    #[serde(flatten)]
    pub finding: Finding,

    /// Present when the base revision already had findings for this
    /// [`Finding::key`], either in the same file or in the file a relocated
    /// item came from.
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

/// A base finding's key in the target tree, or a head finding's key: `(rule,
/// file, symbol)`.
type Key<'a> = (&'a str, &'a str, &'a str);

/// The compared findings of both reports, grouped by key.
struct Tally<'a> {
    /// Base finding count per key, with renamed files under their new path.
    was: BTreeMap<Key<'a>, usize>,

    /// Head findings per key.
    groups: BTreeMap<Key<'a>, Vec<&'a Finding>>,
}

impl<'a> Tally<'a> {
    fn new(base: &'a Report, head: &'a Report, renames: &'a Renames, policy: &Policy) -> Self {
        let compared =
            |finding: &&Finding| policy.gates(&finding.rule) && !policy.is_enforced(&finding.rule);

        let mut was = BTreeMap::<_, usize>::new();
        for finding in base.findings.iter().filter(compared) {
            let file = renames.new_path(&finding.file);
            *was.entry((finding.rule.as_str(), file, finding.symbol.as_str()))
                .or_default() += 1;
        }

        let mut groups = BTreeMap::<_, Vec<_>>::new();
        for finding in head.findings.iter().filter(compared) {
            groups.entry(finding.key()).or_default().push(finding);
        }

        Self { was, groups }
    }

    fn had(&self, key: &Key<'a>) -> usize {
        self.was.get(key).copied().unwrap_or(0)
    }

    /// For each `(rule, symbol)`, the files that have fewer of its findings
    /// than at the base, and by how many.
    ///
    /// File-level findings (empty symbol) are left out: they belong to their
    /// file and can't follow an item.
    fn departed(&self) -> BTreeMap<(&'a str, &'a str), BTreeMap<&'a str, usize>> {
        let mut departed = BTreeMap::<_, BTreeMap<_, _>>::new();
        for (key, &had) in &self.was {
            let (rule, file, symbol) = *key;
            let has = self.groups.get(key).map_or(0, Vec::len);
            if !symbol.is_empty() && had > has {
                departed
                    .entry((rule, symbol))
                    .or_default()
                    .insert(file, had - has);
            }
        }
        departed
    }
}

/// List the relocations that would explain a finding [`verdict`] otherwise
/// reports as new.
///
/// A candidate pairs a file in which a `(rule, symbol)` has fewer findings than
/// at the base with a file in which it has more.
/// The finding counts alone don't show that the item moved: the caller checks
/// each candidate against the source and passes the confirmed ones to
/// [`verdict`].
#[must_use]
pub fn relocation_candidates(
    base: &Report,
    head: &Report,
    renames: &Renames,
    policy: &Policy,
) -> BTreeSet<Relocation> {
    let tally = Tally::new(base, head, renames, policy);
    let departed = tally.departed();

    let mut candidates = BTreeSet::new();
    for (key, group) in &tally.groups {
        let (rule, file, symbol) = *key;
        if group.len() <= tally.had(key) {
            continue;
        }
        let Some(sources) = departed.get(&(rule, symbol)) else {
            continue;
        };
        candidates.extend(
            sources
                .keys()
                .map(|from| Relocation::new(symbol, from, file)),
        );
    }
    candidates
}

/// Compare `head` against `base` under `policy`.
///
/// Exempt rules are excluded, and base findings in renamed files are mapped to
/// their paths in `head` before comparison.
///
/// The findings of an item in `relocations` keep their baseline: as many of a
/// rule's findings as left [`Relocation::from`] are not new in
/// [`Relocation::to`].
/// Without a relocation, a finding is compared within its own file only, even
/// when another file has an item with the same symbol.
#[must_use]
pub fn verdict(
    base: &Report,
    head: &Report,
    renames: &Renames,
    relocations: &BTreeSet<Relocation>,
    policy: &Policy,
) -> Verdict {
    let tally = Tally::new(base, head, renames, policy);
    let mut departed = tally.departed();

    let mut new = Vec::new();
    for (key, group) in &tally.groups {
        let (rule, file, symbol) = *key;
        let mut had = tally.had(key);

        for (from, left) in departed.get_mut(&(rule, symbol)).into_iter().flatten() {
            if group.len() <= had {
                break;
            }
            if !relocations.contains(&Relocation::new(symbol, from, file)) {
                continue;
            }
            let moved = (*left).min(group.len() - had);
            *left -= moved;
            had += moved;
        }
        if group.len() <= had {
            continue;
        }

        let shared = (had > 0).then_some(Shared {
            new: group.len() - had,
            of: group.len(),
        });
        new.extend(group.iter().map(|&finding| NewFinding {
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
