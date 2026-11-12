//! Execution shell: running `rustqual` and `git`, resolving symbols from source
//! files, loading guidance, and caching base reports.

use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fs, io,
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_process::{ExitCode, ProcessRunner};
use sha2::{Digest as _, Sha256};

use crate::{
    Finding, Guidance, Policy, Report, Verdict,
    diff::{self, Relocation, Renames, RenamesError},
    policy::{self, PolicyError},
    sarif::{self, RawFinding, SarifError},
    symbol::Symbols,
};

/// The rustqual config directory, relative to the workspace root.
pub const CONFIG_DIR: &str = ".config/rustqual";

/// The guidance directory, relative to the config directory.
pub const GUIDANCE_DIR: &str = "guidance";

/// Where base reports are cached, relative to the workspace root.
pub const CACHE_DIR: &str = "target/qual";

/// Cache schema version.
/// Increment when [`Report`] serialization or symbol resolution changes.
const CACHE_VERSION: &str = "1";

/// File-level rules that should not be resolved to an enclosing item even when
/// reported at line 1.
const FILE_RULES: &[&str] = &["SRP-002"];

/// Hex lengths of full SHA-1 and SHA-256 git object IDs.
const OID_DIGIT_COUNTS: [usize; 2] = [40, 64];

/// Errors returned while running `rustqual`, `git`, or reading workspace files.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not run `{command}`: {source}")]
    Spawn {
        command: String,
        #[source]
        source: io::Error,
    },

    #[error("`{command}` failed with exit code {status}:\n{stderr}")]
    Command {
        command: String,
        status: ExitCode,
        stderr: String,
    },

    #[error("rustqual's {pass} pass wrote a report that can't be read: {source}")]
    Sarif {
        pass: String,
        #[source]
        source: SarifError,
    },

    /// `rustqual` skips files it cannot read or parse and emits no findings for
    /// them, so skipped files must fail the run.
    #[error("rustqual's {pass} pass skipped files:\n{warnings}")]
    Skipped { pass: String, warnings: String },

    #[error("{path}: {source}")]
    Policy {
        path: Utf8PathBuf,
        #[source]
        source: PolicyError,
    },

    #[error("{path} exempts rule `{rule}`, so a report has no findings for it")]
    Exempt { rule: String, path: Utf8PathBuf },

    #[error(transparent)]
    Renames(#[from] RenamesError),

    #[error("could not read {path}: {source}")]
    Read {
        path: Utf8PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not parse {path} to locate a rustqual finding in it: {source}")]
    Parse {
        path: Utf8PathBuf,
        #[source]
        source: syn::Error,
    },

    #[error("could not create a directory for the base worktree: {0}")]
    TempDir(#[source] io::Error),
}

/// A git working tree with a `rustqual` policy and base-report cache.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: Utf8PathBuf,
    config_dir: Utf8PathBuf,
    cache_dir: Utf8PathBuf,
}

impl Workspace {
    /// Create a [`Workspace`] rooted at `root`, using [`CONFIG_DIR`] and
    /// [`CACHE_DIR`].
    pub fn new(root: impl Into<Utf8PathBuf>) -> Self {
        let root = root.into();
        Self {
            config_dir: root.join(CONFIG_DIR),
            cache_dir: root.join(CACHE_DIR),
            root,
        }
    }

    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    /// Load and validate the workspace [`Policy`] from
    /// `.config/rustqual/qual.toml`.
    ///
    /// # Errors
    ///
    /// Returns an error if the policy file cannot be read or is invalid.
    pub fn policy(&self) -> Result<Policy, Error> {
        let path = self.config_dir.join(policy::FILE);
        let text = read(&path)?;
        Policy::parse(&text).map_err(|source| Error::Policy { path, source })
    }

    /// Collect all gating findings in the working tree.
    ///
    /// # Errors
    ///
    /// Returns an error if the policy cannot be read, a `rustqual` pass fails
    /// or skips a file, or a source file with a finding fails to parse.
    pub fn report(&self, runner: &dyn ProcessRunner) -> Result<Report, Error> {
        let policy = self.policy()?;
        let mut report = self.analyze(runner, &self.root, &policy)?;
        report
            .findings
            .retain(|finding| policy.gates(&finding.rule));
        Ok(report)
    }

    /// Collect the findings of `rule` in the working tree.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exempt`] if the policy exempts `rule`, without running
    /// `rustqual`.
    /// Otherwise fails as [`Self::report`] does.
    pub fn report_rule(&self, runner: &dyn ProcessRunner, rule: &str) -> Result<Report, Error> {
        if !self.policy()?.gates(rule) {
            return Err(Error::Exempt {
                rule: rule.to_owned(),
                path: self.config_dir.join(policy::FILE),
            });
        }

        let mut report = self.report(runner)?;
        report.findings.retain(|finding| finding.rule == rule);
        Ok(report)
    }

    /// Compare the working tree against its merge base with `base`.
    ///
    /// Resolving `git merge-base HEAD <base>` (like `git diff base...HEAD`)
    /// ignores commits added to `base` after the branch forked.
    /// When `base` is an ancestor of `HEAD`, the merge base is `base` itself.
    ///
    /// The merge-base commit is analyzed in a temporary git worktree using the
    /// working tree's `rustqual` configuration, so configuration changes apply
    /// to both revisions.
    /// Base reports are cached under [`CACHE_DIR`], keyed by the merge-base
    /// commit SHA, the `rustqual` version, and the policy and pass config
    /// contents.
    ///
    /// When `base` is git's null object ID (40 or 64 `'0'` digits), only the
    /// working tree is analyzed (see [`diff::enforced_only`]):
    /// [`Verdict::base`] is `None`, [`Verdict::new`] is empty, and only
    /// findings for enforced rules fail the verdict.
    ///
    /// # Errors
    ///
    /// Returns an error if `base` cannot be resolved to a merge base with
    /// `HEAD`, if `git` or `rustqual` fails, or if analysis of either tree
    /// fails (see [`Self::report`]).
    pub fn diff(&self, runner: &dyn ProcessRunner, base: &str) -> Result<Verdict, Error> {
        let policy = self.policy()?;
        if is_null_oid(base) {
            let head_report = self.analyze(runner, &self.root, &policy)?;
            return Ok(diff::enforced_only(&head_report, &policy));
        }

        let sha = run(runner, "git", &["merge-base", "HEAD", base], &self.root)?;
        let sha = sha.trim();

        let base_report = self.analyze_base(runner, sha, &policy)?;
        let head_report = self.analyze(runner, &self.root, &policy)?;
        let names = run(
            runner,
            "git",
            &["diff", "-M", "--name-status", "-z", sha, "--"],
            &self.root,
        )?;
        let renames = Renames::from_name_status(&names)?;

        let candidates = diff::relocation_candidates(&base_report, &head_report, &renames, &policy);
        let relocations = self.relocations(runner, sha, candidates, &renames)?;

        Ok(diff::verdict(
            &base_report,
            &head_report,
            &renames,
            &relocations,
            &policy,
        ))
    }

    /// Keep the `candidates` that the source of both trees confirms.
    ///
    /// An item moved when the working tree's [`Relocation::from`] no longer
    /// defines it and [`Relocation::to`] did not define it at commit `sha`.
    /// An item that is still in its file was fixed there, and an item that the
    /// destination already had is a different item with the same symbol.
    fn relocations(
        &self,
        runner: &dyn ProcessRunner,
        sha: &str,
        candidates: BTreeSet<Relocation>,
        renames: &Renames,
    ) -> Result<BTreeSet<Relocation>, Error> {
        let mut head = BTreeMap::<String, Option<Symbols>>::new();
        let mut base = BTreeMap::<String, Option<Symbols>>::new();
        let mut relocations = BTreeSet::new();

        for candidate in candidates {
            let source = match head.entry(candidate.from.clone()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let symbols = head_symbols(&self.root.join(entry.key()))?;
                    entry.insert(symbols)
                }
            };
            if defines(source.as_ref(), &candidate.symbol) {
                continue;
            }

            let destination = match base.entry(candidate.to.clone()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let path = renames.old_path(entry.key());
                    let symbols = self.base_symbols(runner, sha, path)?;
                    entry.insert(symbols)
                }
            };
            if defines(destination.as_ref(), &candidate.symbol) {
                continue;
            }

            relocations.insert(candidate);
        }

        Ok(relocations)
    }

    /// Index the items of `path` as it is at commit `sha`, or return `None`
    /// when the commit has no such file.
    fn base_symbols(
        &self,
        runner: &dyn ProcessRunner,
        sha: &str,
        path: &str,
    ) -> Result<Option<Symbols>, Error> {
        // `git show` fails on a missing path the same way it fails on anything
        // else, and `ls-tree` prints nothing for one.
        let list = ["ls-tree", "-z", "--name-only", sha, "--", path];
        if run(runner, "git", &list, &self.root)?.is_empty() {
            return Ok(None);
        }

        let object = format!("{sha}:{path}");
        let source = run(runner, "git", &["show", &object], &self.root)?;
        Symbols::parse(&source)
            .map(Some)
            .map_err(|source| Error::Parse {
                path: object.into(),
                source,
            })
    }

    /// Load remediation guidance for each rule in `rules` that has a file in
    /// [`GUIDANCE_DIR`].
    ///
    /// # Errors
    ///
    /// Returns an error if a guidance file exists but cannot be read.
    pub fn guidance<'a>(
        &self,
        rules: impl IntoIterator<Item = &'a str>,
    ) -> Result<Guidance, Error> {
        let dir = self.config_dir.join(GUIDANCE_DIR);
        let mut found = Vec::new();
        for rule in rules.into_iter().collect::<BTreeSet<_>>() {
            let path = dir.join(Guidance::file_name(rule));
            match fs::read_to_string(&path) {
                Ok(text) => found.push((rule.to_owned(), text)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(source) => return Err(Error::Read { path, source }),
            }
        }
        Ok(found.into_iter().collect())
    }

    /// Run every configured pass over `tree`, retaining each rule's findings
    /// from its owning pass.
    fn analyze(
        &self,
        runner: &dyn ProcessRunner,
        tree: &Utf8Path,
        policy: &Policy,
    ) -> Result<Report, Error> {
        let mut raw = Vec::new();
        let mut titles = BTreeMap::new();

        for pass in &policy.passes {
            let config = self.config_dir.join(&pass.config);
            let args = ["-c", config.as_str(), "--format", "sarif", "--no-fail", "."];
            let output = run_output(runner, "rustqual", &args, tree)?;

            let warnings = skipped_files(&output.stderr);
            if !warnings.is_empty() {
                return Err(Error::Skipped {
                    pass: pass.name.clone(),
                    warnings,
                });
            }

            let sarif = sarif::parse(&output.stdout).map_err(|source| Error::Sarif {
                pass: pass.name.clone(),
                source,
            })?;

            titles.extend(sarif.titles);
            raw.extend(
                sarif
                    .findings
                    .into_iter()
                    .filter(|finding| {
                        policy
                            .owner(&finding.rule)
                            .is_some_and(|owner| owner.name == pass.name)
                    })
                    .map(|finding| (pass.name.clone(), finding)),
            );
        }

        let mut findings = locate(tree, raw)?;
        findings.sort();
        Ok(Report { findings, titles })
    }

    /// Analyze commit `sha` in a temporary worktree, or return its cached
    /// report.
    fn analyze_base(
        &self,
        runner: &dyn ProcessRunner,
        sha: &str,
        policy: &Policy,
    ) -> Result<Report, Error> {
        let key = self.cache_key(runner, sha, policy)?;
        let cache = self.cache_dir.join(format!("{key}.json"));
        if let Some(report) = read_cache(&cache) {
            return Ok(report);
        }

        let dir = camino_tempfile::tempdir().map_err(Error::TempDir)?;
        let tree = dir.path().join("base");
        let add = ["worktree", "add", "--detach", tree.as_str(), sha];
        run(runner, "git", &add, &self.root)?;

        // The worktree is removed even when the analysis fails, so a failed
        // run leaves no worktree registered in the repository.
        let analysed = self.analyze(runner, &tree, policy);
        let remove = ["worktree", "remove", "--force", tree.as_str()];
        let removed = run(runner, "git", &remove, &self.root);
        let report = analysed?;
        removed?;

        write_cache(&cache, &report);
        Ok(report)
    }

    fn cache_key(
        &self,
        runner: &dyn ProcessRunner,
        sha: &str,
        policy: &Policy,
    ) -> Result<String, Error> {
        let version = run(runner, "rustqual", &["--version"], &self.root)?;

        let mut hasher = Sha256::new();
        for part in [CACHE_VERSION, sha, version.trim()] {
            hasher.update(part);
            hasher.update([0]);
        }

        let files = [policy::FILE]
            .into_iter()
            .chain(policy.passes.iter().map(|pass| pass.config.as_str()));
        for name in files {
            hasher.update(name);
            hasher.update([0]);
            hasher.update(read(&self.config_dir.join(name))?);
            hasher.update([0]);
        }

        Ok(format!("{:x}", hasher.finalize()))
    }
}

