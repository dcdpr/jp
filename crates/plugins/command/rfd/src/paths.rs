//! Where the commands read and write: the workspace root, the RFD directory,
//! and the ticket directory.

use std::{
    fs,
    io::{self, Write as _},
};

use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    board,
    id::{self, DRAFTS_DIR, RfdId},
};

/// The directories a command works in, each an absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Paths {
    /// The workspace root.
    pub root: Utf8PathBuf,

    /// The RFD directory, holding the published RFDs and the drafts directory.
    pub rfds: Utf8PathBuf,

    /// The ticket directory, which `jp ticket` writes and which says what is in
    /// development.
    pub tickets: Utf8PathBuf,
}

impl Paths {
    /// The directories `rfds` and `tickets` name, resolved against `root`
    /// unless they are absolute.
    #[must_use]
    pub(crate) fn new(root: &Utf8Path, rfds: &Utf8Path, tickets: &Utf8Path) -> Self {
        let resolve = |dir: &Utf8Path| {
            if dir.is_absolute() {
                dir.to_path_buf()
            } else {
                root.join(dir)
            }
        };

        Self {
            root: root.to_path_buf(),
            rfds: resolve(rfds),
            tickets: resolve(tickets),
        }
    }

    /// The drafts directory, inside the RFD directory.
    #[must_use]
    pub(crate) fn drafts(&self) -> Utf8PathBuf {
        self.rfds.join(DRAFTS_DIR)
    }

    /// The documentation tree the RFD directory sits in, where documents link
    /// to an RFD by its file name: `docs` for `docs/rfd`.
    ///
    /// The RFD directory itself when its parent is the workspace root or lies
    /// outside it, so a rewrite never reaches into the whole workspace.
    #[must_use]
    pub(crate) fn docs(&self) -> Utf8PathBuf {
        self.rfds
            .parent()
            .filter(|parent| *parent != self.root && parent.starts_with(&self.root))
            .map_or_else(|| self.rfds.clone(), Utf8Path::to_path_buf)
    }

    /// The directories a link to an RFD file can be in: the documentation tree,
    /// and the ticket directory when it lies outside that tree.
    #[must_use]
    pub(crate) fn linking(&self) -> Vec<Utf8PathBuf> {
        let docs = self.docs();
        if self.tickets.starts_with(&docs) {
            vec![docs]
        } else {
            vec![docs, self.tickets.clone()]
        }
    }

    /// `path` the way the user reads it: relative to the workspace root, with
    /// `/` separators.
    #[must_use]
    pub(crate) fn show(&self, path: &Utf8Path) -> String {
        board::relative(&self.root, path)
    }

    /// The one file `id` names.
    pub(crate) fn resolve(&self, id: RfdId) -> Result<Utf8PathBuf, String> {
        id::resolve(&self.rfds, id).map_err(|error| error.to_string())
    }
}

/// Read a document, with an error naming it.
pub(crate) fn read(path: &Utf8Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))
}

/// Write a document, with an error naming it.
pub(crate) fn write(path: &Utf8Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|error| format!("{path}: {error}"))
}

/// Write a document that must not exist yet.
pub(crate) fn create(path: &Utf8Path, text: &str) -> Result<(), String> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => format!("{path} already exists."),
            _ => format!("{path}: {error}"),
        })
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
