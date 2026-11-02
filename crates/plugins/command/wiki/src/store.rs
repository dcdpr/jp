//! The wiki directory: every page file in it, read, and the order they are read
//! in.
//!
//! Files are named `<id>-<slug>.md`; anything else in the directory, an
//! `index.md` among it, is not a page.
//! Ids are allocated the way tickets' are, by `doc::files`, from the
//! directory's own epoch.
//!
//! A page the command's access doesn't let it read is known by its file name
//! alone: it exists, and nothing else about it is.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use doc::files;
use jp_tool::Capability;

use crate::{
    access::Access,
    order::{ORDER_FILE, Order},
    page::{Page, PageId, WikiKind},
};

/// A page file the command's access doesn't let it read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hidden {
    pub id: PageId,
    pub path: Utf8PathBuf,

    /// What the access said: `access denied: read on docs/wiki/...`.
    pub refusal: String,
}

/// Every page in the wiki, and the order file.
#[derive(Debug, Clone)]
pub(crate) struct Wiki {
    /// Every page file read, ordered by id; two files claiming one id are both
    /// here.
    pub pages: Vec<Page>,

    /// The page files that couldn't be read, ordered by id.
    pub hidden: Vec<Hidden>,

    pub order: Order,
}

impl Wiki {
    /// Read every page in `dir` that `access` allows, and its order file.
    ///
    /// A directory that doesn't exist yet holds no pages.
    /// A directory, or an order file, that `access` doesn't let the command
    /// read is an error: no page can be found, or ranked, without it.
    pub(crate) fn load(dir: &Utf8Path, access: &Access) -> Result<Self, String> {
        access
            .check(Capability::Read, dir)
            .map_err(|refusal| format!("The wiki directory can't be read: {refusal}."))?;

        let mut pages = vec![];
        let mut hidden = vec![];
        for (id, path) in files::list::<WikiKind>(dir).map_err(|error| format!("{dir}: {error}"))? {
            match access.check(Capability::Read, &path) {
                Ok(()) => {
                    let source = read(&path)?;
                    pages.push(Page::read(id, path, source));
                }
                Err(refusal) => hidden.push(Hidden { id, path, refusal }),
            }
        }

        let order_file = dir.join(ORDER_FILE);
        if order_file.is_file() {
            access
                .check(Capability::Read, &order_file)
                .map_err(|refusal| format!("{order_file} can't be read: {refusal}."))?;
        }

        Ok(Self {
            pages,
            hidden,
            order: Order::read(dir)?,
        })
    }

    /// The page `id` names.
    ///
    /// Two files claiming the id is an error rather than a choice: a write
    /// would otherwise land on whichever the directory yielded first.
    /// A page that couldn't be read is an error saying why.
    pub(crate) fn find(&self, id: PageId) -> Result<&Page, String> {
        let pages: Vec<&Page> = self.pages.iter().filter(|page| page.id == id).collect();
        let hidden: Vec<&Hidden> = self.hidden.iter().filter(|page| page.id == id).collect();

        match (pages.as_slice(), hidden.as_slice()) {
            ([one], []) => Ok(one),
            ([], [one]) => Err(format!("{id} can't be read: {}.", one.refusal)),
            ([], []) => Err(format!("No wiki page {id}.")),
            (pages, hidden) => Err(format!(
                "{id} is claimed by more than one file: {}.",
                pages
                    .iter()
                    .map(|page| page.path.as_str())
                    .chain(hidden.iter().map(|page| page.path.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// The file of page `id`, whether or not it could be read.
    #[must_use]
    pub(crate) fn path_of(&self, id: PageId) -> Option<&Utf8Path> {
        self.pages
            .iter()
            .find(|page| page.id == id)
            .map(|page| page.path.as_path())
            .or_else(|| {
                self.hidden
                    .iter()
                    .find(|page| page.id == id)
                    .map(|page| page.path.as_path())
            })
    }

    /// Whether a file carries page `id`, whether or not it could be read.
    #[must_use]
    pub(crate) fn exists(&self, id: PageId) -> bool {
        self.path_of(id).is_some()
    }

    /// What page `id` is called: its title, or for a page that couldn't be
    /// read, its file name.
    #[must_use]
    pub(crate) fn name_of(&self, id: PageId) -> Option<String> {
        if let Some(page) = self.pages.iter().find(|page| page.id == id) {
            return Some(page.name().to_owned());
        }

        self.hidden
            .iter()
            .find(|page| page.id == id)
            .and_then(|page| page.path.file_stem())
            .map(ToOwned::to_owned)
    }

    /// The pages whose `Parent` names `id` and that aren't archived.
    #[must_use]
    pub(crate) fn active_children(&self, id: PageId) -> Vec<&Page> {
        self.pages
            .iter()
            .filter(|page| page.parent_id() == Some(id) && page.id != id && !page.is_archived())
            .collect()
    }
}

/// Write a new page, `contents`, named for a fresh id and `title`, returning
/// its id and path.
pub(crate) fn create(
    dir: &Utf8Path,
    title: &str,
    contents: &str,
) -> Result<(PageId, Utf8PathBuf), String> {
    files::create::<WikiKind>(dir, &files::slug(title), contents).map_err(|error| error.to_string())
}

/// Where the page `id` lives once its title is `title`.
#[must_use]
pub(crate) fn path_for(dir: &Utf8Path, id: PageId, title: &str) -> Utf8PathBuf {
    dir.join(format!("{}{}.md", id.file_prefix(), files::slug(title)))
}

/// Read a file, with an error naming it.
pub(crate) fn read(path: &Utf8Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))
}

/// Write a file, with an error naming it.
pub(crate) fn write(path: &Utf8Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|error| format!("{path}: {error}"))
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
