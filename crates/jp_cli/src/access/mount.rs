//! Parsing and resolution for the `--mount` / `--no-mount` CLI flags.
//!
//! A mount spec has the form `[TOOL:]NAME=PATH[:MODE]`:
//!
//! - `NAME` is a workspace-relative location for the symlink.
//! - `PATH` is the external target the symlink points at.
//! - `MODE` is `ro` (default) or `rw`.
//!   `rw` requires an explicit `TOOL:` prefix.
//! - `TOOL:` scopes the grant to a single tool; without it the grant expands to
//!   all enabled local tools.

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use jp_config::conversation::tool::access::FsRuleConfig;

/// Read/write mode for a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    /// Read-only (the default).
    Ro,
    /// Read-write.
    Rw,
}

/// A parsed `--mount` specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountSpec {
    /// Single tool the grant applies to, or `None` for all enabled local tools.
    pub tool: Option<String>,
    /// Workspace-relative symlink location, as typed (relative to CWD).
    pub name: String,
    /// External target the symlink points at, as typed.
    pub path: String,
    /// Read/write mode.
    pub mode: MountMode,
}

/// Failure reasons for parsing a mount spec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MountParseError {
    /// The spec is missing the required `=` separating name from path.
    #[error("invalid mount '{0}': expected NAME=PATH")]
    MissingEquals(String),

    /// `NAME` or `PATH` was empty.
    #[error("invalid mount '{0}': NAME and PATH must both be non-empty")]
    Empty(String),

    /// The tool prefix is not a valid tool identifier.
    #[error("invalid mount '{spec}': '{tool}' is not a valid tool name")]
    InvalidTool {
        /// The whole spec.
        spec: String,
        /// The offending tool segment.
        tool: String,
    },

    /// `:rw` was requested without a `TOOL:` prefix.
    #[error(
        "invalid mount '{0}': ':rw' requires a TOOL: prefix (e.g. fs_modify_file:NAME=PATH:rw)"
    )]
    WriteRequiresTool(String),
}

impl MountSpec {
    /// Parse a `[TOOL:]NAME=PATH[:MODE]` spec.
    ///
    /// The first `=` splits name from path; a `:` on the left names the tool, a
    /// trailing `:ro`/`:rw` on the right sets the mode.
    /// Windows drive letters only appear to the right of `=`, where the mode is
    /// peeled from the tail, so they don't collide with the tool prefix.
    pub fn parse(spec: &str) -> Result<Self, MountParseError> {
        let (left, right) = spec
            .split_once('=')
            .ok_or_else(|| MountParseError::MissingEquals(spec.to_owned()))?;

        let (tool, name) = match left.split_once(':') {
            Some((tool, name)) => {
                if !is_tool_identifier(tool) {
                    return Err(MountParseError::InvalidTool {
                        spec: spec.to_owned(),
                        tool: tool.to_owned(),
                    });
                }
                (Some(tool.to_owned()), name)
            }
            None => (None, left),
        };

        let (path, mode) = match right.strip_suffix(":rw") {
            Some(path) => (path, MountMode::Rw),
            None => match right.strip_suffix(":ro") {
                Some(path) => (path, MountMode::Ro),
                None => (right, MountMode::Ro),
            },
        };

        if name.is_empty() || path.is_empty() {
            return Err(MountParseError::Empty(spec.to_owned()));
        }

        if mode == MountMode::Rw && tool.is_none() {
            return Err(MountParseError::WriteRequiresTool(spec.to_owned()));
        }

        Ok(Self {
            tool,
            name: name.to_owned(),
            path: path.to_owned(),
            mode,
        })
    }

    /// Resolve `name` to a workspace-relative path.
    ///
    /// `name` is interpreted relative to `cwd`, normalized lexically, and must
    /// land under `workspace_root`.
    /// Returns the workspace-relative symlink location (the `lexical_path` the
    /// resulting rule will carry).
    ///
    /// # Errors
    ///
    /// Returns an error if the resolved location escapes the workspace or lands
    /// under JP-managed storage (`.jp/`).
    pub fn resolve_name(
        &self,
        cwd: &Utf8Path,
        workspace_root: &Utf8Path,
    ) -> Result<Utf8PathBuf, MountResolveError> {
        resolve_workspace_relative(&self.name, cwd, workspace_root)
    }

    /// Build the `access.fs` rule for this mount at the given
    /// workspace-relative rule path.
    #[must_use]
    pub fn rule(&self, rule_path: &str) -> FsRuleConfig {
        FsRuleConfig {
            path: rule_path.to_owned(),
            external: Some(true),
            read: Some(true),
            write: Some(self.mode == MountMode::Rw),
            create: None,
            update: None,
            delete: None,
            execute: None,
        }
    }
}

/// Failure reasons for resolving a mount `NAME`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MountResolveError {
    /// `NAME` is absolute or rooted; it must be workspace-relative.
    #[error("mount name '{0}' must be relative")]
    NotRelative(String),

    /// The resolved location escapes the workspace root.
    #[error("mount '{0}' resolves outside the workspace")]
    OutsideWorkspace(String),

    /// The resolved location is under JP-managed storage.
    #[error("mount '{0}' targets JP-managed storage (.jp/)")]
    ManagedStorage(String),
}

fn resolve_workspace_relative(
    name: &str,
    cwd: &Utf8Path,
    workspace_root: &Utf8Path,
) -> Result<Utf8PathBuf, MountResolveError> {
    // `NAME` is interpreted relative to the current directory. Reject absolute
    // and rooted inputs outright rather than silently accepting one that
    // happens to fall under the workspace.
    let raw = Utf8Path::new(name);
    if raw.is_absolute() || raw.has_root() {
        return Err(MountResolveError::NotRelative(name.to_owned()));
    }

    let joined = cwd.join(name);

    let normalized = normalize_lexical(&joined);

    let relative = normalized
        .strip_prefix(workspace_root)
        .map_err(|_| MountResolveError::OutsideWorkspace(name.to_owned()))?;

    if relative.components().next().is_none() {
        return Err(MountResolveError::OutsideWorkspace(name.to_owned()));
    }

    if relative.starts_with(".jp") {
        return Err(MountResolveError::ManagedStorage(name.to_owned()));
    }

    Ok(relative.to_owned())
}

/// Lexically normalize an absolute path, collapsing `.` and `..` without
/// touching the filesystem.
/// `..` cannot pop above the filesystem root.
fn normalize_lexical(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                out.pop();
            }
            Utf8Component::RootDir => out.push("/"),
            Utf8Component::Prefix(prefix) => out.push(prefix.as_str()),
            Utf8Component::Normal(segment) => out.push(segment),
        }
    }
    out
}

/// Whether `s` is a valid tool identifier (`[a-z_][a-z0-9_]*`).
fn is_tool_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_lowercase() || c.is_ascii_digit())
}

#[cfg(test)]
#[path = "mount_tests.rs"]
mod tests;
