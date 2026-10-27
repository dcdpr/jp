//! The settings snapshot: what the site needs from the project's configuration,
//! written to `.vitepress/jp-vite/settings.json`.
//!
//! The site's loaders also run without `jp`, in a static `vitepress build`, so
//! they read this file rather than the configuration.
//! It holds where the RFDs and tickets live and where the site serves them, the
//! RFD categories, the ticket vocabulary, how much of the Done column the board
//! shows, where links to repository files point, and the revision of the site
//! files it was written with.
//!
//! A workflow whose directory doesn't exist is `null`: the project doesn't have
//! it, and the site neither loads it nor links to it.
//! One that exists has to sit inside the VitePress source directory, which is
//! the only place VitePress serves pages from.

use std::collections::BTreeMap;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde_json::Value;

use crate::{
    options::{Options, rfd_options, ticket_options, web_address},
    site,
};

/// What the site reads of the project's configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Settings {
    /// The revision of the site files the snapshot was written with.
    pub revision: String,

    /// The VitePress source directory, relative to the workspace root; `.` for
    /// the root itself.
    pub site: String,

    /// Where links to repository files point, if anywhere.
    pub repository: Option<String>,

    /// The RFDs, when the project has them.
    pub rfd: Option<RfdSettings>,

    /// The tickets, when the project has them.
    pub ticket: Option<TicketSettings>,
}

/// Where the RFDs live, and what the site shows of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RfdSettings {
    /// The directory, relative to the workspace root, as `jp rfd` reads it.
    pub dir: String,

    /// Where the site serves it: the directory, relative to the source
    /// directory.
    pub route: String,

    /// The categories an RFD can carry, which the index filters by.
    pub categories: Vec<String>,
}

/// Where the tickets live, and what the site shows of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TicketSettings {
    /// The directory, relative to the workspace root, as `jp ticket` reads it.
    pub dir: String,

    /// Where the site serves it: the directory, relative to the source
    /// directory.
    pub route: String,

    /// How many Done tickets the board shows.
    pub done_limit: usize,

    /// The labels a ticket can carry, by key.
    pub labels: BTreeMap<String, Label>,
}

/// One key of the label vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Label {
    /// What the key covers.
    pub description: String,

    /// The values a ticket can be given; none for a key that takes none.
    pub values: Vec<String>,

    /// The values a ticket that already carries one may keep.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<String>,

    /// The fewest of the key's values a ticket carries; left out when it may
    /// carry none.
    #[serde(skip_serializing_if = "is_zero")]
    pub min: usize,

    /// The most of the key's values a ticket carries; left out when there is no
    /// limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_zero(count: &usize) -> bool {
    *count == 0
}

impl Settings {
    /// The snapshot as written to disk: pretty-printed JSON, keys in a fixed
    /// order, ending in a newline.
    pub(crate) fn render(&self) -> String {
        let mut json = serde_json::to_string_pretty(self).expect("settings serialize to JSON");
        json.push('\n');

        json
    }
}

/// The settings of the project at `root`, from the configuration the host
/// passed.
///
/// `origin` is git's `origin` remote, which `repository` falls back to.
pub(crate) fn snapshot(
    root: &Utf8Path,
    options: &Options,
    config: &Value,
    origin: Option<&str>,
) -> Result<Settings, String> {
    let site_dir = source_dir(root, options)?;
    let site = below(root, &site_dir).unwrap_or_else(|| ".".to_owned());

    let rfd_options = rfd_options(config)?;
    let rfd = workflow(root, &site_dir, &rfd_options.dir, "rfd")?.map(|(dir, route)| RfdSettings {
        dir,
        route,
        categories: rfd_options.categories,
    });

    let ticket_options = ticket_options(config)?;
    let ticket = workflow(root, &site_dir, &ticket_options.dir, "ticket")?.map(|(dir, route)| {
        TicketSettings {
            dir,
            route,
            done_limit: ticket_options.done_limit,
            labels: ticket_options
                .labels
                .facets()
                .map(|(key, facet)| {
                    (key.to_owned(), Label {
                        description: facet.description().to_owned(),
                        values: facet.values().map(ToOwned::to_owned).collect(),
                        retired: facet.retired().map(ToOwned::to_owned).collect(),
                        min: facet.min(),
                        max: facet.max(),
                    })
                })
                .collect(),
        }
    });

    Ok(Settings {
        revision: site::REVISION.to_owned(),
        site,
        repository: options
            .repository
            .clone()
            .or_else(|| origin.and_then(web_address)),
        rfd,
        ticket,
    })
}

/// The VitePress source directory, which has to be inside the workspace: the
/// site serves the project's own documents.
pub(crate) fn source_dir(root: &Utf8Path, options: &Options) -> Result<Utf8PathBuf, String> {
    let dir = resolve(root, &options.dir);
    if dir != normalize(root) && below(root, &dir).is_none() {
        return Err(format!(
            "`plugins.command.vite.options.dir` ({}) is outside the workspace.",
            options.dir
        ));
    }

    Ok(dir)
}

/// The workspace-relative directory of the workflow `name` configures as `dir`,
/// and its route on the site, when the directory exists.
fn workflow(
    root: &Utf8Path,
    site_dir: &Utf8Path,
    dir: &Utf8Path,
    name: &str,
) -> Result<Option<(String, String)>, String> {
    let absolute = resolve(root, dir);
    if !absolute.is_dir() {
        return Ok(None);
    }

    let route = below(site_dir, &absolute).ok_or_else(|| {
        format!(
            "`plugins.command.{name}.options.dir` ({dir}) isn't inside the site's source \
             directory ({}), and VitePress serves pages from nowhere else. Move it there, or \
             point `plugins.command.vite.options.dir` at a directory that holds it.",
            below(root, site_dir).unwrap_or_else(|| ".".to_owned()),
        )
    })?;

    // Below the source directory, which is below the root.
    let dir = below(root, &absolute).unwrap_or_else(|| absolute.to_string());

    Ok(Some((dir, route)))
}

/// `path` against the workspace root, with `.` and `..` folded away.
pub(crate) fn resolve(root: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&root.join(path))
    }
}

/// `path` with `.` and `..` folded away, without touching the disk.
fn normalize(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_str()),
        }
    }

    out
}

/// `path` below `base`, with `/` between its components; `None` when it isn't
/// strictly below it.
pub(crate) fn below(base: &Utf8Path, path: &Utf8Path) -> Option<String> {
    let rest = normalize(path)
        .strip_prefix(normalize(base))
        .ok()?
        .to_path_buf();
    let parts: Vec<&str> = rest.components().map(|part| part.as_str()).collect();

    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
