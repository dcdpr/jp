//! Pure domain model and state-machine rules for release branches, notes
//! branches, and release tags.
//!
//! - A `v<version>` tag finalizes a release (e.g. `v0.3.0`).
//! - A release branch (`release/<major>.<minor>`) holds all `<major>.<minor>.x`
//!   releases.
//!   Each release on the branch starts with a commit carrying a
//!   `Starts-Release: <version>` trailer, followed by any commits cherry-picked
//!   from `main` (`(cherry picked from commit <hash>)`).
//! - A release is in preparation from its `Starts-Release` commit until its
//!   `v<version>` tag is created.
//! - Release notes are prepared on `release-notes/v<version>` and merged into
//!   `main`.
//!
//! All git subprocess calls live in [`crate::repository`]; this module parses
//! git output and enforces version and branch transitions.

use std::collections::{BTreeMap, BTreeSet};

use semver::Version;

/// Commit trailer marking the start of a release on a release branch.
pub(crate) const STARTS_RELEASE: &str = "Starts-Release";

/// Prefix written by `git cherry-pick -x` before `<hash>)`.
const PICKED_FROM: &str = "(cherry picked from commit ";

pub(crate) const RELEASE_BRANCH: &str = "release/";
pub(crate) const NOTES_BRANCH: &str = "release-notes/";

/// Release state parsed from local and `origin` tags and release branches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Releases {
    /// Non-prerelease versions with a `v<version>` tag locally or on `origin`.
    pub tagged: BTreeSet<Version>,

    /// Non-prerelease versions with a `v<version>` tag on `origin`.
    pub published: BTreeSet<Version>,

    /// Release branches keyed by `(major, minor)`.
    pub lines: BTreeMap<(u64, u64), Line>,
}

/// State of a single `release/<major>.<minor>` branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    pub branch: String,
    pub tip: String,

    /// Releases started on this branch (`Starts-Release` commits), ordered from
    /// oldest to newest.
    pub started: Vec<Started>,

    /// Hashes of `main` commits cherry-picked onto this branch.
    pub picked: BTreeSet<String>,
}

/// A `Starts-Release` commit on a release branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Started {
    pub version: Version,
    pub hash: String,

    /// Commit timestamp in seconds since the Unix epoch.
    pub at: i64,
}

/// Target release branch for `jp release pick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Target<'a> {
    pub line: &'a Line,

    /// Whether `pick` must create a new `Starts-Release` commit before
    /// cherry-picking (starting a new patch release after the previous release
    /// on `line` was tagged).
    pub start: bool,
}

/// Relationship between a local branch and its remote-tracking branch on
/// `origin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    /// Exists only in the local repository.
    Local,

    /// Exists only on `origin`.
    Remote,

    /// Local and `origin` point to the same commit.
    Published,

    /// Local is a strict descendant of `origin`.
    Ahead,

    /// `origin` is a strict descendant of local.
    Behind,

    /// Local and `origin` have diverged.
    Diverged,
}

impl Standing {
    pub(crate) fn unpushed(self) -> bool {
        matches!(self, Self::Local | Self::Ahead)
    }
}

pub(crate) fn branch(version: &Version) -> String {
    format!("{RELEASE_BRANCH}{}.{}", version.major, version.minor)
}

pub(crate) fn notes_branch(version: &Version) -> String {
    format!("{NOTES_BRANCH}v{version}")
}

pub(crate) fn abandon_branch(version: &Version) -> String {
    format!("{NOTES_BRANCH}abandon-v{version}")
}

pub(crate) fn tag(version: &Version) -> String {
    format!("v{version}")
}

/// Parse `(major, minor)` from a `release/<major>.<minor>` branch name.
pub(crate) fn line_of(name: &str) -> Option<(u64, u64)> {
    let (major, minor) = name.strip_prefix(RELEASE_BRANCH)?.split_once('.')?;

    Some((number(major)?, number(minor)?))
}

fn number(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if !digits || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }

    text.parse().ok()
}

