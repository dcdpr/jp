//! A fragments directory, held to its layout, and every fragment and highlight
//! in it to its format.
//!
//! The paths a fragments directory holds are a closed set:
//!
//! ```text
//! changelog.d/
//!   README.md               the directory's documentation, never read
//!   enable-explicit.md      a fragment no release has taken yet
//!   highlights/
//!     fan-out.md            a highlight no release has taken yet
//!   0.3.0/                  a release's directory, named after its version
//!     overview.md           the release's introduction, never read here
//!     retry-idle-turns.md   a fragment the release took
//!     highlights/
//!       plugin-tools.md     a highlight the release took
//! ```
//!
//! A fragment or highlight is `<slug>.md`, and no slug is `overview` or
//! `readme`, in any letter case.
//! Anything else is a [`Problem`] rather than a file left unread, so a stray
//! file fails loudly instead of vanishing when a release is assembled.
//!
//! [`check`] holds the whole directory to these rules.
//! [`read`] holds one [`Part`] of it to them, the top level or one release's
//! directory, and returns each fragment and highlight in it, read into its
//! format.

use std::{fmt, fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use semver::Version;

use crate::{
    fragment::{self, Fragment, FragmentError},
    highlight::{self, Highlight, HighlightError},
};

/// The file at the top level that documents the directory.
const README: &str = "README.md";

/// The file in a release's directory that introduces the release.
pub(crate) const OVERVIEW: &str = "overview.md";

/// The directory, at the top level and in each release's directory, that holds
/// highlights.
const HIGHLIGHTS: &str = "highlights";

/// The extension every fragment's and highlight's file name ends in.
const EXTENSION: &str = ".md";

/// Each slug no fragment or highlight can have, in any letter case, and what it
/// is reserved for: a case-insensitive filesystem reads `readme.md` and
/// `README.md` as one file.
const RESERVED: [(&str, &str); 2] = [
    (
        "overview",
        "a release's introduction, `overview.md` in the release's directory",
    ),
    (
        "readme",
        "the fragments directory's documentation, `README.md` at its top level",
    ),
];

/// What [`check`] found in a fragments directory.
#[derive(Debug, Default)]
pub struct Report {
    /// How many files were read as fragments, released ones included.
    pub fragments: usize,

    /// How many files were read as highlights, released ones included.
    pub highlights: usize,

    /// Every problem, in the order of the paths they concern.
    pub problems: Vec<Problem>,
}

impl Report {
    fn problem(&mut self, path: &Utf8Path, kind: ProblemKind) {
        self.problems.push(Problem {
            path: path.to_owned(),
            kind,
        });
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for problem in &self.problems {
            writeln!(f, "{problem}")?;
        }

        let checked = format!(
            "{} and {}",
            counted(self.fragments, "fragment", "fragments"),
            counted(self.highlights, "highlight", "highlights"),
        );
        match self.problems.len() {
            0 => write!(f, "Checked {checked}: no problems."),
            count => write!(
                f,
                "\nChecked {checked}: {}.",
                counted(count, "problem", "problems")
            ),
        }
    }
}

/// A path in a fragments directory that breaks a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The file or directory, as the fragments directory [`check`] was given
    /// joined with the path below it.
    pub path: Utf8PathBuf,

    /// The rule it breaks.
    pub kind: ProblemKind,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.kind)
    }
}

/// The rule a [`Problem`] breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemKind {
    /// A fragment that doesn't follow its format.
    MalformedFragment(FragmentError),

    /// A highlight that doesn't follow its format.
    MalformedHighlight(HighlightError),

    /// A fragment or highlight whose contents aren't UTF-8 text.
    NotUtf8,

    /// A file outside a `highlights/` directory whose name isn't `<slug>.md`.
    NotAFragment,

    /// A file in a `highlights/` directory whose name isn't `<slug>.md`.
    NotAHighlight,

    /// A fragment or highlight whose slug is reserved.
    Reserved {
        slug: &'static str,
        reserved_for: &'static str,
    },

    /// A directory at the top level that is neither `highlights/` nor named
    /// after a semantic version.
    NotARelease,

    /// A directory in a release's directory other than its `highlights/`.
    NestedInRelease,

    /// A directory in a `highlights/` directory.
    NestedInHighlights,
}

