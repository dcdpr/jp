//! Assembling a [`Release`] from a fragments directory and git history.
//!
//! [`read`] loads either a specific release subdirectory (`<version>/`) or all
//! unreleased changes (the top level of the fragments directory plus any
//! `<version>/` subdirectory without a `v<version>` git tag), resolves credits
//! from git history and [`forge`], and sorts highlights and entries.
//!
//! A [`Release`] serializes to the versioned JSON schema ([`FORMAT`]) consumed
//! by site generators and external tools:
//!
//! ```json
//! {
//!   "format": 1,
//!   "version": "0.3.0",
//!   "date": "2026-10-28",
//!   "overview": "JP 0.3.0 lets the assistant call one tool on many inputs ...",
//!   "highlights": [
//!     {
//!       "slug": "fan-out",
//!       "title": "Tool calls fan out",
//!       "authors": ["@JeanMertz"],
//!       "body": "When the assistant needs the same tool on several inputs ...",
//!       "credits": [{ "kind": "change_request", "label": "#1182", "url": "https://github.com/dcdpr/jp/pull/1182" }]
//!     }
//!   ],
//!   "entries": [
//!     {
//!       "slug": "plugin-manifest-routing",
//!       "type": "changed",
//!       "body": "JP finds a command plugin by the manifest its binary embeds, ...",
//!       "credits": [{ "kind": "change_request", "label": "#1216", "url": "https://github.com/dcdpr/jp/pull/1216" }],
//!       "upgrade": { "title": "Command plugins are found by their manifest", "body": "..." }
//!     }
//!   ]
//! }
//! ```
//!
//! Untagged releases serialize with `"date": null`, unreleased changes also
//! have `"version": null`, and non-breaking entries have `"upgrade": null`.
//!
//! See `docs/rfd/122-release-pipeline.md`.
//!
//! [`forge`]: crate::forge

use std::{collections::BTreeMap, fmt, fs, io, num::NonZeroU64};

use camino::Utf8Path;
use doc::repository::web_address;
use semver::Version;
use serde::{Serialize, Serializer};

use crate::{
    directory::{self, CheckError, Contents, Filed, OVERVIEW, Part, ReadError},
    document::ListedCredit,
    forge::{Credit, Forge, NoAddress, Project},
    fragment::{Fragment, Type},
    git::{self, GitError},
    highlight::Highlight,
    history::{self, Commit, History},
    links::Links,
};

/// Schema version written in the `"format"` field when a [`Release`] is
/// serialized to JSON.
pub const FORMAT: u32 = 1;

/// Assembled release data prior to rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Release version, or `None` for unreleased changes.
    pub version: Option<Version>,

    /// Creation date (`YYYY-MM-DD`) of tag `v<version>`, or `None` if untagged.
    pub date: Option<String>,

    /// Release introduction as Markdown: the contents of
    /// `<version>/overview.md` if non-empty, or a generated entry count
    /// summary.
    pub overview: String,

    /// Highlights ordered by first credit and then by slug.
    pub highlights: Vec<Credited<Highlight>>,

    /// Fragment entries grouped by [`Type`] in release-note order, and within
    /// each type ordered by first credit and then by slug.
    pub entries: Vec<Credited<Fragment>>,
}

/// A fragment or highlight paired with its resolved [`Credit`] list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credited<T> {
    /// File slug (e.g. `retry-idle-turns` for `retry-idle-turns.fixed.md`).
    pub slug: String,

    /// Deduplicated change requests and commits credited for this file, ordered
    /// from oldest to newest.
    pub credits: Vec<Credit>,

    pub file: T,
}

/// Custom section headings for changelog [`Type`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headings(BTreeMap<Type, String>);

impl Headings {
    /// Set the section heading for `type_`.
    pub fn set(&mut self, type_: Type, heading: impl Into<String>) {
        self.0.insert(type_, heading.into());
    }