/// Resolve each finding to its enclosing symbol, parsing each source file under
/// `tree` at most once.
fn locate(tree: &Utf8Path, raw: Vec<(String, RawFinding)>) -> Result<Vec<Finding>, Error> {
    let mut files = BTreeMap::<String, Symbols>::new();
    let mut findings = Vec::with_capacity(raw.len());

    for (pass, raw) in raw {
        let symbol = if raw.file.is_empty() || FILE_RULES.contains(&raw.rule.as_str()) {
            String::new()
        } else {
            let symbols = match files.entry(raw.file.clone()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let path = tree.join(entry.key());
                    let source = read(&path)?;
                    let symbols =
                        Symbols::parse(&source).map_err(|source| Error::Parse { path, source })?;
                    entry.insert(symbols)
                }
            };
            symbols.at(raw.line)
        };

        findings.push(Finding {
            file: raw.file,
            line: raw.line,
            rule: raw.rule,
            symbol,
            message: raw.message,
            pass,
        });
    }

    Ok(findings)
}

/// Index the items of the working-tree file at `path`, or return `None` when
/// the file is gone.
fn head_symbols(path: &Utf8Path) -> Result<Option<Symbols>, Error> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Read {
                path: path.to_owned(),
                source,
            });
        }
    };

    Symbols::parse(&source)
        .map(Some)
        .map_err(|source| Error::Parse {
            path: path.to_owned(),
            source,
        })
}

