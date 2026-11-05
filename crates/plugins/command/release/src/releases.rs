//! What `origin`'s tags and release branches say about a project's releases,
//! and what `cut`, `pick`, and `finalize` may do next.
//!
//! A tag `v0.3.0` finalizes release 0.3.0.
//! A release branch, `release/0.3`, holds the code of every 0.3.x release:
//! `main` at the cut, and on top of it two kinds of commit.
//! The commit that starts a release carries a `Starts-Release: 0.3.1` trailer,
//! and a commit picked from `main` ends its message with `(cherry picked from
//! commit <hash>)`, as `git cherry-pick -x` writes it.
//! A release is in preparation from the commit that starts it until its tag.
//! A minor or major release is cut when the commit that starts it is made.
//!
//! Everything here reads what git printed and decides; the `repository` module
//! runs git.

use std::collections::{BTreeMap, BTreeSet};

use semver::Version;

/// The trailer the commit that starts a release on its branch carries, naming
/// the release's version.
pub(crate) const STARTS_RELEASE: &str = "Starts-Release";

/// What `git cherry-pick -x` writes into a picked commit's message, followed by
/// the picked commit's hash and `)`.
const PICKED_FROM: &str = "(cherry picked from commit ";

/// What every release branch's name starts with.
const RELEASE_BRANCH: &str = "release/";

/// What origin's tags and release branches say about the releases.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Releases {
    /// The versions a `v` tag finalized.
    ///
    /// Tags of pre-releases are left out.
    pub tagged: BTreeSet<Version>,

    /// Every release branch, by the major and minor version its releases share.
    pub lines: BTreeMap<(u64, u64), Line>,
}

/// A release branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    /// Its name, such as `release/0.3`.
    pub branch: String,

    /// The hash of its tip.
    pub tip: String,

    /// The releases started on it, oldest first.
    pub started: Vec<Started>,

    /// The hashes of the commits of `main` picked onto it.
    pub picked: BTreeSet<String>,
}

/// A release started on its release branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Started {
    pub version: Version,

    /// When the commit that starts it was made, in seconds since the Unix
    /// epoch.
    pub at: i64,
}

/// Where `pick` puts its commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Target<'a> {
    pub line: &'a Line,

    /// Whether the release has yet to start: a patch release, after the line's
    /// last release was tagged.
    pub start: bool,
}

/// The name of the release branch holding the releases `major.minor.x`.
pub(crate) fn branch(version: &Version) -> String {
    format!("{RELEASE_BRANCH}{}.{}", version.major, version.minor)
}

/// The major and minor version the release branch `name` holds, when it is
/// named like one, such as `release/0.3`.
pub(crate) fn line_of(name: &str) -> Option<(u64, u64)> {
    let (major, minor) = name.strip_prefix(RELEASE_BRANCH)?.split_once('.')?;

    Some((number(major)?, number(minor)?))
}

/// `text` as a number written the way semantic versions write them: digits,
/// with no leading zero.
fn number(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if !digits || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }

    text.parse().ok()
}

/// The versions the tags `git ls-remote --tags --refs` lists finalized: each
/// `v`, then a semantic version that is no pre-release.
pub(crate) fn parse_tags(listing: &str) -> BTreeSet<Version> {
    listing
        .lines()
        .filter_map(|line| {
            let (_, name) = line.split_once('\t')?;
            let version = Version::parse(name.strip_prefix("refs/tags/v")?).ok()?;

            (version.pre.is_empty() && version.build.is_empty()).then_some(version)
        })
        .collect()
}