    /// Return the configured heading for `type_`, or its capitalized ID (e.g.
    /// `"Added"`).
    #[must_use]
    pub fn of(&self, type_: Type) -> String {
        match self.0.get(&type_) {
            Some(heading) => heading.clone(),
            None => capitalized(type_.id()),
        }
    }

    /// Iterate over configured `(Type, heading)` pairs in [`Type`] order.
    pub fn configured(&self) -> impl Iterator<Item = (Type, &str)> {
        self.0
            .iter()
            .map(|(type_, heading)| (*type_, heading.as_str()))
    }
}

impl FromIterator<(Type, String)> for Headings {
    fn from_iter<I: IntoIterator<Item = (Type, String)>>(headings: I) -> Self {
        Self(headings.into_iter().collect())
    }
}

/// Error returned by [`read`].
#[derive(Debug)]
pub enum ReleaseError {
    /// Failed to read or validate the fragments directory.
    Read(ReadError),

    /// Failed to read `<version>/overview.md`.
    Overview(CheckError),

    /// Repository is a shallow clone and lacks full commit history for credits.
    Shallow,

    /// A `git` subprocess failed.
    Git(GitError),

    /// `git log` output could not be parsed.
    Log(String),

    /// The forge requires a repository web URL for credit links and none is
    /// configured.
    NoAddress(NoAddress),
}

impl fmt::Display for ReleaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(f, "{error}"),
            Self::Overview(error) => write!(f, "{error}"),
            Self::Shallow => f.write_str(
                "the repository is a shallow clone, which lacks the history credits are read \
                 from; fetch the rest of it with `git fetch --unshallow`",
            ),
            Self::Git(error) => write!(f, "{error}"),
            Self::Log(message) => write!(
                f,
                "git's log of the fragments directory can't be read: {message}"
            ),
            Self::NoAddress(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ReleaseError {}

impl From<ReadError> for ReleaseError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl From<GitError> for ReleaseError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

impl From<NoAddress> for ReleaseError {
    fn from(error: NoAddress) -> Self {
        Self::NoAddress(error)
    }
}

/// Assemble the [`Release`] for `part` of the fragments directory `dir`.
///
/// [`Part::Unreleased`] includes both the top-level files and any `<version>/`
/// subdirectory that does not yet have a `v<version>` git tag.
///
/// Credits link to `repository` (defaulting to the web URL of git's `origin`
/// remote) on `forge` (defaulting to [`Forge::infer`]).
/// A tagged release takes its date from tag `v<version>`.
/// When `<version>/overview.md` is missing or empty, `headings` is used to
/// generate a fallback summary.
///
/// # Errors
///
/// Returns [`ReleaseError`] if the fragments directory or `overview.md` is
/// unreadable or invalid, if the repository is a shallow clone, if `git` fails,
/// or if `forge` requires a repository URL and none is available.
pub fn read(
    dir: &Utf8Path,
    part: &Part,
    forge: Option<Forge>,
    repository: Option<&str>,
    headings: &Headings,
) -> Result<Release, ReleaseError> {
    let Contents {
        fragments,
        highlights,
    } = match part {
        Part::Unreleased => unreleased(dir)?,
        Part::Release(_) => directory::read(dir, part)?,
    };

    let address = match repository {
        Some(address) => Some(address.to_owned()),
        None => git::origin(dir).as_deref().and_then(web_address),
    };
    let forge = forge.unwrap_or_else(|| Forge::infer(address.as_deref()));
    let project = Project::new(forge, address.as_deref())?;

    let history = History::read(dir)?;
    let commits = history.commits();

    let mut entries = credit(&project, commits, fragments);
    entries.sort_by(|(order, entry), (other_order, other)| {
        (entry.file.type_, order, &entry.slug).cmp(&(other.file.type_, other_order, &other.slug))
    });
    let mut highlights = credit(&project, commits, highlights);
    highlights.sort_by(|(order, highlight), (other_order, other)| {
        (order, &highlight.slug).cmp(&(other_order, &other.slug))
    });
    let entries: Vec<_> = entries.into_iter().map(|(_, entry)| entry).collect();
    let highlights: Vec<_> = highlights
        .into_iter()
        .map(|(_, highlight)| highlight)
        .collect();

    let (version, date, overview) = match part {
        Part::Unreleased => (None, None, None),
        Part::Release(version) => (
            Some(version.clone()),
            git::tag_date(dir, &format!("v{version}"))?,
            overview(&dir.join(version.to_string()))?,
        ),
    };
    let overview =
        overview.unwrap_or_else(|| summary(version.as_ref(), &entries, &highlights, headings));

    Ok(Release {
        version,
        date,
        overview,
        highlights,
        entries,
    })
}

trait Listed {
    fn credits(&self) -> Option<&[ListedCredit]>;
}

impl Listed for Fragment {
    fn credits(&self) -> Option<&[ListedCredit]> {
        self.credits.as_deref()
    }
}

impl Listed for Highlight {
    fn credits(&self) -> Option<&[ListedCredit]> {
        self.credits.as_deref()
    }
}

/// Sort key for ordering files by their first credit: change requests by
/// ascending number, then commits from oldest to newest, then uncredited files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Order {
    ChangeRequest(NonZeroU64),
    Commit(usize),
    Nobody,
}

