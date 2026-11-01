//! The RFDs and tickets a wiki page can name, read once per command.
//!
//! A page names them by id, `RFD 042` or `T-02wt0kx`, or links to their files;
//! [`Library`] knows which exist, what each is titled, and what an RFD's status
//! is, which is what `check` and `show` ask.
//!
//! A command reads only what its access allows: a document it can't read is
//! known by its file name, and a directory it can't list leaves its documents
//! unknown, which [`Lookup::Unknown`] tells apart from missing.

use std::{collections::BTreeMap, fs};

use camino::{Utf8Path, Utf8PathBuf};
use doc::{files, metadata::Header};
use ticket::id::TicketKind;

use crate::{access::Access, options::Paths, page::PageId};

/// The directory drafts live in, inside the RFD directory.
const DRAFTS_DIR: &str = "drafts";

/// An RFD or a ticket.
#[derive(Debug, Clone)]
pub(crate) struct Document {
    pub path: Utf8PathBuf,

    /// Its title, or its file name when it couldn't be read.
    pub title: String,

    /// The first word of its `Status`: `Accepted` for `Accepted (2026-08-05)`.
    pub status: Option<String>,

    /// Its text; `None` when the command's access doesn't let it read the file.
    pub source: Option<String>,
}

/// What a lookup by id found.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Lookup<T> {
    Found(T),

    /// The directory that would hold it was listed, and it isn't there.
    Missing,

    /// The directory that would hold it couldn't be listed.
    Unknown,
}

/// Every RFD and ticket in the workspace.
#[derive(Debug, Clone, Default)]
pub(crate) struct Library {
    /// RFDs by id: `042`, or `D05` for a draft.
    pub rfds: BTreeMap<String, Document>,

    /// Tickets by canonical id, `T-02wt0kx`.
    pub tickets: BTreeMap<String, Document>,

    /// The directories the command's access didn't let it list.
    pub unlisted: Vec<Utf8PathBuf>,

    rfd_dir: Utf8PathBuf,
    drafts_dir: Utf8PathBuf,
    ticket_dir: Utf8PathBuf,
}

impl Library {
    /// Read the RFDs and tickets of the workspace `paths` describe, as far as
    /// `access` allows.
    ///
    /// A directory that doesn't exist holds nothing, and a file that can't be
    /// read for any reason but the access is passed over: these are documents
    /// the wiki links to, and a problem with one is theirs to report.
    #[must_use]
    pub(crate) fn load(paths: &Paths, access: &Access) -> Self {
        let mut library = Self {
            rfd_dir: paths.rfds.clone(),
            drafts_dir: paths.rfds.join(DRAFTS_DIR),
            ticket_dir: paths.tickets.clone(),
            ..Self::default()
        };

        for (dir, digits, prefix) in [
            (library.rfd_dir.clone(), 3, ""),
            (library.drafts_dir.clone(), 2, "D"),
        ] {
            if !library.lists(&dir, access) {
                continue;
            }
            for path in markdown_files(&dir) {
                let Some(id) = rfd_id(&path, digits, prefix) else {
                    continue;
                };
                if let Some(document) = read(&path, access, rfd_title) {
                    library.rfds.insert(id, document);
                }
            }
        }

        let ticket_dir = library.ticket_dir.clone();
        if library.lists(&ticket_dir, access) {
            for (id, path) in files::list::<TicketKind>(&ticket_dir).unwrap_or_default() {
                if let Some(document) = read(&path, access, crate::page::title) {
                    library.tickets.insert(id.to_string(), document);
                }
            }
        }

        library
    }

    /// Whether `dir` can be listed under `access`; one that can't is recorded
    /// as unlisted.
    ///
    /// A directory that isn't there lists as empty.
    fn lists(&mut self, dir: &Utf8Path, access: &Access) -> bool {
        if !dir.is_dir() || access.reads(dir) {
            return true;
        }

        self.unlisted.push(dir.to_path_buf());
        false
    }