/// Parse `git ls-remote --tags` / `git for-each-ref` lines
/// (`<hash>\trefs/tags/<name>`) into a map from tag name to commit hash.
///
/// For annotated tags, the peeled `refs/tags/<name>^{}` entry overrides the tag
/// object hash.
pub(crate) fn parse_tags(listing: &str) -> BTreeMap<String, String> {
    let mut tags = BTreeMap::new();
    let mut peeled = BTreeMap::new();
    for line in listing.lines() {
        let Some((hash, reference)) = line.split_once('\t') else {
            continue;
        };
        let Some(name) = reference.strip_prefix("refs/tags/") else {
            continue;
        };

        match name.strip_suffix("^{}") {
            Some(name) => peeled.insert(name.to_owned(), hash.to_owned()),
            None => tags.insert(name.to_owned(), hash.to_owned()),
        };
    }

    tags.extend(peeled);
    tags
}

/// Extract non-prerelease semantic versions from `v<version>` tag names.
pub(crate) fn versions<'a>(names: impl IntoIterator<Item = &'a String>) -> BTreeSet<Version> {
    names
        .into_iter()
        .filter_map(|name| {
            let version = Version::parse(name.strip_prefix('v')?).ok()?;

            (version.pre.is_empty() && version.build.is_empty()).then_some(version)
        })
        .collect()
}

/// Parse `Starts-Release` commits (oldest first) and cherry-picked commit
/// hashes from `git log -z --format=%H%n%ct%n%B` output.
pub(crate) fn parse_line(log: &str) -> Result<(Vec<Started>, BTreeSet<String>), String> {
    const FIELDS: usize = 3;

    let mut started = vec![];
    let mut picked = BTreeSet::new();

    for record in log.split('\0') {
        let record = record.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(FIELDS, '\n');
        let commit = fields.next().unwrap_or_default();
        let at = fields
            .next()
            .and_then(|at| at.trim().parse::<i64>().ok())
            .ok_or_else(|| format!("The log of a release branch gives commit {commit} no date."))?;
        let message = fields.next().unwrap_or_default();

        let trailers = message.trim_end().rsplit("\n\n").next().unwrap_or_default();
        for line in trailers.lines().map(str::trim) {
            let version = line
                .strip_prefix(STARTS_RELEASE)
                .and_then(|rest| rest.strip_prefix(':'))
                .and_then(|version| Version::parse(version.trim()).ok());
            if let Some(version) = version {
                started.push(Started {
                    version,
                    hash: commit.to_owned(),
                    at,
                });
            }

            let hash = line
                .strip_prefix(PICKED_FROM)
                .and_then(|rest| rest.strip_suffix(')'));
            if let Some(hash) = hash {
                picked.insert(hash.to_owned());
            }
        }
    }
    started.reverse();

    Ok((started, picked))
}

pub(crate) fn start_message(version: &Version) -> String {
    format!("chore(release): Start release {version}\n\n{STARTS_RELEASE}: {version}\n")
}

/// Extract the Unix timestamp from the `tagger` header line of `git cat-file
/// tag` output.
pub(crate) fn tagger_time(text: &str) -> Option<i64> {
    let tagger = text
        .lines()
        .take_while(|line| !line.is_empty())
        .find_map(|line| line.strip_prefix("tagger "))?;

    // Format: `tagger Name <email> <unix_seconds> <tz_offset>`
    let mut fields = tagger.rsplit(' ');
    fields.next()?;

    fields.next()?.parse().ok()
}

/// Return whether `version` is allowed to contain breaking changes (a major
/// release, or a `0.x.0` minor release).
pub(crate) fn can_break(version: &Version) -> bool {
    version.patch == 0 && (version.major == 0 || version.minor == 0)
}

impl Releases {
    /// Highest version across all tagged and started releases.
    pub(crate) fn latest(&self) -> Option<&Version> {
        self.tagged.iter().chain(self.started()).max()
    }

    /// Highest minor or major version (`patch == 0`) across all tagged and
    /// started releases.
    pub(crate) fn newest_cut(&self) -> Option<&Version> {
        self.tagged
            .iter()
            .chain(self.started())
            .filter(|version| version.patch == 0)
            .max()
    }

    /// Return the Unix timestamp of the `Starts-Release` commit for `version`,
    /// or `None` if `version` was tagged without a `Starts-Release` commit.
    pub(crate) fn started_at(&self, version: &Version) -> Option<i64> {
        self.line(version)?
            .started
            .iter()
            .find(|started| started.version == *version)
            .map(|started| started.at)
    }