fn credit<T: Listed>(
    project: &Project,
    commits: &[Commit],
    files: Vec<Filed<T>>,
) -> Vec<(Order, Credited<T>)> {
    let paths: Vec<&str> = files.iter().map(|filed| filed.path.as_str()).collect();
    let credited = history::credited(commits, &paths);

    files
        .into_iter()
        .zip(credited)
        .map(|(filed, indices)| {
            let (order, credits) = match filed.file.credits() {
                Some(listed) => listed_credits(project, commits, listed),
                None => history_credits(project, commits, &indices),
            };

            (order, Credited {
                slug: filed.slug,
                credits,
                file: filed.file,
            })
        })
        .collect()
}

fn listed_credits(
    project: &Project,
    commits: &[Commit],
    listed: &[ListedCredit],
) -> (Order, Vec<Credit>) {
    let order = listed.first().map_or(Order::Nobody, |credit| match credit {
        ListedCredit::ChangeRequest(number) => Order::ChangeRequest(*number),
        ListedCredit::Commit(hash) => Order::Commit(
            commits
                .iter()
                .position(|commit| commit.hash.starts_with(hash.as_str()))
                .map_or(usize::MAX, |index| commits.len() - index),
        ),
    });

    let mut credits = vec![];
    for credit in listed {
        let credit = match credit {
            ListedCredit::ChangeRequest(number) => project.change_request(*number),
            ListedCredit::Commit(hash) => project.commit(hash),
        };
        push_new(&mut credits, credit);
    }

    (order, credits)
}

fn history_credits(
    project: &Project,
    commits: &[Commit],
    indices: &[usize],
) -> (Order, Vec<Credit>) {
    let mut order = Order::Nobody;
    let mut credits = vec![];

    for &index in indices {
        let Some(commit) = commits.get(index) else {
            continue;
        };
        if credits.is_empty() {
            order = match project.forge().change_request(&commit.message) {
                Some(number) => Order::ChangeRequest(number),
                None => Order::Commit(commits.len() - index),
            };
        }
        push_new(&mut credits, project.credit(&commit.hash, &commit.message));
    }

    (order, credits)
}

/// Load all unreleased fragments and highlights: top-level files plus any
/// `<version>/` subdirectory without a `v<version>` tag.
fn unreleased(dir: &Utf8Path) -> Result<Contents, ReleaseError> {
    let mut contents = directory::read(dir, &Part::Unreleased)?;
    for version in directory::releases(dir).map_err(ReadError::from)? {
        if git::tag_date(dir, &format!("v{version}"))?.is_some() {
            continue;
        }

        let release = directory::read(dir, &Part::Release(version))?;
        contents.fragments.extend(release.fragments);
        contents.highlights.extend(release.highlights);
    }

    Ok(contents)
}

fn push_new(credits: &mut Vec<Credit>, credit: Credit) {
    if !credits.contains(&credit) {
        credits.push(credit);
    }
}