/// Whether a file, when it exists, has an item at `symbol`.
fn defines(symbols: Option<&Symbols>, symbol: &str) -> bool {
    symbols.is_some_and(|symbols| symbols.contains(symbol))
}

/// Return `true` if `rev` is a full-length SHA-1 or SHA-256 null object ID.
///
/// Abbreviated all-zero prefixes (such as `0000000`) are rejected because they
/// can match real commits.
fn is_null_oid(rev: &str) -> bool {
    OID_DIGIT_COUNTS.contains(&rev.len()) && rev.bytes().all(|byte| byte == b'0')
}

/// Extract stderr lines where `rustqual` warns that it skipped a file.
fn skipped_files(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| {
            line.starts_with("Warning: Could not parse ")
                || line.starts_with("Warning: Could not read ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run(
    runner: &dyn ProcessRunner,
    program: &str,
    args: &[&str],
    dir: &Utf8Path,
) -> Result<String, Error> {
    run_output(runner, program, args, dir).map(|output| output.stdout)
}

fn run_output(
    runner: &dyn ProcessRunner,
    program: &str,
    args: &[&str],
    dir: &Utf8Path,
) -> Result<jp_process::ProcessOutput, Error> {
    let command = || format!("{program} {}", args.join(" "));
    let output = runner
        .run(program, args, dir)
        .map_err(|source| Error::Spawn {
            command: command(),
            source,
        })?;

    if !output.success() {
        return Err(Error::Command {
            command: command(),
            status: output.status,
            stderr: output.stderr,
        });
    }
    Ok(output)
}

fn read(path: &Utf8Path) -> Result<String, Error> {
    fs::read_to_string(path).map_err(|source| Error::Read {
        path: path.to_owned(),
        source,
    })
}

fn read_cache(path: &Utf8Path) -> Option<Report> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write `report` to the cache at `path`, ignoring I/O errors so a read-only or
/// missing cache directory only costs a re-analysis on the next run.
fn write_cache(path: &Utf8Path, report: &Report) {
    let Ok(text) = serde_json::to_string(report) else {
        return;
    };
    if let Some(dir) = path.parent() {
        drop(fs::create_dir_all(dir));
    }
    drop(fs::write(path, text));
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
