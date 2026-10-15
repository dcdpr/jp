//! The files `jp vite` owns in a project, built from the binary.
//!
//! The binary carries the crate's `site/` directory ([`FILES`]).
//! `jp-vite/` is written as it is to the project's `.vitepress/jp-vite/`, and
//! each page in `pages/` to the directory of the workflow it belongs to:
//! `pages/rfd/` to the RFD directory and `pages/ticket/` to the ticket
//! directory, for the workflows the project has.
//! A page names `.vitepress/jp-vite` as `@@jp-vite@@`, which is replaced with
//! the path from the page to it, since that depends on how deep the page sits.
//! The settings snapshot joins them as `.vitepress/jp-vite/settings.json`.

use camino::Utf8PathBuf;

use crate::settings::Settings;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/site.rs"));
}

pub(crate) use embedded::FILES;

/// The revision of the site files the binary carries.
///
/// `jp ticket` and `jp rfd` carry the revision of the files they were built
/// alongside, computed the same way.
pub(crate) const REVISION: &str = env!("JP_SITE_REVISION");

/// Where the files written as they are live, below the source directory.
pub(crate) const OWNED_DIR: &str = ".vitepress/jp-vite";

/// The snapshot's name in [`OWNED_DIR`].
pub(crate) const SETTINGS_FILE: &str = "settings.json";

/// What a page writes in place of the path from it to [`OWNED_DIR`].
const PLACEHOLDER: &str = "@@jp-vite@@";

/// One file `jp vite` owns, and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Owned {
    /// Where it goes, relative to the workspace root.
    pub path: Utf8PathBuf,

    pub contents: Vec<u8>,
}

/// The directory holding the files written as they are, relative to the
/// workspace root.
pub(crate) fn owned_dir(settings: &Settings) -> Utf8PathBuf {
    if settings.site == "." {
        Utf8PathBuf::from(OWNED_DIR)
    } else {
        Utf8PathBuf::from(&settings.site).join(OWNED_DIR)
    }
}

/// Where the snapshot goes, relative to the workspace root.
pub(crate) fn settings_path(settings: &Settings) -> Utf8PathBuf {
    owned_dir(settings).join(SETTINGS_FILE)
}

/// Every file `jp vite` owns in a project with these settings, ordered by path:
/// the files under [`OWNED_DIR`], the snapshot among them, and the pages of the
/// workflows the project has.
pub(crate) fn owned(settings: &Settings) -> Vec<Owned> {
    let dir = owned_dir(settings);
    let mut owned = vec![];

    for (path, contents) in FILES {
        if let Some(rest) = path.strip_prefix("jp-vite/") {
            owned.push(Owned {
                path: dir.join(rest),
                contents: contents.to_vec(),
            });
        } else if let Some(rest) = path.strip_prefix("pages/rfd/")
            && let Some(rfd) = &settings.rfd
        {
            owned.push(page(&rfd.dir, &rfd.route, rest, contents));
        } else if let Some(rest) = path.strip_prefix("pages/ticket/")
            && let Some(ticket) = &settings.ticket
        {
            owned.push(page(&ticket.dir, &ticket.route, rest, contents));
        }
    }

    owned.push(Owned {
        path: settings_path(settings),
        contents: settings.render().into_bytes(),
    });
    owned.sort_by(|a, b| a.path.cmp(&b.path));

    owned
}

/// The page at `rest` in a workflow's directory, `dir` on disk and `route` on
/// the site, pointing at [`OWNED_DIR`] from where it sits.
fn page(dir: &str, route: &str, rest: &str, contents: &[u8]) -> Owned {
    // The page's own directory is `route/rest` without its last component.
    let depth = route.split('/').count() + rest.split('/').count() - 1;
    let path_to_owned = format!("{}{OWNED_DIR}", "../".repeat(depth));

    Owned {
        path: Utf8PathBuf::from(dir).join(rest),
        contents: String::from_utf8_lossy(contents)
            .replace(PLACEHOLDER, &path_to_owned)
            .into_bytes(),
    }
}

#[cfg(test)]
#[path = "site_tests.rs"]
mod tests;