/// Read `<dir>/overview.md` with reference links inlined, returning `None` if
/// the file does not exist or contains only whitespace.
fn overview(dir: &Utf8Path) -> Result<Option<String>, ReleaseError> {
    let path = dir.join(OVERVIEW);
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(ReleaseError::Overview(CheckError { path, source })),
    };

    let source = source
        .strip_prefix('\u{feff}')
        .unwrap_or(&source)
        .replace("\r\n", "\n");
    let overview = Links::read(&source).standalone(&source, 0..source.len());
    let overview = overview.trim();

    Ok((!overview.is_empty()).then(|| overview.to_owned()))
}

/// Generate a fallback overview summarizing entry counts by type and highlight
/// titles.
fn summary(
    version: Option<&Version>,
    entries: &[Credited<Fragment>],
    highlights: &[Credited<Highlight>],
    headings: &Headings,
) -> String {
    let counts: Vec<String> = Type::ALL
        .into_iter()
        .filter_map(|type_| {
            let count = entries
                .iter()
                .filter(|entry| entry.file.type_ == type_)
                .count();
            (count > 0).then(|| format!("{} ({count})", headings.of(type_)))
        })
        .collect();
    let counts = series(&counts);

    let mut summary = match (version, entries.len()) {
        (Some(version), 0) => format!("Version {version} has no entries."),
        (Some(version), 1) => format!("Version {version} has 1 entry: {counts}."),
        (Some(version), count) => format!("Version {version} has {count} entries: {counts}."),
        (None, 0) => "No entries are unreleased.".to_owned(),
        (None, 1) => format!("1 entry is unreleased: {counts}."),
        (None, count) => format!("{count} entries are unreleased: {counts}."),
    };

    if !highlights.is_empty() {
        summary.push_str("\n\nHighlights:\n");
        for highlight in highlights {
            summary.push_str(&format!("\n- {}", highlight.file.title));
        }
    }

    summary
}

/// Format `items` as an Oxford-comma list (`a`, `a and b`, or `a, b, and c`).
pub(crate) fn series(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

fn capitalized(id: &str) -> String {
    let mut chars = id.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

impl Serialize for Release {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let highlights = self
            .highlights
            .iter()
            .map(|highlight| HighlightJson {
                slug: &highlight.slug,
                title: &highlight.file.title,
                authors: &highlight.file.authors,
                body: &highlight.file.body,
                credits: &highlight.credits,
            })
            .collect();
        let entries = self
            .entries
            .iter()
            .map(|entry| EntryJson {
                slug: &entry.slug,
                type_: entry.file.type_.id(),
                body: &entry.file.entry,
                credits: &entry.credits,
                upgrade: entry.file.upgrade.as_ref().map(|upgrade| UpgradeJson {
                    title: &upgrade.title,
                    body: &upgrade.body,
                }),
            })
            .collect();

        ReleaseJson {
            format: FORMAT,
            version: self.version.as_ref().map(ToString::to_string),
            date: self.date.as_deref(),
            overview: &self.overview,
            highlights,
            entries,
        }
        .serialize(serializer)
    }
}

#[derive(Serialize)]
struct ReleaseJson<'a> {
    format: u32,
    version: Option<String>,
    date: Option<&'a str>,
    overview: &'a str,
    highlights: Vec<HighlightJson<'a>>,
    entries: Vec<EntryJson<'a>>,
}

#[derive(Serialize)]
struct HighlightJson<'a> {
    slug: &'a str,
    title: &'a str,
    authors: &'a [String],
    body: &'a str,
    credits: &'a [Credit],
}

#[derive(Serialize)]
struct EntryJson<'a> {
    slug: &'a str,
    #[serde(rename = "type")]
    type_: &'static str,
    body: &'a str,
    credits: &'a [Credit],
    upgrade: Option<UpgradeJson<'a>>,
}

#[derive(Serialize)]
struct UpgradeJson<'a> {
    title: &'a str,
    body: &'a str,
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
