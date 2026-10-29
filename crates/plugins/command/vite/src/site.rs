//! The files `jp vite` writes in a project, built from the binary.
//!
//! The binary carries the crate's `site/` directory ([`FILES`]).
//! `jp-vite/` is owned: it is written as it is to the project's
//! `.vitepress/jp-vite/`, together with the settings snapshot, and every sync
//! rewrites it.
//! `pages/` holds a starting page for each list the workflows show:
//! `pages/rfd/` goes to the RFD directory and `pages/ticket/` to the ticket
//! directory, for the workflows the project has.
//! A page is written only where there is none, and is the project's from then
//! on: it renders its list with one tag, such as `<RfdIndex />`, which the
//! theme entry registers.

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

/// One file `jp vite` writes, and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Owned {
    /// Where it goes, relative to the workspace root.
    pub path: Utf8PathBuf,

    pub contents: Vec<u8>,
}

/// `rest` below the project path `base`, joined with `/`.
///
/// Paths relative to the workspace root are written with `/` on every platform,
/// as git and the configuration write them, so a listing reads the same
/// everywhere; a file operation takes them joined to the root.
pub(crate) fn join(base: impl AsRef<str>, rest: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{}/{rest}", base.as_ref()))
}

/// The directory holding the files written as they are, relative to the
/// workspace root.
pub(crate) fn owned_dir(settings: &Settings) -> Utf8PathBuf {
    if settings.site == "." {
        Utf8PathBuf::from(OWNED_DIR)
    } else {
        join(&settings.site, OWNED_DIR)
    }
}

/// Where the snapshot goes, relative to the workspace root.
pub(crate) fn settings_path(settings: &Settings) -> Utf8PathBuf {
    join(owned_dir(settings), SETTINGS_FILE)
}

/// Every file `jp vite` owns in a project with these settings, ordered by path:
/// the files under [`OWNED_DIR`], the snapshot among them.
pub(crate) fn owned(settings: &Settings) -> Vec<Owned> {
    let dir = owned_dir(settings);
    let mut owned: Vec<Owned> = FILES
        .iter()
        .filter_map(|(path, contents)| {
            let rest = path.strip_prefix("jp-vite/")?;
            Some(Owned {
                path: join(&dir, rest),
                contents: contents.to_vec(),
            })
        })
        .collect();

    owned.push(Owned {
        path: settings_path(settings),
        contents: settings.render().into_bytes(),
    });
    owned.sort_by(|a, b| a.path.cmp(&b.path));

    owned
}

/// The starting pages of the workflows the project has, ordered by path.
pub(crate) fn pages(settings: &Settings) -> Vec<Owned> {
    let mut pages: Vec<Owned> = FILES
        .iter()
        .filter_map(|(path, contents)| {
            let (dir, rest) = if let Some(rest) = path.strip_prefix("pages/rfd/") {
                (&settings.rfd.as_ref()?.dir, rest)
            } else {
                (
                    &settings.ticket.as_ref()?.dir,
                    path.strip_prefix("pages/ticket/")?,
                )
            };

            Some(Owned {
                path: join(dir, rest),
                contents: contents.to_vec(),
            })
        })
        .collect();
    pages.sort_by(|a, b| a.path.cmp(&b.path));

    pages
}

#[cfg(test)]
#[path = "site_tests.rs"]
mod tests;