impl fmt::Display for ProblemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedFragment(error) => write!(f, "{error}"),
            Self::MalformedHighlight(error) => write!(f, "{error}"),
            Self::NotUtf8 => f.write_str("not UTF-8 text"),
            Self::NotAFragment => f.write_str(
                "not a fragment, which is named `<slug>.md`; rename it, or move it out of the \
                 fragments directory",
            ),
            Self::NotAHighlight => f.write_str(
                "not a highlight, which is named `<slug>.md`; rename it, or move it out of the \
                 fragments directory",
            ),
            Self::Reserved { slug, reserved_for } => write!(
                f,
                "the slug `{slug}` is reserved, in any letter case, for {reserved_for}; rename it"
            ),
            Self::NotARelease => f.write_str(
                "neither `highlights/` nor a release's directory, which is named after its \
                 version, such as `0.3.0`",
            ),
            Self::NestedInRelease => {
                f.write_str("a release's directory holds no directory but `highlights/`")
            }
            Self::NestedInHighlights => {
                f.write_str("a `highlights/` directory holds no directories")
            }
        }
    }
}

/// A file or directory [`check`] or [`read`] couldn't read.
#[derive(Debug)]
pub struct CheckError {
    pub path: Utf8PathBuf,
    pub source: io::Error,
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot read {}: {}", self.path, self.source)
    }
}

impl std::error::Error for CheckError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// One part of a fragments directory, which [`read`] reads on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// The fragments and highlights at the top level, which no release has
    /// taken yet.
    Unreleased,

    /// The directory of the release with this version.
    Release(Version),
}

/// The fragments and highlights one [`Part`] of a fragments directory holds,
/// each read into its format, in the order of their paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contents {
    pub fragments: Vec<Filed<Fragment>>,
    pub highlights: Vec<Filed<Highlight>>,
}

/// A fragment or highlight, and where it is filed in the fragments directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filed<T> {
    /// The file's name, without `.md`.
    pub slug: String,

    /// The file's path below the fragments directory, with `/` between its
    /// parts on every platform, such as `0.3.0/highlights/fan-out.md`.
    pub path: String,

    /// The file, read into its format.
    pub file: T,
}

/// Why [`read`] couldn't return the contents of a part.
#[derive(Debug)]
pub enum ReadError {
    /// The fragments directory has no directory for the release.
    NoRelease { dir: Utf8PathBuf, version: Version },

    /// A file or directory couldn't be read.
    Unreadable(CheckError),

    /// Paths in the part break the layout, or files in it their format.
    Problems(Vec<Problem>),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRelease { dir, version } => {
                write!(f, "{dir} has no release {version}: no directory {version}/")
            }
            Self::Unreadable(error) => write!(f, "{error}"),
            Self::Problems(problems) => {
                let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
                f.write_str(&lines.join("\n"))
            }
        }
    }
}

impl std::error::Error for ReadError {}

impl From<CheckError> for ReadError {
    fn from(error: CheckError) -> Self {
        Self::Unreadable(error)
    }
}

/// Check every path in the fragments directory `dir` against the layout, and
/// every fragment and highlight against its format, released ones included.
///
/// `README.md` at the top level and `overview.md` in a release's directory are
/// never read: neither holds a fragment.
///
/// # Errors
///
/// Returns the first file or directory that can't be read, `dir` itself
/// included: a missing fragments directory is an error, not an empty one.
pub fn check(dir: &Utf8Path) -> Result<Report, CheckError> {
    let mut report = Report::default();
    read_top_level(dir, true, &mut report, &mut Contents::default())?;

    Ok(report)
}

