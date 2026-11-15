//! Layout validation and file loading for a fragments directory.
//!
//! The allowed paths inside a fragments directory are a closed set:
//!
//! ```text
//! changelog.d/
//!   README.md                     directory documentation (ignored)
//!   enable-explicit.removed.md    unreleased fragment
//!   highlights/
//!     fan-out.md                  unreleased highlight
//!   0.3.0/                        release subdirectory named by version
//!     overview.md                 release overview (read by release::read)
//!     retry-idle-turns.fixed.md   released fragment
//!     highlights/
//!       plugin-tools.md           released highlight
//! ```
//!
//! Fragments are named `<slug>.<type>.md` ([`fragment::name`]) and highlights
//! are named `<slug>.md`.
//! Any other file or directory is reported as a [`Problem`] rather than
//! silently skipped, so stray files cannot be omitted when a release is
//! assembled.
//!
//! [`check`] validates the entire directory, including released subdirectories.
//! [`read`] validates and loads a single [`Part`] (either the top-level
//! unreleased files or one release subdirectory).

use std::{fmt, fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use semver::Version;

use crate::{
    fragment::{self, Fragment, FragmentError, NameError},
    highlight::{self, Highlight, HighlightError},
};

/// Top-level documentation file skipped during fragment checks.
const README: &str = "README.md";

/// Release introduction file in `<version>/overview.md`.
pub(crate) const OVERVIEW: &str = "overview.md";

const HIGHLIGHTS: &str = "highlights";
const EXTENSION: &str = ".md";

/// Validation summary returned by [`check`].
#[derive(Debug, Default)]
pub struct Report {
    /// Number of fragment files checked, including released subdirectories.
    pub fragments: usize,

    /// Number of highlight files checked, including released subdirectories.
    pub highlights: usize,

    /// Validation problems sorted by file path.
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

/// A validation failure at a path in a fragments directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Path to the offending file or directory, joined onto the root passed to
    /// [`check`].
    pub path: Utf8PathBuf,

    pub kind: ProblemKind,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.kind)
    }
}

/// Reason a path failed fragments directory validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemKind {
    /// Fragment file failed format validation.
    MalformedFragment(FragmentError),

    /// Highlight file failed format validation.
    MalformedHighlight(HighlightError),

    /// File contents are not valid UTF-8.
    NotUtf8,

    /// File outside `highlights/` is not named `<slug>.<type>.md`.
    FragmentName(NameError),

    /// File inside `highlights/` is not named `<slug>.md`.
    NotAHighlight,

    /// Top-level subdirectory is neither `highlights/` nor a semantic version.
    NotARelease,

    /// Subdirectory inside `<version>/` other than `highlights/`.
    NestedInRelease,

    /// Subdirectory inside `highlights/`.
    NestedInHighlights,
}

impl fmt::Display for ProblemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedFragment(error) => write!(f, "{error}"),
            Self::MalformedHighlight(error) => write!(f, "{error}"),
            Self::NotUtf8 => f.write_str("not UTF-8 text"),
            Self::FragmentName(error) => write!(f, "{error}"),
            Self::NotAHighlight => f.write_str(
                "not a highlight, which is named `<slug>.md`; rename it, or move it out of the \
                 fragments directory",
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

/// I/O error reading a file or directory during [`check`] or [`read`].
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

/// Subset of a fragments directory loaded by [`read`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Top-level unreleased fragments and highlights.
    Unreleased,

    /// Subdirectory for a specific release version.
    Release(Version),
}

/// Location of a fragment or highlight within a fragments directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// Top-level unreleased area or a release subdirectory.
    pub part: Part,

    /// Whether the file lives under `highlights/`.
    pub highlight: bool,

    /// File name (e.g. `retry-idle-turns.fixed.md`).
    pub name: String,
}

impl Place {
    /// Parse a `/`-separated path relative to the fragments directory (e.g.
    /// `retry-idle-turns.fixed.md` or `0.3.0/highlights/fan-out.md`).
    ///
    /// Returns `None` if `path` is not a valid fragment or highlight location.
    #[must_use]
    pub fn parse(path: &str) -> Option<Self> {
        let mut parts: Vec<&str> = path.split('/').collect();
        let name = parts.pop().filter(|name| !name.is_empty())?;
        let highlight = parts.last() == Some(&HIGHLIGHTS);
        if highlight {
            parts.pop();
        }

        let part = match parts.as_slice() {
            [] => Part::Unreleased,
            [version] => Part::Release(Version::parse(version).ok()?),
            _ => return None,
        };

        Some(Self {
            part,
            highlight,
            name: name.to_owned(),
        })
    }

    /// Format the `/`-separated path relative to the fragments directory.
    #[must_use]
    pub fn path(&self) -> String {
        let mut path = match &self.part {
            Part::Unreleased => String::new(),
            Part::Release(version) => format!("{version}/"),
        };
        if self.highlight {
            path.push_str(HIGHLIGHTS);
            path.push('/');
        }
        path.push_str(&self.name);

        path
    }
}

/// Parsed fragments and highlights from a single [`Part`] of a fragments
/// directory, sorted by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contents {
    pub fragments: Vec<Filed<Fragment>>,
    pub highlights: Vec<Filed<Highlight>>,
}

