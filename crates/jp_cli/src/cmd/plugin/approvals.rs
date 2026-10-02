//! The plugin approval store.
//!
//! Remembers the user's answers to the `ask` prompt, one binary per plugin
//! name: the file and the SHA-256 of its contents.
//! It also marks the binaries JP installed itself, which is how JP tells an
//! official binary it may update from one changed on this machine.
//!
//! The store is user-global and never enters config or a conversation: an
//! approval names a path on this machine.
//!
//! See: `docs/rfd/077-plugin-configuration-and-trust-policy.md`, "Approval
//! Store".

use std::{fs, io::Write as _};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::NamedUtf8TempFile;
use jp_plugin::registry::{ApprovedPlugin, PluginApprovals};
use tracing::warn;

use super::{
    discovery::{LocalPlugin, ManifestState},
    registry,
};
use crate::cmd;

/// Filename of the approval store in the user data directory.
const APPROVALS_FILE: &str = "plugin-approvals.json";

/// How a binary relates to the approval stored for its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalMatch {
    /// No approval for this name.
    None,

    /// Approved: this file, with these contents.
    Matches,

    /// This file was approved, and its contents changed since.
    Changed,

    /// Another file with this name is the approved one.
    Elsewhere(Utf8PathBuf),
}

/// The approvals, and where they are stored.
#[derive(Debug, Default)]
pub(crate) struct ApprovalStore {
    path: Option<Utf8PathBuf>,
    approvals: PluginApprovals,
}

impl ApprovalStore {
    /// Read the store from the user data directory.
    pub(crate) fn load() -> Self {
        Self::load_from(
            jp_workspace::user_data_dir()
                .ok()
                .map(|dir| dir.join(APPROVALS_FILE)),
        )
    }

    /// Read the store at `path`, treating a missing or malformed file as empty.
    pub(crate) fn load_from(path: Option<Utf8PathBuf>) -> Self {
        let approvals = path.as_deref().map(read).unwrap_or_default();
        Self { path, approvals }
    }

    /// The approval stored for `name`.
    pub(crate) fn get(&self, name: &str) -> Option<&ApprovedPlugin> {
        self.approvals.approved.get(name)
    }

    /// How the binary at `path`, with contents hashing to `sha256`, relates to
    /// the approval stored for `name`.
    pub(crate) fn check(&self, name: &str, path: &Utf8Path, sha256: &str) -> ApprovalMatch {
        let Some(approval) = self.get(name) else {
            return ApprovalMatch::None;
        };

        if !same_file(&approval.path, path) {
            return ApprovalMatch::Elsewhere(approval.path.clone());
        }

        if approval.sha256 == sha256 {
            ApprovalMatch::Matches
        } else {
            ApprovalMatch::Changed
        }
    }

    /// Record `approval` for `name`, replacing any earlier one, and save.
    ///
    /// The store is read again first, so an approval another `jp` recorded
    /// since this one loaded is kept.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be written.
    pub(crate) fn record(&mut self, name: &str, approval: ApprovedPlugin) -> cmd::Output {
        self.update(|approvals| {
            approvals.approved.insert(name.to_owned(), approval);
        })
    }

    /// Remove the approval for `name`, and save.
    ///
    /// Returns the approval that was removed.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be written.
    pub(crate) fn remove(&mut self, name: &str) -> Result<Option<ApprovedPlugin>, cmd::Error> {
        let mut removed = None;
        self.update(|approvals| removed = approvals.approved.remove(name))?;
        Ok(removed)
    }

    /// Give a binary without a readable manifest the manifest its approval
    /// recorded, while its contents are still the approved ones.
    ///
    /// Hashes only such binaries, which are rare: one compressed with UPX, or
    /// built without the manifest.
    pub(crate) fn apply_recorded(&self, local: &mut [LocalPlugin]) {
        for plugin in local {
            if plugin.manifest != ManifestState::Missing {
                continue;
            }

            let Some(approval) = self.get(&plugin.name) else {
                continue;
            };
            let Some(manifest) = &approval.manifest else {
                continue;
            };

            if !same_file(&approval.path, &plugin.path) {
                continue;
            }

            let Ok(sha256) = registry::sha256_file(&plugin.path) else {
                continue;
            };

            if sha256 == approval.sha256 {
                plugin.manifest = ManifestState::Recorded(manifest.clone());
            }
        }
    }

    fn update(&mut self, change: impl FnOnce(&mut PluginApprovals)) -> cmd::Output {
        let path = self
            .path
            .clone()
            .ok_or("cannot determine the user data directory for plugin approvals")?;

        let mut approvals = read(&path);
        change(&mut approvals);
        write(&path, &approvals)
            .map_err(|e| format!("failed to write plugin approvals to {path}: {e}"))?;

        self.approvals = approvals;
        Ok(())
    }
}

/// Whether two paths name the same file.
///
/// Compared after resolving symlinks, so a link on `$PATH` into a package store
/// is the file it points at.
fn same_file(a: &Utf8Path, b: &Utf8Path) -> bool {
    match (a.canonicalize_utf8(), b.canonicalize_utf8()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn read(path: &Utf8Path) -> PluginApprovals {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return PluginApprovals::default();
        }
        Err(error) => {
            warn!(%path, %error, "Failed to read plugin approvals; treating as empty.");
            return PluginApprovals::default();
        }
    };

    serde_json::from_str(&content).unwrap_or_else(|error| {
        warn!(%path, %error, "Malformed plugin approvals; treating as empty.");
        PluginApprovals::default()
    })
}

/// Replace the file whole, so a failed write leaves the old approvals.
fn write(path: &Utf8Path, approvals: &PluginApprovals) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    fs::create_dir_all(dir)?;

    let json = serde_json::to_vec_pretty(approvals).map_err(std::io::Error::other)?;
    let mut tmp = NamedUtf8TempFile::new_in(dir)?;
    tmp.write_all(&json)?;
    tmp.persist(path).map_err(std::io::Error::other)?;

    Ok(())
}

#[cfg(test)]
#[path = "approvals_tests.rs"]
mod tests;