    /// Return the untagged version currently in preparation on `line`, if any.
    pub(crate) fn in_preparation<'a>(&self, line: &'a Line) -> Option<&'a Version> {
        line.started
            .last()
            .map(|started| &started.version)
            .filter(|version| !self.tagged.contains(*version))
    }

    pub(crate) fn line(&self, version: &Version) -> Option<&Line> {
        self.lines.get(&(version.major, version.minor))
    }

    /// Return all release branches that contain `hash` either in their branch
    /// ancestry (`containing`) or via cherry-pick.
    pub(crate) fn shipping(&self, hash: &str, containing: &[String]) -> Vec<&Line> {
        self.lines
            .values()
            .filter(|line| line.picked.contains(hash) || containing.contains(&line.branch))
            .collect()
    }

    /// Resolve and validate the version for `jp release cut`.
    pub(crate) fn next_cut(
        &self,
        breaking: bool,
        requested: Option<&Version>,
    ) -> Result<Version, String> {
        let latest = self.latest();
        let version = match (requested, latest) {
            (Some(requested), _) => requested.clone(),
            (None, None) => Version::new(0, 1, 0),
            (None, Some(latest)) if latest.major >= 1 && breaking => {
                Version::new(latest.major + 1, 0, 0)
            }
            (None, Some(latest)) => Version::new(latest.major, latest.minor + 1, 0),
        };

        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err(format!(
                "{version} is a pre-release, and `jp release` makes none."
            ));
        }
        if version.patch != 0 {
            return Err(format!(
                "{version} is a patch release, which `jp release pick {version} <commit>...` \
                 starts on its release branch; a cut starts a minor or major release."
            ));
        }
        if let Some(latest) = latest
            && version <= *latest
        {
            return Err(format!(
                "{version} isn't past {latest}, the newest release tagged or started."
            ));
        }
        if let Some(line) = self.line(&version) {
            return Err(format!("{} exists already.", line.branch));
        }

        Ok(version)
    }

    /// Resolve and validate the target release branch for `jp release pick
    /// <version>`.
    pub(crate) fn pick_target(&self, version: &Version) -> Result<Target<'_>, String> {
        let Some(line) = self.line(version) else {
            return Err(if version.patch == 0 {
                format!(
                    "{version} hasn't been cut; cut it with `jp release cut --version {version}`."
                )
            } else {
                format!(
                    "There is no {}, so {version} has no release branch to pick onto.",
                    branch(version)
                )
            });
        };

        if let Some(prepared) = self.in_preparation(line) {
            if prepared == version {
                return Ok(Target { line, start: false });
            }
            return Err(format!(
                "{prepared} is in preparation on {}; pick into it, or finalize it before starting \
                 another release there.",
                line.branch
            ));
        }

        let Some(next) = self.next_patch(line) else {
            return Err(format!(
                "{} has no release started on it, so it holds no release to pick into.",
                line.branch
            ));
        };
        if next != *version {
            return Err(format!(
                "The next release on {} is {next}, so pick into that.",
                line.branch
            ));
        }

        Ok(Target { line, start: true })
    }

    /// Resolve and validate the release branch to tag for `jp release finalize
    /// <version>`.
    pub(crate) fn finalize_target(&self, version: &Version) -> Result<&Line, String> {
        if self.published.contains(version) {
            return Err(format!(
                "{version} is finalized already: origin has its tag v{version}."
            ));
        }
        if self.tagged.contains(version) {
            return Err(format!(
                "{version} is tagged in this repository already; publish the tag with `git push \
                 origin v{version}`."
            ));
        }
        let Some(line) = self.line(version) else {
            return Err(format!(
                "There is no {}, so {version} hasn't been cut.",
                branch(version)
            ));
        };

        match self.in_preparation(line) {
            Some(prepared) if prepared == version => Ok(line),
            Some(prepared) => Err(format!(
                "{prepared}, not {version}, is in preparation on {}.",
                line.branch
            )),
            None => Err(format!(
                "{version} isn't in preparation on {}; pick a commit into it with `jp release \
                 pick {version} <commit>` to start it.",
                line.branch
            )),
        }
    }

    fn next_patch(&self, line: &Line) -> Option<Version> {
        let last = line
            .started
            .iter()
            .map(|started| &started.version)
            .filter(|version| self.tagged.contains(*version))
            .max()?;

        Some(Version::new(last.major, last.minor, last.patch + 1))
    }

    fn started(&self) -> impl Iterator<Item = &Version> {
        self.lines
            .values()
            .flat_map(|line| line.started.iter().map(|started| &started.version))
    }
}

#[cfg(test)]
#[path = "releases_tests.rs"]
mod tests;
