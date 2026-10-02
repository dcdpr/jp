//! Finding the plugin binaries on this machine and reading their manifests.
//!
//! [`discover`] lists every `jp-*` executable in JP's plugin install directory
//! and on `$PATH`, and reads each one's manifest without running it.
//!
//! See: `docs/rfd/072-command-plugin-system.md`, "Plugin Manifest" and "Plugin
//! Identity".

use std::{collections::HashSet, fs};

use camino::{Utf8Path, Utf8PathBuf, absolute_utf8};
use jp_plugin::manifest::{self, Manifest};
use tracing::debug;

use super::registry;

/// A `jp-*` executable found on this machine.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LocalPlugin {
    /// The file name without `jp-`, and without `.exe` on Windows.
    ///
    /// Keys the plugin's configuration and its approval.
    pub name: String,

    /// Where the binary was found.
    pub path: Utf8PathBuf,

    /// Whether JP's install directory holds it, or a `$PATH` directory.
    pub location: Location,

    /// What the binary's manifest says, or why it cannot be used.
    pub manifest: ManifestState,
}

/// Where a plugin binary was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Location {
    /// JP's own plugin install directory.
    InstallDir,

    /// A directory on `$PATH`.
    Path,
}

/// What reading a binary's manifest produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManifestState {
    /// A valid manifest.
    Valid(Manifest),

    /// The file carries no manifest, and its approval recorded the manifest
    /// fields its `describe` answer gave.
    ///
    /// Never read from a file: [`ApprovalStore::apply_recorded`] sets it, for a
    /// binary whose contents are still the approved ones.
    ///
    /// [`ApprovalStore::apply_recorded`]: super::approvals::ApprovalStore::apply_recorded
    Recorded(Manifest),

    /// The file carries no manifest.
    Missing,

    /// The file carries a manifest that cannot be used, and why.
    Invalid(String),
}

impl ManifestState {
    /// The manifest, when the binary has a usable one.
    pub(crate) fn valid(&self) -> Option<&Manifest> {
        match self {
            Self::Valid(manifest) | Self::Recorded(manifest) => Some(manifest),
            Self::Missing | Self::Invalid(_) => None,
        }
    }

    /// Why the binary claims nothing, for a listing.
    pub(crate) fn problem(&self) -> Option<String> {
        match self {
            Self::Valid(_) | Self::Recorded(_) => None,
            Self::Missing => Some("no plugin manifest".to_owned()),
            Self::Invalid(reason) => Some(format!("invalid manifest: {reason}")),
        }
    }
}

/// List the plugin binaries in JP's install directory and on `$PATH`, with
/// their manifests.
pub(crate) fn discover() -> Vec<LocalPlugin> {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let path_dirs: Vec<Utf8PathBuf> = std::env::split_paths(&path_var)
        .filter_map(|dir| Utf8PathBuf::from_path_buf(dir).ok())
        .collect();

    discover_in(registry::bin_dir().as_deref(), &path_dirs)
}

/// [`discover`], with every location passed in.
///
/// `install_dir` is searched before `path_dirs`, and both in order.
/// A file reached twice, through a symlink or a directory listed twice, is one
/// binary.
/// Two different files with the same name are both returned: that they conflict
/// is the router's to report.
///
/// Every returned path is absolute, resolved against the current directory, so
/// it names the same file from whatever directory it is later used.
/// Symlinks are kept, so a path reads the way it does on `$PATH`.
pub(crate) fn discover_in(
    install_dir: Option<&Utf8Path>,
    path_dirs: &[Utf8PathBuf],
) -> Vec<LocalPlugin> {
    let mut seen = HashSet::new();
    let mut plugins = Vec::new();

    let dirs = install_dir
        .map(|dir| (dir, Location::InstallDir))
        .into_iter()
        .chain(path_dirs.iter().map(|dir| (dir.as_path(), Location::Path)));

    for (dir, location) in dirs {
        // An empty `$PATH` entry has no absolute form, and names no directory
        // to list either.
        let Ok(dir) = absolute_utf8(dir) else {
            continue;
        };

        for (name, path) in executables(&dir) {
            // Identity is the file itself, so a symlink into a package store
            // and the file it points at are one binary.
            let Ok(canonical) = path.canonicalize_utf8() else {
                continue;
            };
            if !seen.insert(canonical.clone()) {
                continue;
            }

            let manifest = read_manifest(&canonical);
            plugins.push(LocalPlugin {
                name,
                path,
                location,
                manifest,
            });
        }
    }

    plugins
}

/// The `jp-*` executables directly inside `dir`, as `(name, path)` pairs.
fn executables(dir: &Utf8Path) -> Vec<(String, Utf8PathBuf)> {
    let Ok(entries) = dir.read_dir_utf8() else {
        return Vec::new();
    };

    let mut found: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = plugin_name(entry.file_name())?.to_owned();
            let path = entry.into_path();
            is_executable(&path).then_some((name, path))
        })
        .collect();

    found.sort();
    found
}

/// The plugin name a file name carries, if it is a plugin's file name.
pub(crate) fn plugin_name(file_name: &str) -> Option<&str> {
    let name = file_name.strip_prefix("jp-")?;

    #[cfg(windows)]
    let name = name.strip_suffix(".exe")?;

    (!name.is_empty()).then_some(name)
}

/// Whether `path` is a file this user can run.
fn is_executable(path: &Utf8Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };

    if !meta.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    true
}

/// Read the manifest of the binary at `path`.
pub(crate) fn read_manifest(path: &Utf8Path) -> ManifestState {
    debug!(%path, "Reading plugin manifest.");

    match fs::read(path) {
        Ok(bytes) => match manifest::find(&bytes) {
            Ok(Some(manifest)) => ManifestState::Valid(manifest),
            Ok(None) => ManifestState::Missing,
            Err(error) => ManifestState::Invalid(error.to_string()),
        },
        Err(error) => ManifestState::Invalid(format!("cannot read the file: {error}")),
    }
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