/// The releases started on a release branch, oldest first, and the commits of
/// `main` picked onto it, read from `git log -z --format=%H%n%ct%n%B` of the
/// branch's commits that are not on `main`, newest first.
///
/// The error names a commit the log gives no date.
pub(crate) fn parse_line(log: &str) -> Result<(Vec<Started>, BTreeSet<String>), String> {
    let mut started = vec![];
    let mut picked = BTreeSet::new();

    for record in log.split('\0') {
        let record = record.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(3, '\n');
        let commit = fields.next().unwrap_or_default();
        let at = fields
            .next()
            .and_then(|at| at.trim().parse::<i64>().ok())
            .ok_or_else(|| format!("The log of a release branch gives commit {commit} no date."))?;
        let message = fields.next().unwrap_or_default();

        // Both trailers sit in the last paragraph, where git writes trailers.
        let trailers = message.trim_end().rsplit("\n\n").next().unwrap_or_default();
        for line in trailers.lines().map(str::trim) {
            let version = line
                .strip_prefix(STARTS_RELEASE)
                .and_then(|rest| rest.strip_prefix(':'))
                .and_then(|version| Version::parse(version.trim()).ok());
            if let Some(version) = version {
                started.push(Started { version, at });
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

/// The message of the commit that starts release `version` on its branch.
pub(crate) fn start_message(version: &Version) -> String {
    format!("Start release {version}\n\n{STARTS_RELEASE}: {version}\n")
}

/// When the tag object `text`, as `git cat-file tag` prints it, was made, in
/// seconds since the Unix epoch: the time on its `tagger` line.
pub(crate) fn tagger_time(text: &str) -> Option<i64> {
    let tagger = text
        .lines()
        .take_while(|line| !line.is_empty())
        .find_map(|line| line.strip_prefix("tagger "))?;

    // `Name <email> 1792584000 +0200`: the time, then its time zone.
    let mut fields = tagger.rsplit(' ');
    fields.next()?;

    fields.next()?.parse().ok()
}

/// Whether release `version` may ship a change that requires users to act: a
/// major release, or a minor one before 1.0.
pub(crate) fn can_break(version: &Version) -> bool {
    version.patch == 0 && (version.major == 0 || version.minor == 0)
}

impl Releases {
    /// The newest version a tag finalized or a release branch started.
    pub(crate) fn latest(&self) -> Option<&Version> {
        self.tagged.iter().chain(self.started()).max()
    }

    /// The newest minor or major release a tag finalized or a release branch
    /// started: the last release cut.
    pub(crate) fn newest_cut(&self) -> Option<&Version> {
        self.tagged
            .iter()
            .chain(self.started())
            .filter(|version| version.patch == 0)
            .max()
    }

    /// When `version` was started on its release branch, in seconds since the
    /// Unix epoch; `None` when no commit on the branch starts it, such as for a
    /// release tagged by hand.
    pub(crate) fn started_at(&self, version: &Version) -> Option<i64> {
        self.line(version)?
            .started
            .iter()
            .find(|started| started.version == *version)
            .map(|started| started.at)
    }

    /// The release in preparation on `line`: the last one started on it, until
    /// its tag.
    pub(crate) fn in_preparation<'a>(&self, line: &'a Line) -> Option<&'a Version> {
        line.started
            .last()
            .map(|started| &started.version)
            .filter(|version| !self.tagged.contains(*version))
    }

    /// The release branch that holds `version`.
    pub(crate) fn line(&self, version: &Version) -> Option<&Line> {
        self.lines.get(&(version.major, version.minor))
    }

    /// The release branches that ship the change the commit `hash` made: those
    /// whose history holds it, as `containing` lists them by name, and those it
    /// was picked onto.
    pub(crate) fn shipping(&self, hash: &str, containing: &[String]) -> Vec<&Line> {
        self.lines
            .values()
            .filter(|line| line.picked.contains(hash) || containing.contains(&line.branch))
            .collect()
    }

    /// The version `cut` starts: `requested`, or the next minor release after
    /// the newest one tagged or started, or from 1.0 on the next major one when
    /// `breaking`.
    ///
    /// The first release is 0.1.0.
    /// The error says why the version can't be cut.
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

    /// Where `pick` puts commits for `version`: into the release in preparation
    /// on its branch, or into the next patch release of a branch whose last
    /// release is tagged, which `pick` then starts.
    ///
    /// The error says what `pick` accepts instead.
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

    /// The release branch whose tip `finalize` tags as `version`: the one
    /// holding `version` in preparation.
    ///
    /// The error says why `version` can't be finalized.
    pub(crate) fn finalize_target(&self, version: &Version) -> Result<&Line, String> {
        if self.tagged.contains(version) {
            return Err(format!(
                "{version} is finalized already: origin has its tag v{version}."
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

    /// The patch release after the last release tagged on `line`.
    fn next_patch(&self, line: &Line) -> Option<Version> {
        let last = line
            .started
            .iter()
            .map(|started| &started.version)
            .filter(|version| self.tagged.contains(*version))
            .max()?;

        Some(Version::new(last.major, last.minor, last.patch + 1))
    }

    /// Every version a release branch started.
    fn started(&self) -> impl Iterator<Item = &Version> {
        self.lines
            .values()
            .flat_map(|line| line.started.iter().map(|started| &started.version))
    }
}

#[cfg(test)]
#[path = "releases_tests.rs"]
mod tests;
