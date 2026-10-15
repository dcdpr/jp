//! RFD identifiers, and the one file each names.
//!
//! An RFD is named by its permanent number (`042`) once it reached Discussion,
//! and by its draft slot (`D05`) before that.
//! [`RfdId`] reads an id the way people type one (`RFD 042`, `42`, `d5`) and
//! always writes it in canonical form.
//! [`resolve`] finds the file an id names, and refuses when no file or more
//! than one does.

use std::{collections::HashSet, error, fmt, fs, io, str::FromStr};

use camino::{Utf8Path, Utf8PathBuf};

/// The directory drafts live in, inside the RFD directory.
pub(crate) const DRAFTS_DIR: &str = "drafts";

/// The highest permanent number: published filenames carry three digits.
const MAX_NUMBER: u16 = 999;

/// The highest draft slot: draft filenames carry two digits.
const MAX_SLOT: u8 = 99;

/// An RFD's identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RfdId {
    /// A permanent number, `001` to `999`.
    ///
    /// `000` is not an RFD: the templates carry it.
    Published(u16),

    /// A draft slot, `D01` to `D99`.
    Draft(u8),
}

impl RfdId {
    /// The prefix the file this id names starts with: `042-` or `D05-`.
    #[must_use]
    pub(crate) fn file_prefix(self) -> String {
        format!("{self}-")
    }

    /// The directory, inside the RFD directory `dir`, that holds this id's
    /// file.
    #[must_use]
    pub(crate) fn directory(self, dir: &Utf8Path) -> Utf8PathBuf {
        match self {
            Self::Published(_) => dir.to_path_buf(),
            Self::Draft(_) => dir.join(DRAFTS_DIR),
        }
    }

    /// The id a file name opens with, if it opens with one: `042-slug.md` is
    /// `042`, `D05-slug.md` is `D05`.
    ///
    /// The digits are taken as written, so `42-slug.md` and `D5-slug.md` name
    /// no id.
    #[must_use]
    pub(crate) fn of_file(name: &str) -> Option<Self> {
        let (id, _) = name.split_once('-')?;
        let fixed = |digits: &str, count: usize| {
            (digits.len() == count && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| digits.parse::<u16>().ok())
                .flatten()
        };

        if let Some(slot) = id.strip_prefix('D') {
            let slot = u8::try_from(fixed(slot, 2)?).ok()?;
            return (1..=MAX_SLOT).contains(&slot).then_some(Self::Draft(slot));
        }

        let number = fixed(id, 3)?;
        (1..=MAX_NUMBER)
            .contains(&number)
            .then_some(Self::Published(number))
    }
}

impl fmt::Display for RfdId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Published(number) => write!(f, "{number:03}"),
            Self::Draft(slot) => write!(f, "D{slot:02}"),
        }
    }
}

/// Text that names no RFD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvalidId(pub String);

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid RFD id '{}'. Use a number (41) or draft ID (D01).",
            self.0
        )
    }
}

impl error::Error for InvalidId {}

impl FromStr for RfdId {
    type Err = InvalidId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let given = s.trim();
        let invalid = || InvalidId(given.to_owned());

        // `RFD 042`, the way the documents cite each other.
        let bare = strip_rfd_prefix(given);

        if let Some(slot) = bare.strip_prefix(['D', 'd']) {
            let slot = digits(slot).ok_or_else(invalid)?;
            return u8::try_from(slot)
                .ok()
                .filter(|slot| (1..=MAX_SLOT).contains(slot))
                .map(Self::Draft)
                .ok_or_else(invalid);
        }

        let number = digits(bare).ok_or_else(invalid)?;
        u16::try_from(number)
            .ok()
            .filter(|number| (1..=MAX_NUMBER).contains(number))
            .map(Self::Published)
            .ok_or_else(invalid)
    }
}

/// `text` with a leading `RFD` and the whitespace after it removed, or as it is
/// without one.
fn strip_rfd_prefix(text: &str) -> &str {
    let Some((head, rest)) = text.split_at_checked(3) else {
        return text;
    };

    if head.eq_ignore_ascii_case("rfd") && rest.starts_with(char::is_whitespace) {
        rest.trim_start()
    } else {
        text
    }
}

/// The value of a run of ASCII digits, or `None` for anything else.
fn digits(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    // Leading zeros carry no value, and a long run of them is still a small
    // number.
    let significant = text.trim_start_matches('0');
    if significant.is_empty() {
        return Some(0);
    }

    significant.parse().ok()
}

