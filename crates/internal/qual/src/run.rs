//! The shell around the comparison: running rustqual and git, reading source
//! and guidance files, and caching base reports.

use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fs, io,
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_process::{ExitCode, ProcessRunner};
use sha2::{Digest as _, Sha256};

use crate::{
    Finding, Guidance, Policy, Report, Verdict,
    diff::{self, Renames, RenamesError},
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

/// Bumped whenever a cached report would mean something else: a change to the
/// shape of [`Report`], or to how symbols are named.
const CACHE_VERSION: &str = "1";

/// Rules whose findings are about a whole file, so their line names no item.
const FILE_RULES: &[&str] = &["SRP-002"];

/// Why a comparison or a report couldn't be produced.
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

    /// rustqual skips a file it can't read or parse, and reports nothing in it,
    /// so a new finding there would go unseen.
    #[error("rustqual's {pass} pass skipped files:\n{warnings}")]
    Skipped { pass: String, warnings: String },

    #[error("{path}: {source}")]
    Policy {
        path: Utf8PathBuf,
        #[source]
        source: PolicyError,
    },

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

/// A git checkout with a rustqual policy, and where to cache base reports.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: Utf8PathBuf,
    config_dir: Utf8PathBuf,
    cache_dir: Utf8PathBuf,
}

impl Workspace {
    /// The checkout at `root`, with its policy in [`CONFIG_DIR`] and its cache
    /// in [`CACHE_DIR`].
    pub fn new(root: impl Into<Utf8PathBuf>) -> Self {
        let root = root.into();
        Self {
            config_dir: root.join(CONFIG_DIR),
            cache_dir: root.join(CACHE_DIR),
            root,
        }
    }

    /// The root of the checkout.
    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    /// Read the policy.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy file can't be read or isn't valid.
    pub fn policy(&self) -> Result<Policy, Error> {
        let path = self.config_dir.join(policy::FILE);
        let text = read(&path)?;
        Policy::parse(&text).map_err(|source| Error::Policy { path, source })
    }

    /// Every gating finding in the working tree.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy can't be read, a rustqual pass fails or
    /// skips a file, or a file with a finding doesn't parse.
    pub fn report(&self, runner: &dyn ProcessRunner) -> Result<Report, Error> {
        let policy = self.policy()?;
        let mut report = self.analyze(runner, &self.root, &policy)?;
        report
            .findings
            .retain(|finding| policy.gates(&finding.rule));
        Ok(report)
    }

    /// Compare the working tree against the commit it forked from `base` at.
    ///
    /// That is the merge base of `HEAD` and `base`, so commits `base` gained
    /// after the fork aren't counted against the working tree, the way `git
    /// diff base...HEAD` leaves them out.
    /// When `base` is an ancestor of `HEAD`, the merge base is `base` itself.
    ///
    /// The base is analysed in a temporary worktree, with the working tree's
    /// rustqual configs, so a config change shows up on both sides rather than
    /// as new findings.
    /// Its report is cached under [`CACHE_DIR`], keyed by the base commit, the
    /// rustqual version, and the contents of the policy and the configs.
    ///
    /// # Errors
    ///
    /// Returns an error when `base` isn't a commit or shares no history with
    /// `HEAD`, git or rustqual fails, or any step of [`report`] does, for
    /// either tree.
    ///
    /// [`report`]: Self::report
    pub fn diff(&self, runner: &dyn ProcessRunner, base: &str) -> Result<Verdict, Error> {
        let policy = self.policy()?;
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

        Ok(diff::verdict(&base_report, &head_report, &renames, &policy))
    }

    /// The guidance for each of `rules` that has a file in [`GUIDANCE_DIR`].
    ///
    /// # Errors
    ///
    /// Returns an error when a guidance file exists but can't be read.
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

    /// Run every pass over `tree`, keeping each rule's findings from the pass
    /// that owns it.
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

    /// Analyse the commit `sha` in a temporary worktree, or read its report
    /// from the cache.
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

/// Give each finding the symbol it sits in, parsing each file under `tree` at
/// most once.
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

/// The lines in which rustqual says it skipped a file.
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

/// A cached report, or `None` when there is none that can be read.
fn read_cache(path: &Utf8Path) -> Option<Report> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Cache `report`, if the cache can be written.
///
/// A cache that can't be written costs the next run a rustqual analysis, and
/// nothing else, so the failure is ignored.
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
