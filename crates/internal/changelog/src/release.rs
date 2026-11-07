//! A release, as its notes are built: its version and date, its overview, and
//! every highlight and fragment it ships, each with who it credits.
//!
//! [`read`] builds one from a release's directory, or from the unreleased top
//! level of a fragments directory.
//! It credits each file to the commits that wrote or changed it while it sat at
//! the top level, read from the default branch's history, and the `forge`
//! module says what such a credit reads like and where it links.
//!
//! A [`Release`] serializes to the JSON other tools render from, versioned by
//! [`FORMAT`]:
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
//! A release without a tag has a `null` date, and the unreleased fragments have
//! a `null` version too.
//! An entry without an upgrade section has a `null` `upgrade`.
//!
//! See `docs/rfd/122-release-pipeline.md`.

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

/// The version of the JSON a [`Release`] serializes to, written as its
/// `format`, so a consumer can refuse a shape it doesn't know.
pub const FORMAT: u32 = 1;

/// A release's notes, before they are rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The release's version; `None` for the fragments no release has taken.
    pub version: Option<Version>,

    /// The day the release's tag was made, as `YYYY-MM-DD`; `None` while the
    /// release has no tag.
    pub date: Option<String>,

    /// The release's introduction, as Markdown: what its `overview.md` holds,
    /// or else a summary of what it ships.
    pub overview: String,

    /// The highlights, ordered by their first credit and then by slug.
    pub highlights: Vec<Credited<Highlight>>,

    /// The fragments, grouped by type in the order release notes show the
    /// types, and within a type ordered by their first credit and then by slug.
    pub entries: Vec<Credited<Fragment>>,
}

/// A fragment or highlight in a release, and who it credits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credited<T> {
    /// The file's slug, such as `retry-idle-turns` for
    /// `retry-idle-turns.fixed.md`.
    pub slug: String,

    /// The change requests and commits that wrote or changed the file while it
    /// sat at the top level, oldest first and each once, or the ones its
    /// `Credits` fields list instead.
    pub credits: Vec<Credit>,

    /// The file, read into its format.
    pub file: T,
}

/// The heading each type's section has in the release notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headings(BTreeMap<Type, String>);

impl Headings {
    /// Head the section of `type_` with `heading`.
    pub fn set(&mut self, type_: Type, heading: impl Into<String>) {
        self.0.insert(type_, heading.into());
    }

    /// The heading of the section of `type_`: the one it was given, or else its
    /// id capitalized, such as `Added`.
    #[must_use]
    pub fn of(&self, type_: Type) -> String {
        match self.0.get(&type_) {
            Some(heading) => heading.clone(),
            None => capitalized(type_.id()),
        }
    }
}

impl FromIterator<(Type, String)> for Headings {
    fn from_iter<I: IntoIterator<Item = (Type, String)>>(headings: I) -> Self {
        Self(headings.into_iter().collect())
    }
}

/// Why [`read`] couldn't build a release.
#[derive(Debug)]
pub enum ReleaseError {
    /// The fragments directory, or the part of it the release is, can't be
    /// read, or breaks the rules `check` holds it to.
    Read(ReadError),

    /// The release's `overview.md` can't be read.
    Overview(CheckError),

    /// The repository is a shallow clone, without the history credits are read
    /// from.
    Shallow,

    /// A git command couldn't be run, or failed.
    Git(GitError),

    /// git's log of the fragments directory doesn't read as one.
    Log(String),

    /// The forge links its credits, and there is no web address to link at.
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

/// Build the release that `part` of the fragments directory `dir` holds: a
/// release's directory, or the unreleased top level.
///
/// Credits link to the project on `forge`, at the web address of git's `origin`
/// remote; without a `forge`, to the one that address is on (see
/// [`Forge::infer`]).
/// A release's date is its tag's, `v<version>`.
/// `headings` names the types in the summary that stands in for a missing
/// overview.
///
/// # Errors
///
/// Returns why the part or its overview can't be read, refuses a shallow clone
/// rather than build a release without its credits, and returns a git command
/// that failed.
pub fn read(
    dir: &Utf8Path,
    part: &Part,
    forge: Option<Forge>,
    headings: &Headings,
) -> Result<Release, ReleaseError> {
    let Contents {
        fragments,
        highlights,
    } = directory::read(dir, part)?;

    let address = git::origin(dir).as_deref().and_then(web_address);
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

/// A file that can list who to credit in place of the ones history names.
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

/// Where a file sorts among the others of its kind, by its first credit: change
/// requests by number, then commits oldest first, then the files crediting
/// nobody.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Order {
    ChangeRequest(NonZeroU64),

    /// A commit, by how many commits of the log are as old as it or older.
    Commit(usize),

    Nobody,
}

/// Each of `files` with its credits, from the project's history `commits`, and
/// where it sorts by its first credit.
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

/// The credits a file lists as `listed`, and where it sorts by the first of
/// them.
///
/// A listed commit sorts by its place in the project's history `commits`, and
/// after every commit there when the history doesn't hold it.
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
                // The log lists the newest first.
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

/// The credits for the commits of `commits` at `indices`, oldest first.
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
                // The log lists the newest first.
                None => Order::Commit(commits.len() - index),
            };
        }
        push_new(&mut credits, project.credit(&commit.hash, &commit.message));
    }

    (order, credits)
}

/// Add `credit` to `credits` unless they hold it already.
fn push_new(credits: &mut Vec<Credit>, credit: Credit) {
    if !credits.contains(&credit) {
        credits.push(credit);
    }
}

/// What `overview.md` in the release's directory `dir` holds, with its links
/// inline, or `None` when there is no such file or it holds nothing but
/// whitespace.
///
/// # Errors
///
/// Returns [`ReleaseError::Overview`] when the file exists but can't be read.
pub fn overview(dir: &Utf8Path) -> Result<Option<String>, ReleaseError> {
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

/// What stands in for the overview of a release without one: its version, how
/// many entries of each type it has, and its highlights' titles.
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

/// `items` as a sentence lists them: `a`, `a and b`, or `a, b, and c`.
pub(crate) fn series(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// `id` with its first letter in upper case.
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

/// A [`Release`] as its JSON holds it.
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