/// A parsed fragment or highlight paired with its location in the fragments
/// directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filed<T> {
    /// File slug without `.md` or `.<type>` (e.g. `retry-idle-turns` for
    /// `retry-idle-turns.fixed.md`).
    pub slug: String,

    /// `/`-separated path relative to the fragments directory (e.g.
    /// `0.3.0/highlights/fan-out.md`).
    pub path: String,

    pub file: T,
}

/// Error returned by [`read`].
#[derive(Debug)]
pub enum ReadError {
    /// The release subdirectory `<dir>/<version>/` does not exist.
    NoRelease { dir: Utf8PathBuf, version: Version },

    /// A file or directory could not be read.
    Unreadable(CheckError),

    /// One or more paths or files failed validation.
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

/// Validate every path, fragment, and highlight in `dir`, including released
/// subdirectories.
///
/// Skips top-level `README.md` and per-release `overview.md` files.
///
/// # Errors
///
/// Returns [`CheckError`] if `dir` or any entry inside it cannot be read.
pub fn check(dir: &Utf8Path) -> Result<Report, CheckError> {
    let mut report = Report::default();
    read_top_level(dir, true, &mut report, &mut Contents::default())?;

    Ok(report)
}

/// Validate and load all fragments and highlights in `part` of `dir`.
///
/// [`Part::Unreleased`] reads only the top level without descending into
/// release subdirectories.
///
/// # Errors
///
/// Returns [`ReadError::NoRelease`] if the release subdirectory is missing,
/// [`ReadError::Unreadable`] on I/O failure, or [`ReadError::Problems`] if any
/// path or file in `part` is invalid.
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

/// Return the release versions that have subdirectories in `dir`, sorted from
/// oldest to newest.
///
/// # Errors
///
/// Returns [`CheckError`] if `dir` cannot be read.
pub fn releases(dir: &Utf8Path) -> Result<Vec<Version>, CheckError> {
    let mut versions: Vec<Version> = entries(dir)?
        .into_iter()
        .filter(|entry| entry.is_dir)
        .filter_map(|entry| Version::parse(&entry.name).ok())
        .collect();
    versions.sort();

    Ok(versions)
}

fn read_top_level(
    dir: &Utf8Path,
    releases: bool,
    report: &mut Report,
    contents: &mut Contents,
) -> Result<(), CheckError> {
    for entry in entries(dir)? {
        let path = dir.join(&entry.name);
        if entry.is_dir {
            read_top_level_dir(&path, &entry.name, releases, report, contents)?;
        } else if entry.name != README {
            read_file(&path, &entry.name, "", Format::Fragment, report, contents)?;
        }
    }

    Ok(())
}

fn read_top_level_dir(
    path: &Utf8Path,
    name: &str,
    releases: bool,
    report: &mut Report,
    contents: &mut Contents,
) -> Result<(), CheckError> {
    if name == HIGHLIGHTS {
        return read_highlights(path, &format!("{HIGHLIGHTS}/"), report, contents);
    }
    if !is_version(name) {
        report.problem(path, ProblemKind::NotARelease);
        return Ok(());
    }
    if releases {
        read_release(path, &format!("{name}/"), report, contents)?;
    }

    Ok(())
}

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

#[derive(Clone, Copy)]
enum Format {
    Fragment,
    Highlight,
}

fn read_file(
    path: &Utf8Path,
    name: &str,
    below: &str,
    format: Format,
    report: &mut Report,
    contents: &mut Contents,
) -> Result<(), CheckError> {
    let (slug, type_) = match format {
        Format::Fragment => match fragment::name(name) {
            Ok((slug, type_)) => (slug, Some(type_)),
            Err(error) => {
                report.problem(path, ProblemKind::FragmentName(error));
                return Ok(());
            }
        },
        Format::Highlight => {
            let Some(slug) = name.strip_suffix(EXTENSION).filter(|slug| !slug.is_empty()) else {
                report.problem(path, ProblemKind::NotAHighlight);
                return Ok(());
            };
            (slug, None)
        }
    };

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

    let slug = slug.to_owned();
    let filed = format!("{below}{name}");
    match type_ {
        Some(type_) => match fragment::parse(type_, &source) {
            Ok(file) => contents.fragments.push(Filed {
                slug,
                path: filed,
                file,
            }),
            Err(error) => report.problem(path, ProblemKind::MalformedFragment(error)),
        },
        None => match highlight::parse(&source) {
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

/// Return whether `path` (relative to the fragments directory) is a top-level
/// fragment or highlight rather than inside a `<version>/` subdirectory.
pub(crate) fn is_unreleased(path: &str) -> bool {
    let name = path
        .strip_prefix(HIGHLIGHTS)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(path);

    !name.is_empty() && !name.contains('/')
}

/// Return whether `name` is a valid semantic version without a `v` prefix.
fn is_version(name: &str) -> bool {
    Version::parse(name).is_ok()
}

struct Entry {
    name: String,
    is_dir: bool,
}

/// Read and sort the entries of `dir` by name for deterministic output.
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

fn counted(count: usize, one: &str, many: &str) -> String {
    let noun = if count == 1 { one } else { many };
    format!("{count} {noun}")
}

#[cfg(test)]
#[path = "directory_tests.rs"]
mod tests;
