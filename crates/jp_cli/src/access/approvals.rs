//! User-local approval store for external symlink-mount targets.
//!
//! Approvals bind a workspace-relative mount path to a canonical absolute
//! target on a trust-on-first-use basis.
//! The store lives outside the conversation stream because the canonical target
//! is a host-local path that must not enter shared conversation state.

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// File name of the approval store within user-workspace storage.
pub const APPROVALS_FILE: &str = "approvals.json";

/// The on-disk approval store.
///
/// `mounts` is the only approval category in v1; future categories can join as
/// sibling fields.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalStore {
    #[serde(default)]
    mounts: Vec<MountApproval>,
}

/// A single approved `(rule_path -> canonical_target)` binding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MountApproval {
    /// The workspace-relative mount path (e.g.
    /// `fork`).
    pub rule_path: String,
    /// The canonical absolute target the mount is approved to resolve to.
    pub canonical_target: Utf8PathBuf,
    /// When the binding was approved.
    pub approved_at: DateTime<Utc>,
}

/// The result of consulting the store for a `(rule_path, candidate)` pair.
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalLookup {
    /// An approval exists whose target matches the candidate.
    Approved,
    /// An approval exists but for a different target (retargeting).
    Retargeted {
        /// The previously approved target.
        previous: Utf8PathBuf,
    },
    /// No approval exists for this rule path.
    Unknown,
}

impl ApprovalStore {
    /// Read the store from `path`, treating a missing or malformed file as an
    /// empty store (with a warning for malformed content).
    #[must_use]
    pub fn load(path: &Utf8Path) -> Self {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                warn!(%path, %error, "Failed to read approval store; treating as empty.");
                return Self::default();
            }
        };

        serde_json::from_str(&raw).unwrap_or_else(|error| {
            warn!(%path, %error, "Malformed approval store; treating as empty.");
            Self::default()
        })
    }

    /// Persist the store to `path` atomically (write to a temp file in the same
    /// directory, then rename over the target).
    ///
    /// # Errors
    ///
    /// Returns an error if the parent directory cannot be created or the
    /// temp-file write or rename fails.
    pub fn save(&self, path: &Utf8Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let dir = path.parent().unwrap_or_else(|| Utf8Path::new("."));
        let mut tmp = camino_tempfile::NamedUtf8TempFile::new_in(dir)?;
        std::io::Write::write_all(&mut tmp, json.as_bytes())?;
        tmp.persist(path).map_err(std::io::Error::other)?;

        Ok(())
    }

    /// Decide how a candidate target relates to the stored approval.
    #[must_use]
    pub fn lookup(&self, rule_path: &str, candidate: &Utf8Path) -> ApprovalLookup {
        match self.mounts.iter().find(|m| m.rule_path == rule_path) {
            None => ApprovalLookup::Unknown,
            Some(m) if m.canonical_target == candidate => ApprovalLookup::Approved,
            Some(m) => ApprovalLookup::Retargeted {
                previous: m.canonical_target.clone(),
            },
        }
    }

    /// Record (or replace) an approval for `rule_path`.
    pub fn record(&mut self, rule_path: &str, canonical_target: Utf8PathBuf, now: DateTime<Utc>) {
        if let Some(existing) = self.mounts.iter_mut().find(|m| m.rule_path == rule_path) {
            existing.canonical_target = canonical_target;
            existing.approved_at = now;
        } else {
            self.mounts.push(MountApproval {
                rule_path: rule_path.to_owned(),
                canonical_target,
                approved_at: now,
            });
        }
    }
}

#[cfg(test)]
#[path = "approvals_tests.rs"]
mod tests;