/// Read the fragments and highlights of `part` of the fragments directory
/// `dir`, holding its paths to the layout and its files to their formats as
/// [`check`] does.
///
/// [`Part::Unreleased`] reads the top level without entering any release's
/// directory.
///
/// # Errors
///
/// Returns [`ReadError::NoRelease`] for a release without a directory, the
/// first file or directory that can't be read, or every [`Problem`] in the
/// part.
pub fn read(dir: &Utf8Path, part: &Part) -> Result<Contents, ReadError> {
    let mut report = Report::default();
    let mut contents = Contents::default();

    match part {
        Part::Unreleased => read_top_level(dir, false, &mut report, &mut contents)?,
        Part::Release(version) => {
            let name = version.to_string();
            let path = dir.join(&name);
            if !path.is_dir() {
                return Err(ReadError::NoRelease {
                    dir: dir.to_owned(),
                    version: version.clone(),
                });
            }
            read_release(&path, &format!("{name}/"), &mut report, &mut contents)?;
        }
    }

    if !report.problems.is_empty() {
        return Err(ReadError::Problems(report.problems));
    }

    Ok(contents)
}

/// Read the top level of the fragments directory `dir`, and each release's
/// directory in it when `releases` is set.
fn read_top_level(
    dir: &Utf8Path,
    releases: bool,
    report: &mut Report,
    contents: &mut Contents,
) -> Result<(), CheckError> {
    for entry in entries(dir)? {
        let path = dir.join(&entry.name);
        if entry.is_dir {
            if entry.name == HIGHLIGHTS {
                read_highlights(&path, &format!("{HIGHLIGHTS}/"), report, contents)?;
            } else if !is_version(&entry.name) {
                report.problem(&path, ProblemKind::NotARelease);
            } else if releases {
                read_release(&path, &format!("{}/", entry.name), report, contents)?;
            }
        } else if entry.name != README {
            read_file(&path, &entry.name, "", Format::Fragment, report, contents)?;
        }
    }

    Ok(())
}

/// Read the release's directory `dir`, which is `below` in the fragments
/// directory.
fn read_release(
    dir: &Utf8Path,
    below: &str,
    report: &mut Report,
    contents: &mut Contents,
) -> Result<(), CheckError> {
    for entry in entries(dir)? {
        let path = dir.join(&entry.name);
        if entry.is_dir {
            if entry.name == HIGHLIGHTS {
                read_highlights(&path, &format!("{below}{HIGHLIGHTS}/"), report, contents)?;
            } else {
                report.problem(&path, ProblemKind::NestedInRelease);
            }
        } else if entry.name != OVERVIEW {
            read_file(
                &path,
                &entry.name,
                below,
                Format::Fragment,
                report,
                contents,
            )?;
        }
    }

    Ok(())
}

/// Read the `highlights/` directory `dir`, at the top level or in a release's
/// directory, which is `below` in the fragments directory.
fn read_highlights(
    dir: &Utf8Path,
    below: &str,
    report: &mut Report,
    contents: &mut Contents,
) -> Result<(), CheckError> {
    for entry in entries(dir)? {
        let path = dir.join(&entry.name);
        if entry.is_dir {
            report.problem(&path, ProblemKind::NestedInHighlights);
        } else {
            read_file(
                &path,
                &entry.name,
                below,
                Format::Highlight,
                report,
                contents,
            )?;
        }
    }

    Ok(())
}

/// The format a file is held to, decided by the directory it sits in.
#[derive(Clone, Copy)]
enum Format {
    Fragment,
    Highlight,
}