    /// The RFD `id` names: `042`, or `D05` for a draft.
    #[must_use]
    pub(crate) fn rfd(&self, id: &str) -> Lookup<&Document> {
        let dir = if id.starts_with('D') {
            &self.drafts_dir
        } else {
            &self.rfd_dir
        };

        lookup(self.rfds.get(id), self.unlisted.contains(dir))
    }

    /// The ticket `id` names, as `T-02wt0kx`.
    #[must_use]
    pub(crate) fn ticket(&self, id: &str) -> Lookup<&Document> {
        lookup(
            self.tickets.get(id),
            self.unlisted.contains(&self.ticket_dir),
        )
    }

    /// Every document whose text couldn't be read, by path.
    pub(crate) fn unread(&self) -> impl Iterator<Item = &Utf8Path> {
        self.rfds
            .values()
            .chain(self.tickets.values())
            .filter(|document| document.source.is_none())
            .map(|document| document.path.as_path())
    }

    /// The name a document at `path` goes by in a mention: `W-0a1b2c3` for a
    /// wiki page in `wiki`, `RFD 042`, `T-02wt0kx`, or `None` for any other
    /// file.
    #[must_use]
    pub(crate) fn name_of(&self, path: &Utf8Path, wiki: &Utf8Path) -> Option<String> {
        if path.parent() == Some(wiki) {
            let (id, _) = path.file_stem()?.split_once('-')?;
            return id.parse::<PageId>().ok().map(|id| id.to_string());
        }

        let rfd = self
            .rfds
            .iter()
            .find(|(_, document)| document.path == path)
            .map(|(id, _)| format!("RFD {id}"));
        let ticket = || {
            self.tickets
                .iter()
                .find(|(_, document)| document.path == path)
                .map(|(id, _)| id.clone())
        };

        rfd.or_else(ticket)
    }
}

/// What a lookup found, given whether the directory it looked in was listed.
fn lookup(found: Option<&Document>, unlisted: bool) -> Lookup<&Document> {
    match found {
        Some(document) => Lookup::Found(document),
        None if unlisted => Lookup::Unknown,
        None => Lookup::Missing,
    }
}

/// The markdown files directly in `dir`, by name.
fn markdown_files(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return vec![];
    };

    let mut found: Vec<Utf8PathBuf> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| Utf8PathBuf::try_from(entry.path()).ok())
        .filter(|path| path.extension() == Some("md"))
        .collect();
    found.sort();

    found
}

/// The id an RFD file is named by: `042` for `042-slug.md`, `D05` for
/// `D05-slug.md`; `None` for a template or any other name.
fn rfd_id(path: &Utf8Path, digits: usize, prefix: &str) -> Option<String> {
    let (id, slug) = path.file_stem()?.split_once('-')?;
    let number = id.strip_prefix(prefix)?;
    let numbered = number.len() == digits
        && number.bytes().all(|b| b.is_ascii_digit())
        && number.bytes().any(|b| b != b'0');

    (numbered && !slug.is_empty()).then(|| id.to_owned())
}

/// The title an RFD's `# RFD 042: Title` heading carries.
fn rfd_title(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let (_, title) = line.strip_prefix("# RFD ")?.split_once(':')?;
        Some(title.trim().to_owned())
    })
}

/// Read the document at `path`, titled by `title`, or by its file name.
///
/// A document `access` doesn't let the command read is known by its file name
/// alone.
fn read(
    path: &Utf8Path,
    access: &Access,
    title: impl Fn(&str) -> Option<String>,
) -> Option<Document> {
    let named = path.file_stem().unwrap_or_default().to_owned();
    if !access.reads(path) {
        return Some(Document {
            path: path.to_path_buf(),
            title: named,
            status: None,
            source: None,
        });
    }

    let source = fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = source.lines().collect();
    let status = Header::of(&lines).field("Status").map(|status| {
        status
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect::<String>()
    });

    Some(Document {
        path: path.to_path_buf(),
        title: title(&source).unwrap_or(named),
        status: status.filter(|status| !status.is_empty()),
        source: Some(source),
    })
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