/// Why an id names no single file.
#[derive(Debug)]
pub(crate) enum ResolveError {
    /// No file carries the id.
    NotFound(RfdId),

    /// More than one file carries the id.
    Ambiguous { id: RfdId, files: Vec<Utf8PathBuf> },

    /// The directory couldn't be read.
    Io { dir: Utf8PathBuf, error: io::Error },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id @ RfdId::Published(_)) => write!(f, "No RFD found with number {id}."),
            Self::NotFound(id @ RfdId::Draft(_)) => write!(f, "No draft RFD found with ID {id}."),
            Self::Ambiguous { id, files } => {
                writeln!(f, "Ambiguous: {} files share ID {id}:", files.len())?;
                for file in files {
                    writeln!(f, "  {file}")?;
                }
                write!(
                    f,
                    "Renumber or remove one before continuing; a rewrite keyed on {id} would \
                     corrupt the others."
                )
            }
            Self::Io { dir, error } => write!(f, "{dir}: {error}"),
        }
    }
}

impl error::Error for ResolveError {}

/// Find the one file `id` names in the RFD directory `dir`.
///
/// Drafts are looked up in its `drafts/` directory.
/// Two files sharing an id is refused rather than settled by picking one: a
/// command that rewrites the file it was given would rewrite the wrong
/// document.
pub(crate) fn resolve(dir: &Utf8Path, id: RfdId) -> Result<Utf8PathBuf, ResolveError> {
    let directory = id.directory(dir);
    let prefix = id.file_prefix();

    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ResolveError::NotFound(id));
        }
        Err(error) => {
            return Err(ResolveError::Io {
                dir: directory,
                error,
            });
        }
    };

    let mut files = vec![];
    for entry in entries {
        let entry = entry.map_err(|error| ResolveError::Io {
            dir: directory.clone(),
            error,
        })?;
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };

        if name.starts_with(&prefix) && Utf8Path::new(&name).extension() == Some("md") {
            files.push(directory.join(name));
        }
    }
    files.sort();

    match files.len() {
        0 => Err(ResolveError::NotFound(id)),
        1 => Ok(files.remove(0)),
        _ => Err(ResolveError::Ambiguous { id, files }),
    }
}

/// The ids the markdown files directly in `dir` open with.
///
/// A directory that doesn't exist holds none.
fn taken(dir: &Utf8Path) -> Result<HashSet<RfdId>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(format!("{dir}: {error}")),
    };

    let mut taken = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("{dir}: {error}"))?;
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        if Utf8Path::new(&name).extension() == Some("md")
            && let Some(id) = RfdId::of_file(&name)
        {
            taken.insert(id);
        }
    }

    Ok(taken)
}

/// The first draft slot no file in the drafts directory of the RFD directory
/// `dir` takes.
pub(crate) fn next_draft_slot(dir: &Utf8Path) -> Result<RfdId, String> {
    let taken = taken(&dir.join(DRAFTS_DIR))?;

    (1..=MAX_SLOT)
        .map(RfdId::Draft)
        .find(|id| !taken.contains(id))
        .ok_or_else(|| "No draft slots available (D01-D99 all in use).".to_owned())
}

/// The first permanent number no file in the RFD directory `dir` takes: the
/// lowest gap, or one past the highest.
///
/// Only the files here count, so a number taken on another branch is not seen.
pub(crate) fn next_number(dir: &Utf8Path) -> Result<RfdId, String> {
    let taken = taken(dir)?;

    (1..=MAX_NUMBER)
        .map(RfdId::Published)
        .find(|id| !taken.contains(id))
        .ok_or_else(|| "No permanent numbers available (001-999 all in use).".to_owned())
}

/// The file-name slug a title produces: lowercase, spaces as dashes, and every
/// character but ASCII letters, digits, `_`, and `-` dropped.
///
/// `Shareable RFD, Ticket, and Site Tooling` becomes
/// `shareable-rfd-ticket-and-site-tooling`.
#[must_use]
pub(crate) fn slug(title: &str) -> String {
    title
        .chars()
        .filter_map(|c| match c.to_ascii_lowercase() {
            ' ' => Some('-'),
            c @ ('a'..='z' | '0'..='9' | '_' | '-') => Some(c),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
#[path = "id_tests.rs"]
mod tests;