/// Read the file at `path`, named `name` and `below` in the fragments
/// directory, as `format`, into `contents` when it follows the format and into
/// `report` when it doesn't.
fn read_file(
    path: &Utf8Path,
    name: &str,
    below: &str,
    format: Format,
    report: &mut Report,
    contents: &mut Contents,
) -> Result<(), CheckError> {
    if let Some(kind) = misnamed(name, format) {
        report.problem(path, kind);
        return Ok(());
    }

    match format {
        Format::Fragment => report.fragments += 1,
        Format::Highlight => report.highlights += 1,
    }
    let bytes = fs::read(path).map_err(|source| CheckError {
        path: path.to_owned(),
        source,
    })?;
    let Ok(source) = String::from_utf8(bytes) else {
        report.problem(path, ProblemKind::NotUtf8);
        return Ok(());
    };

    // `misnamed` refused a name without the extension.
    let slug = name.strip_suffix(EXTENSION).unwrap_or(name).to_owned();
    let filed = format!("{below}{name}");
    match format {
        Format::Fragment => match fragment::parse(&source) {
            Ok(file) => contents.fragments.push(Filed {
                slug,
                path: filed,
                file,
            }),
            Err(error) => report.problem(path, ProblemKind::MalformedFragment(error)),
        },
        Format::Highlight => match highlight::parse(&source) {
            Ok(file) => contents.highlights.push(Filed {
                slug,
                path: filed,
                file,
            }),
            Err(error) => report.problem(path, ProblemKind::MalformedHighlight(error)),
        },
    }

    Ok(())
}

/// Why `name` isn't the file name of a file in `format`, if it isn't.
fn misnamed(name: &str, format: Format) -> Option<ProblemKind> {
    let Some(slug) = name.strip_suffix(EXTENSION).filter(|slug| !slug.is_empty()) else {
        return Some(match format {
            Format::Fragment => ProblemKind::NotAFragment,
            Format::Highlight => ProblemKind::NotAHighlight,
        });
    };

    reservation(slug).map(|(slug, reserved_for)| ProblemKind::Reserved { slug, reserved_for })
}

/// What `slug` is reserved for, when it is one of the slugs no fragment or
/// highlight can have, in any letter case.
#[must_use]
pub fn reserved_for(slug: &str) -> Option<&'static str> {
    reservation(slug).map(|(_, reserved_for)| reserved_for)
}

/// The reserved slug `slug` spells, and what it is reserved for, if it spells
/// one.
fn reservation(slug: &str) -> Option<(&'static str, &'static str)> {
    RESERVED
        .into_iter()
        .find(|(reserved, _)| slug.eq_ignore_ascii_case(reserved))
}

/// Whether `path`, below the fragments directory with `/` between its parts, is
/// a file at the top level or in its `highlights/`, where a fragment or
/// highlight sits until a release takes it.
pub(crate) fn is_unreleased(path: &str) -> bool {
    let name = path
        .strip_prefix(HIGHLIGHTS)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(path);

    !name.is_empty() && !name.contains('/')
}

/// Whether `name` names a release's directory: a semantic version, such as
/// `0.3.0` or `1.0.0-rc.1`, with no `v` in front.
fn is_version(name: &str) -> bool {
    Version::parse(name).is_ok()
}

/// One entry of a directory.
struct Entry {
    name: String,

    /// Whether the entry is a directory, or a symbolic link to one.
    is_dir: bool,
}

/// Every entry of `dir`, ordered by name, so problems are reported in the same
/// order on every machine.
fn entries(dir: &Utf8Path) -> Result<Vec<Entry>, CheckError> {
    let unreadable = |source| CheckError {
        path: dir.to_owned(),
        source,
    };

    let mut entries = vec![];
    for entry in dir.read_dir_utf8().map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        entries.push(Entry {
            name: entry.file_name().to_owned(),
            is_dir: entry.path().is_dir(),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(entries)
}

/// `count` followed by the noun it counts.
fn counted(count: usize, one: &str, many: &str) -> String {
    let noun = if count == 1 { one } else { many };
    format!("{count} {noun}")
}

#[cfg(test)]
#[path = "directory_tests.rs"]
mod tests;
